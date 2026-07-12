use std::{
    collections::HashMap,
    net::SocketAddr,
    time::{Duration, Instant},
};

use rand::RngExt as _;
use sha2::{Digest as _, Sha256};
use tractor_beam_relay_protocol::v2::{CAP_ROOM_PATH_PROBE, DuplicateDecision, FrameIdWindow};

use crate::{
    config::RelayConfig,
    domain::PeerId,
    domain_v2::{
        DataDestination, DataProfile, DataSource, JoinBegin, JoinChallenge, JoinReady, PathKey,
        PeerView, Presence, PresenceBroadcast, ResumeFailure, ResumeKey, ResumeReady, RouteData,
        RouteProbe, SessionKey, StateError,
    },
};

const RECOVERY_GRACE: Duration = Duration::from_secs(120);
const PROBE_RATE_LIMIT_PER_SECOND: u32 = 10;

#[derive(Clone, Debug)]
struct PendingJoin {
    session: SessionKey,
    steam_id64: u64,
    display_name: Option<String>,
    profile: DataProfile,
    capabilities: u64,
    challenge_id: [u8; 16],
    pow_nonce: [u8; 16],
    issued_at: Instant,
}

#[derive(Debug)]
struct Peer {
    control_peer: PeerId,
    connection_id: u64,
    resume_key: ResumeKey,
    path_key: Option<PathKey>,
    udp_address: Option<SocketAddr>,
    steam_id64: u64,
    display_name: Option<String>,
    profile: DataProfile,
    capabilities: u64,
    presence: Presence,
    detached_until: Option<Instant>,
    frames: FrameIdWindow,
    last_seen: Instant,
    rate_window_started: Instant,
    rate_window_packets: u32,
    rate_window_bytes: usize,
    probe_window_started: Instant,
    probe_window_frames: u32,
}

#[derive(Debug)]
struct Room {
    metric_id: u64,
    peers: HashMap<u64, Peer>,
    last_seen: Instant,
}

#[derive(Debug)]
pub(crate) struct RelayStateV2 {
    config: RelayConfig,
    pending: HashMap<PeerId, PendingJoin>,
    rooms: HashMap<SessionKey, Room>,
    connection_rooms: HashMap<u64, SessionKey>,
    control_connections: HashMap<PeerId, u64>,
    next_metric_id: u64,
}

impl RelayStateV2 {
    pub(crate) fn new(config: RelayConfig) -> Self {
        Self {
            config,
            pending: HashMap::new(),
            rooms: HashMap::new(),
            connection_rooms: HashMap::new(),
            control_connections: HashMap::new(),
            next_metric_id: 1,
        }
    }

    pub(crate) fn begin_join(&mut self, begin: JoinBegin) -> Result<JoinChallenge, StateError> {
        self.validate_room_capacity(begin.session, begin.steam_id64)?;
        let challenge = JoinChallenge {
            challenge_id: random_bytes(),
            pow_nonce: random_bytes(),
            difficulty_bits: self.config.pow_difficulty_bits,
        };
        self.pending.insert(
            begin.control_peer,
            PendingJoin {
                session: begin.session,
                steam_id64: begin.steam_id64,
                display_name: begin.display_name,
                profile: begin.profile,
                capabilities: begin.capabilities,
                challenge_id: challenge.challenge_id,
                pow_nonce: challenge.pow_nonce,
                issued_at: begin.now,
            },
        );
        Ok(challenge)
    }

    pub(crate) fn complete_join(
        &mut self,
        control_peer: PeerId,
        challenge_id: [u8; 16],
        proof: &str,
        now: Instant,
    ) -> Result<(JoinReady, Option<PresenceBroadcast>), StateError> {
        let pending = self
            .pending
            .remove(&control_peer)
            .ok_or(StateError::MissingChallenge)?;
        if pending.challenge_id != challenge_id {
            return Err(StateError::InvalidChallenge);
        }
        if now.duration_since(pending.issued_at) > Duration::from_secs(30) {
            return Err(StateError::InvalidChallenge);
        }
        if !verify_pow(&pending, proof, self.config.pow_difficulty_bits) {
            return Err(StateError::InvalidProof);
        }
        self.validate_room_capacity(pending.session, pending.steam_id64)?;
        self.remove_duplicate(pending.session, pending.steam_id64);

        let connection_id = random_nonzero_u64(&self.connection_rooms);
        let resume_key = ResumeKey(random_bytes());
        let path_key = (pending.profile == DataProfile::Udp).then(|| PathKey(random_bytes()));
        let metric_id = self.next_metric_id;
        let room = self.rooms.entry(pending.session).or_insert_with(|| Room {
            metric_id,
            peers: HashMap::new(),
            last_seen: now,
        });
        if room.metric_id == metric_id {
            self.next_metric_id = self.next_metric_id.saturating_add(1);
        }
        room.last_seen = now;
        room.peers.insert(
            connection_id,
            Peer {
                control_peer,
                connection_id,
                resume_key,
                path_key,
                udp_address: None,
                steam_id64: pending.steam_id64,
                display_name: pending.display_name,
                profile: pending.profile,
                capabilities: pending.capabilities,
                presence: Presence::Connected,
                detached_until: None,
                frames: FrameIdWindow::new(),
                last_seen: now,
                rate_window_started: now,
                rate_window_packets: 0,
                rate_window_bytes: 0,
                probe_window_started: now,
                probe_window_frames: 0,
            },
        );
        self.connection_rooms.insert(connection_id, pending.session);
        self.control_connections.insert(control_peer, connection_id);
        let ready = JoinReady {
            connection_id,
            resume_key,
            peers: room_views(room),
        };
        let broadcast = self.broadcast(pending.session, Some(connection_id));
        Ok((ready, broadcast))
    }

    pub(crate) fn resume(
        &mut self,
        control_peer: PeerId,
        connection_id: u64,
        resume_key: ResumeKey,
        now: Instant,
    ) -> Result<ResumeReady, ResumeFailure> {
        let session = *self
            .connection_rooms
            .get(&connection_id)
            .ok_or(ResumeFailure::UnknownConnection)?;
        let room = self
            .rooms
            .get_mut(&session)
            .ok_or(ResumeFailure::UnknownConnection)?;
        let peer = room
            .peers
            .get_mut(&connection_id)
            .ok_or(ResumeFailure::UnknownConnection)?;
        if peer.resume_key != resume_key {
            return Err(ResumeFailure::InvalidCredential);
        }
        if peer.detached_until.is_some_and(|deadline| now > deadline) {
            return Err(ResumeFailure::Expired);
        }
        peer.control_peer = control_peer;
        peer.presence = Presence::Connected;
        peer.detached_until = None;
        peer.last_seen = now;
        room.last_seen = now;
        let udp_path_valid = peer.profile == DataProfile::Udp && peer.udp_address.is_some();
        self.control_connections.insert(control_peer, connection_id);
        let peers = room_views(room);
        let broadcast = self.broadcast(session, Some(connection_id));
        Ok(ResumeReady {
            connection_id,
            peers,
            udp_path_valid,
            broadcast,
        })
    }

    pub(crate) fn detach(
        &mut self,
        control_peer: PeerId,
        now: Instant,
    ) -> Option<PresenceBroadcast> {
        let connection_id = self.control_connections.remove(&control_peer)?;
        let session = *self.connection_rooms.get(&connection_id)?;
        let room = self.rooms.get_mut(&session)?;
        let peer = room.peers.get_mut(&connection_id)?;
        if peer.control_peer != control_peer || peer.presence == Presence::Reconnecting {
            return None;
        }
        peer.presence = Presence::Reconnecting;
        peer.detached_until = Some(now + RECOVERY_GRACE);
        self.broadcast(session, Some(connection_id))
    }

    pub(crate) fn stop(&mut self, control_peer: PeerId) -> Option<PresenceBroadcast> {
        let connection_id = self.control_connections.remove(&control_peer)?;
        self.remove_connection(connection_id)
    }

    pub(crate) fn bind_udp_path(
        &mut self,
        connection_id: u64,
        path_key: PathKey,
        address: SocketAddr,
        now: Instant,
    ) -> Result<PeerId, StateError> {
        let peer = self.peer_mut(connection_id)?;
        if peer.profile != DataProfile::Udp {
            return Err(StateError::ProfileMismatch);
        }
        if peer.path_key != Some(path_key) {
            return Err(StateError::PathNotValidated);
        }
        peer.udp_address = Some(address);
        peer.last_seen = now;
        Ok(peer.control_peer)
    }

    pub(crate) fn route_data(&mut self, request: RouteData) -> Result<DataDestination, StateError> {
        let RouteData {
            connection_id,
            frame_id,
            from_steam_id64,
            to_steam_id64,
            source,
            frame_bytes,
            now,
        } = request;
        let session = *self
            .connection_rooms
            .get(&connection_id)
            .ok_or(StateError::UnknownConnection)?;
        let room = self
            .rooms
            .get_mut(&session)
            .ok_or(StateError::UnknownConnection)?;
        {
            let sender = room
                .peers
                .get_mut(&connection_id)
                .ok_or(StateError::UnknownConnection)?;
            validate_source(sender, from_steam_id64, source)?;
            match sender.frames.observe(frame_id) {
                DuplicateDecision::New | DuplicateDecision::Reordered => {}
                DuplicateDecision::Duplicate => return Err(StateError::DuplicateFrame),
                DuplicateDecision::TooOld => return Err(StateError::FrameTooOld),
            }
            apply_traffic_budget(sender, &self.config, frame_bytes, now)?;
        }
        room.last_seen = now;
        target_destination(room, to_steam_id64, false)
    }

    pub(crate) fn route_probe(
        &mut self,
        request: RouteProbe,
    ) -> Result<DataDestination, StateError> {
        let RouteProbe {
            connection_id,
            from_steam_id64,
            to_steam_id64,
            source,
            frame_bytes,
            now,
        } = request;
        let session = *self
            .connection_rooms
            .get(&connection_id)
            .ok_or(StateError::UnknownConnection)?;
        let room = self
            .rooms
            .get_mut(&session)
            .ok_or(StateError::UnknownConnection)?;
        {
            let sender = room
                .peers
                .get_mut(&connection_id)
                .ok_or(StateError::UnknownConnection)?;
            validate_source(sender, from_steam_id64, source)?;
            if sender.capabilities & CAP_ROOM_PATH_PROBE == 0 {
                return Err(StateError::ProbeUnsupported);
            }
            apply_traffic_budget(sender, &self.config, frame_bytes, now)?;
            if now.duration_since(sender.probe_window_started) >= Duration::from_secs(1) {
                sender.probe_window_started = now;
                sender.probe_window_frames = 0;
            }
            let next = sender.probe_window_frames.saturating_add(1);
            if next > PROBE_RATE_LIMIT_PER_SECOND {
                return Err(StateError::ProbeRateLimited);
            }
            sender.probe_window_frames = next;
        }
        room.last_seen = now;
        target_destination(room, to_steam_id64, true)
    }

    pub(crate) fn cleanup(&mut self, now: Instant) -> Vec<PresenceBroadcast> {
        self.pending
            .retain(|_, pending| now.duration_since(pending.issued_at) < Duration::from_secs(30));
        let expired = self
            .rooms
            .values()
            .flat_map(|room| room.peers.values())
            .filter(|peer| peer.detached_until.is_some_and(|deadline| now >= deadline))
            .map(|peer| peer.connection_id)
            .collect::<Vec<_>>();
        let mut broadcasts = Vec::new();
        for connection_id in expired {
            if let Some(broadcast) = self.remove_connection(connection_id) {
                broadcasts.push(broadcast);
            }
        }
        self.rooms.retain(|_, room| {
            !room.peers.is_empty()
                || now.duration_since(room.last_seen)
                    < Duration::from_secs(self.config.room_idle_seconds)
        });
        broadcasts
    }

    #[cfg(test)]
    pub(crate) fn control_peer(&self, connection_id: u64) -> Option<PeerId> {
        let session = self.connection_rooms.get(&connection_id)?;
        self.rooms
            .get(session)?
            .peers
            .get(&connection_id)
            .map(|peer| peer.control_peer)
    }

    pub(crate) fn connection_for_control(&self, control_peer: PeerId) -> Option<u64> {
        self.control_connections.get(&control_peer).copied()
    }

    pub(crate) fn path_key(&self, connection_id: u64) -> Option<PathKey> {
        let session = self.connection_rooms.get(&connection_id)?;
        self.rooms.get(session)?.peers.get(&connection_id)?.path_key
    }

    pub(crate) fn active_counts(&self) -> (usize, usize) {
        (
            self.rooms.len(),
            self.rooms.values().map(|room| room.peers.len()).sum(),
        )
    }

    #[cfg(test)]
    pub(crate) fn room_count(&self) -> usize {
        self.rooms.len()
    }
    #[cfg(test)]
    pub(crate) fn peer_count(&self) -> usize {
        self.rooms.values().map(|room| room.peers.len()).sum()
    }

    fn validate_room_capacity(
        &self,
        session: SessionKey,
        steam_id64: u64,
    ) -> Result<(), StateError> {
        if let Some(room) = self.rooms.get(&session) {
            let already = room
                .peers
                .values()
                .any(|peer| peer.steam_id64 == steam_id64);
            if !already && room.peers.len() >= self.config.max_peers_per_room {
                return Err(StateError::RoomFull);
            }
        } else if self.rooms.len() >= self.config.max_rooms {
            return Err(StateError::RelayFull);
        }
        Ok(())
    }

    fn remove_duplicate(&mut self, session: SessionKey, steam_id64: u64) {
        let duplicate = self.rooms.get(&session).and_then(|room| {
            room.peers
                .values()
                .find(|peer| peer.steam_id64 == steam_id64)
                .map(|peer| peer.connection_id)
        });
        if let Some(connection_id) = duplicate {
            let _ = self.remove_connection(connection_id);
        }
    }

    fn remove_connection(&mut self, connection_id: u64) -> Option<PresenceBroadcast> {
        let session = self.connection_rooms.remove(&connection_id)?;
        let room = self.rooms.get_mut(&session)?;
        let peer = room.peers.remove(&connection_id)?;
        self.control_connections.remove(&peer.control_peer);
        self.broadcast(session, None)
    }

    fn peer_mut(&mut self, connection_id: u64) -> Result<&mut Peer, StateError> {
        let session = *self
            .connection_rooms
            .get(&connection_id)
            .ok_or(StateError::UnknownConnection)?;
        self.rooms
            .get_mut(&session)
            .and_then(|room| room.peers.get_mut(&connection_id))
            .ok_or(StateError::UnknownConnection)
    }

    fn broadcast(&self, session: SessionKey, exclude: Option<u64>) -> Option<PresenceBroadcast> {
        let room = self.rooms.get(&session)?;
        let recipients = room
            .peers
            .iter()
            .filter_map(|(connection_id, peer)| {
                (Some(*connection_id) != exclude && peer.presence == Presence::Connected)
                    .then_some(peer.control_peer)
            })
            .collect::<Vec<_>>();
        (!recipients.is_empty()).then(|| PresenceBroadcast {
            recipients,
            peers: room_views(room),
        })
    }
}

fn room_views(room: &Room) -> Vec<PeerView> {
    let mut peers = room
        .peers
        .values()
        .map(|peer| PeerView {
            steam_id64: peer.steam_id64,
            display_name: peer.display_name.clone(),
            presence: peer.presence,
            capabilities: peer.capabilities,
        })
        .collect::<Vec<_>>();
    peers.sort_by_key(|peer| peer.steam_id64);
    peers
}

fn validate_source(
    sender: &Peer,
    from_steam_id64: u64,
    source: DataSource,
) -> Result<(), StateError> {
    if sender.steam_id64 != from_steam_id64 {
        return Err(StateError::SenderMismatch);
    }
    match (sender.profile, source) {
        (DataProfile::Tcp, DataSource::Tcp(peer)) if peer == sender.control_peer => Ok(()),
        (DataProfile::Udp, DataSource::Udp(address)) if Some(address) == sender.udp_address => {
            Ok(())
        }
        (DataProfile::Udp, DataSource::Udp(_)) => Err(StateError::PathNotValidated),
        _ => Err(StateError::ProfileMismatch),
    }
}

fn apply_traffic_budget(
    sender: &mut Peer,
    config: &RelayConfig,
    frame_bytes: usize,
    now: Instant,
) -> Result<(), StateError> {
    if now.duration_since(sender.rate_window_started) >= Duration::from_secs(1) {
        sender.rate_window_started = now;
        sender.rate_window_packets = 0;
        sender.rate_window_bytes = 0;
    }
    let next_packets = sender.rate_window_packets.saturating_add(1);
    let next_bytes = sender.rate_window_bytes.saturating_add(frame_bytes);
    if next_packets > config.rate_limit_per_second
        || next_bytes > config.byte_rate_limit_burst
        || (sender.rate_window_bytes > 0 && next_bytes > config.byte_rate_limit_per_second)
    {
        return Err(StateError::RateLimited);
    }
    sender.rate_window_packets = next_packets;
    sender.rate_window_bytes = next_bytes;
    sender.last_seen = now;
    Ok(())
}

fn target_destination(
    room: &Room,
    to_steam_id64: u64,
    require_probe: bool,
) -> Result<DataDestination, StateError> {
    let target = room
        .peers
        .values()
        .find(|peer| peer.steam_id64 == to_steam_id64)
        .ok_or(StateError::TargetNotJoined)?;
    if require_probe && target.capabilities & CAP_ROOM_PATH_PROBE == 0 {
        return Err(StateError::ProbeUnsupported);
    }
    match target.profile {
        DataProfile::Tcp if target.presence == Presence::Connected => {
            Ok(DataDestination::Tcp(target.control_peer))
        }
        DataProfile::Udp => target
            .udp_address
            .map(DataDestination::Udp)
            .ok_or(StateError::TargetUnavailable),
        DataProfile::Tcp => Err(StateError::TargetUnavailable),
    }
}

fn verify_pow(pending: &PendingJoin, proof: &str, difficulty: u8) -> bool {
    if difficulty == 0 {
        return proof.is_empty();
    }
    let mut hasher = Sha256::new();
    hasher.update(pending.challenge_id);
    hasher.update(pending.session.0);
    hasher.update(pending.steam_id64.to_be_bytes());
    hasher.update(pending.pow_nonce);
    hasher.update(proof.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    leading_zero_bits(&digest, difficulty)
}

fn leading_zero_bits(bytes: &[u8; 32], bits: u8) -> bool {
    let whole = usize::from(bits / 8);
    let rest = bits % 8;
    whole <= bytes.len()
        && bytes[..whole].iter().all(|byte| *byte == 0)
        && (rest == 0 || bytes.get(whole).is_some_and(|byte| byte >> (8 - rest) == 0))
}

fn random_bytes() -> [u8; 16] {
    rand::rng().random()
}

fn random_nonzero_u64(existing: &HashMap<u64, SessionKey>) -> u64 {
    loop {
        let value: u64 = rand::rng().random();
        if value != 0 && !existing.contains_key(&value) {
            return value;
        }
    }
}

#[cfg(test)]
#[path = "state_v2_tests.rs"]
mod tests;
