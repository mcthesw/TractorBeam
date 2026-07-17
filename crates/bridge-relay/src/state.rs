use std::{
    collections::HashMap,
    net::SocketAddr,
    time::{Duration, Instant},
};

use tractor_beam_relay_protocol::{CAP_ROOM_PATH_PROBE, DuplicateDecision, FrameIdWindow};

use crate::{
    config::RelayConfig,
    domain::PeerId,
    domain::{
        DataDestination, DataProfile, JoinBegin, JoinChallenge, JoinReady, PathKey, Presence,
        PresenceBroadcast, ResumeFailure, ResumeKey, ResumeReady, RouteData, RouteProbe,
        SessionKey, StateError,
    },
};

mod routing;
mod traffic_budget;

use routing::{
    random_bytes, random_nonzero_u64, room_views, target_destination, validate_source, verify_pow,
};
use traffic_budget::TrafficBudget;

const RECOVERY_GRACE: Duration = Duration::from_secs(120);

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
    traffic_budget: TrafficBudget,
}

#[derive(Debug)]
struct Room {
    peers: HashMap<u64, Peer>,
    last_seen: Instant,
}

#[derive(Debug)]
pub(crate) struct RelayState {
    config: RelayConfig,
    pending: HashMap<PeerId, PendingJoin>,
    rooms: HashMap<SessionKey, Room>,
    connection_rooms: HashMap<u64, SessionKey>,
    control_connections: HashMap<PeerId, u64>,
}

impl RelayState {
    pub(crate) fn new(config: RelayConfig) -> Self {
        Self {
            config,
            pending: HashMap::new(),
            rooms: HashMap::new(),
            connection_rooms: HashMap::new(),
            control_connections: HashMap::new(),
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
        let room = self.rooms.entry(pending.session).or_insert_with(|| Room {
            peers: HashMap::new(),
            last_seen: now,
        });
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
                traffic_budget: TrafficBudget::new(&self.config, now),
            },
        );
        self.connection_rooms.insert(connection_id, pending.session);
        self.control_connections.insert(control_peer, connection_id);
        let ready = JoinReady {
            connection_id,
            resume_key,
            peers: room_views(room),
            profile: pending.profile,
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
        let profile = peer.profile;
        let udp_path_valid = peer.profile == DataProfile::Udp && peer.udp_address.is_some();
        self.control_connections.insert(control_peer, connection_id);
        let peers = room_views(room);
        let broadcast = self.broadcast(session, Some(connection_id));
        Ok(ResumeReady {
            connection_id,
            peers,
            profile,
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
            sender.traffic_budget.check_traffic(frame_bytes, now)?;
            sender.last_seen = now;
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
            sender.traffic_budget.check_traffic(frame_bytes, now)?;
            sender.traffic_budget.check_probe(now)?;
            sender.last_seen = now;
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

    pub(crate) fn active_peer_counts(&self) -> [usize; 4] {
        let mut counts = [0; 4];
        for peer in self.rooms.values().flat_map(|room| room.peers.values()) {
            let profile_offset = match peer.profile {
                DataProfile::Tcp => 0,
                DataProfile::Udp => 2,
            };
            let presence_offset = match peer.presence {
                Presence::Connected => 0,
                Presence::Reconnecting => 1,
            };
            counts[profile_offset + presence_offset] += 1;
        }
        counts
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

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
