use std::{
    collections::HashMap,
    fmt::{self, Display},
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

use basement_bridge_core::protocol::{
    ClientMetadata, ControlMessage, PROTOCOL_MAJOR, PROTOCOL_MINOR, PowChallenge, PowProof,
};
use rand::RngExt as _;
use tracing::info;

use crate::{
    config::RelayConfig,
    incident::{MissingTargetIncident, MissingTargetLogBudget, RoomPeerSnapshot},
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PeerId(u64);

impl PeerId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl Display for PeerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "peer-{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeerTransport {
    Udp,
    Tcp,
}

impl Display for PeerTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Udp => formatter.write_str("udp"),
            Self::Tcp => formatter.write_str("tcp"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RoomSummary {
    pub(crate) name: String,
    pub(crate) peers: usize,
    pub(crate) tcp_peers: usize,
    pub(crate) udp_peers: usize,
}

#[derive(Clone, Debug)]
struct PendingJoin {
    room: String,
    steam_id64: String,
    display_name: Option<String>,
    admission: String,
    pow: Option<PowChallenge>,
    token: String,
    issued_at: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct JoinRequest {
    pub(crate) peer_id: PeerId,
    pub(crate) room: String,
    pub(crate) steam_id64: String,
    pub(crate) display_name: Option<String>,
    pub(crate) client: Option<ClientMetadata>,
    pub(crate) admission: Option<String>,
    pub(crate) now: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct JoinCompletion {
    pub(crate) peer_id: PeerId,
    pub(crate) room: String,
    pub(crate) steam_id64: String,
    pub(crate) client: Option<ClientMetadata>,
    pub(crate) challenge: String,
    pub(crate) pow_proof: Option<PowProof>,
    pub(crate) transport: PeerTransport,
    pub(crate) now: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct RoomBroadcast {
    pub(crate) recipients: Vec<PeerId>,
    pub(crate) peers: Vec<basement_bridge_core::protocol::PeerInfo>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CleanupOutcome {
    pub(crate) broadcasts: Vec<RoomBroadcast>,
    pub(crate) removed_peers: Vec<PeerId>,
}

#[derive(Clone, Debug)]
pub(crate) struct JoinOutcome {
    pub(crate) response: ControlMessage,
    pub(crate) broadcast: Option<RoomBroadcast>,
}

#[derive(Clone, Debug)]
struct Peer {
    steam_id64: String,
    display_name: Option<String>,
    transport: PeerTransport,
    last_seen: Instant,
}

#[derive(Debug, Default)]
struct Room {
    admission: String,
    peers: HashMap<PeerId, Peer>,
    last_seen: Option<Instant>,
}

impl Room {
    fn new(admission: String) -> Self {
        Self {
            admission,
            peers: HashMap::new(),
            last_seen: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RateWindow {
    started_at: Instant,
    packets: u32,
}

#[derive(Clone, Copy, Debug)]
struct ByteBucket {
    updated_at: Instant,
    tokens: usize,
}

impl ByteBucket {
    fn full(now: Instant, capacity: usize) -> Self {
        Self {
            updated_at: now,
            tokens: capacity,
        }
    }

    fn refill(&mut self, now: Instant, refill_per_second: usize, capacity: usize) {
        let elapsed = now.duration_since(self.updated_at);
        let refill = (elapsed.as_secs_f64() * refill_per_second as f64) as usize;
        if refill > 0 {
            self.tokens = self.tokens.saturating_add(refill).min(capacity);
            self.updated_at = now;
        }
    }

    fn can_spend(&self, bytes: usize) -> bool {
        bytes <= self.tokens
    }

    fn spend(&mut self, bytes: usize) {
        self.tokens -= bytes;
    }
}

#[derive(Clone, Copy, Debug)]
struct PeerRateLimit {
    last_seen: Instant,
    packet_window: RateWindow,
    byte_bucket: ByteBucket,
}

impl PeerRateLimit {
    fn new(now: Instant, byte_burst: usize) -> Self {
        Self {
            last_seen: now,
            packet_window: RateWindow {
                started_at: now,
                packets: 0,
            },
            byte_bucket: ByteBucket::full(now, byte_burst),
        }
    }
}

#[derive(Debug)]
pub(crate) struct RelayState {
    config: RelayConfig,
    pending: HashMap<PeerId, PendingJoin>,
    rooms: HashMap<String, Room>,
    peer_rooms: HashMap<PeerId, String>,
    rates: HashMap<PeerId, PeerRateLimit>,
    health_pong_rates: HashMap<IpAddr, RateWindow>,
    missing_target_logs: HashMap<String, MissingTargetLogBudget>,
}

impl RelayState {
    pub(crate) fn new(config: RelayConfig) -> Self {
        Self {
            config,
            pending: HashMap::new(),
            rooms: HashMap::new(),
            peer_rooms: HashMap::new(),
            rates: HashMap::new(),
            health_pong_rates: HashMap::new(),
            missing_target_logs: HashMap::new(),
        }
    }

    pub(crate) fn allow_packet(&mut self, peer_id: PeerId, bytes: usize, now: Instant) -> bool {
        let byte_burst = self.config.byte_rate_limit_burst;
        let rate_limit = self
            .rates
            .entry(peer_id)
            .or_insert_with(|| PeerRateLimit::new(now, byte_burst));
        if now.duration_since(rate_limit.packet_window.started_at) >= Duration::from_secs(1) {
            rate_limit.packet_window.started_at = now;
            rate_limit.packet_window.packets = 0;
        }
        rate_limit.byte_bucket.refill(
            now,
            self.config.byte_rate_limit_per_second,
            self.config.byte_rate_limit_burst,
        );
        rate_limit.last_seen = now;
        if !rate_limit.byte_bucket.can_spend(bytes) {
            return false;
        }
        if rate_limit.packet_window.packets >= self.config.rate_limit_per_second {
            return false;
        }
        rate_limit.packet_window.packets = rate_limit.packet_window.packets.saturating_add(1);
        rate_limit.byte_bucket.spend(bytes);
        true
    }

    pub(crate) fn allow_health_pong(&mut self, source: IpAddr, now: Instant) -> bool {
        let window = self.health_pong_rates.entry(source).or_insert(RateWindow {
            started_at: now,
            packets: 0,
        });
        if now.duration_since(window.started_at) >= Duration::from_secs(1) {
            window.started_at = now;
            window.packets = 0;
        }
        window.packets = window.packets.saturating_add(1);
        window.packets <= self.config.health_pongs_per_second_per_ip
    }

    pub(crate) fn is_blocked(&self, address: SocketAddr) -> bool {
        self.config
            .blocked_cidrs
            .iter()
            .any(|network| network.contains(&address.ip()))
    }

    pub(crate) fn challenge_join(&mut self, request: JoinRequest) -> ControlMessage {
        let JoinRequest {
            peer_id,
            room,
            steam_id64,
            display_name,
            client,
            admission,
            now,
        } = request;
        if let Err(error) = validate_client_metadata(client.as_ref()) {
            return *error;
        }
        let Some(admission) = admission.filter(|value| !value.is_empty()) else {
            return error_message("admission_required", "room admission material is required");
        };
        if let Err(error) = self.validate_join(peer_id, &room, &admission) {
            return *error;
        }

        let token = join_token();
        let pow = (self.config.pow_difficulty_bits > 0)
            .then(|| PowChallenge::sha256(join_token(), self.config.pow_difficulty_bits));
        self.pending.insert(
            peer_id,
            PendingJoin {
                room,
                steam_id64,
                display_name,
                admission,
                pow: pow.clone(),
                token: token.clone(),
                issued_at: now,
            },
        );
        ControlMessage::Challenge { token, pow }
    }

    pub(crate) fn complete_join(&mut self, completion: JoinCompletion) -> JoinOutcome {
        let JoinCompletion {
            peer_id,
            room,
            steam_id64,
            client,
            challenge,
            pow_proof,
            transport,
            now,
        } = completion;
        if let Err(error) = validate_client_metadata(client.as_ref()) {
            return JoinOutcome {
                response: *error,
                broadcast: None,
            };
        }
        let Some(pending) = self.pending.remove(&peer_id) else {
            return JoinOutcome {
                response: error_message("missing_challenge", "join challenge was not issued"),
                broadcast: None,
            };
        };
        if pending.room != room || pending.steam_id64 != steam_id64 || pending.token != challenge {
            return JoinOutcome {
                response: error_message("bad_challenge", "join challenge did not match"),
                broadcast: None,
            };
        }
        if let Err(error) = validate_pow(&pending, pow_proof.as_ref()) {
            return JoinOutcome {
                response: *error,
                broadcast: None,
            };
        }
        self.remove_duplicate_peer(&pending.room, &pending.steam_id64, peer_id);
        if let Err(error) = self.validate_join(peer_id, &pending.room, &pending.admission) {
            return JoinOutcome {
                response: *error,
                broadcast: None,
            };
        }

        self.peer_rooms.insert(peer_id, pending.room.clone());
        let room = self
            .rooms
            .entry(pending.room.clone())
            .or_insert_with(|| Room::new(pending.admission));
        room.last_seen = Some(now);
        room.peers.insert(
            peer_id,
            Peer {
                steam_id64: pending.steam_id64,
                display_name: pending.display_name,
                transport,
                last_seen: now,
            },
        );
        info!(
            %peer_id,
            room = %pending.room,
            %transport,
            peers = room.peers.len(),
            "peer joined"
        );
        let response = ControlMessage::Ready {
            peers: self.room_peer_infos(&pending.room),
        };
        let broadcast = self.room_broadcast(&pending.room, Some(peer_id));
        JoinOutcome {
            response,
            broadcast,
        }
    }

    pub(crate) fn peer_room(&self, peer_id: PeerId) -> Option<String> {
        self.peer_rooms.get(&peer_id).cloned()
    }

    pub(crate) fn touch_peer(&mut self, peer_id: PeerId, now: Instant) -> Option<String> {
        let room_name = self.peer_rooms.get(&peer_id)?.clone();
        let room = self.rooms.get_mut(&room_name)?;
        room.last_seen = Some(now);
        let peer = room.peers.get_mut(&peer_id)?;
        peer.last_seen = now;
        Some(room_name)
    }

    pub(crate) fn target_peer(&self, room_name: &str, steam_id64: u64) -> Option<PeerId> {
        let target = steam_id64.to_string();
        self.rooms.get(room_name).and_then(|room| {
            room.peers
                .iter()
                .find_map(|(peer_id, peer)| (peer.steam_id64 == target).then_some(*peer_id))
        })
    }

    pub(crate) fn record_missing_target_incident(
        &mut self,
        room_name: &str,
        now: Instant,
    ) -> Option<MissingTargetIncident> {
        if !self
            .missing_target_logs
            .entry(room_name.to_owned())
            .or_default()
            .should_log(now)
        {
            return None;
        }
        Some(MissingTargetIncident {
            peers: self.room_peer_snapshots(room_name),
        })
    }

    fn room_peer_snapshots(&self, room_name: &str) -> Vec<RoomPeerSnapshot> {
        let mut peers = self
            .rooms
            .get(room_name)
            .map(|room| {
                room.peers
                    .iter()
                    .map(|(peer_id, peer)| RoomPeerSnapshot {
                        peer_id: *peer_id,
                        steam_id64: peer.steam_id64.clone(),
                        transport: peer.transport,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        peers.sort_by(|left, right| {
            left.steam_id64
                .cmp(&right.steam_id64)
                .then_with(|| left.peer_id.0.cmp(&right.peer_id.0))
        });
        peers
    }

    pub(crate) fn room_peer_infos(
        &self,
        room_name: &str,
    ) -> Vec<basement_bridge_core::protocol::PeerInfo> {
        let mut peers = self
            .rooms
            .get(room_name)
            .map(|room| {
                room.peers
                    .values()
                    .map(|peer| basement_bridge_core::protocol::PeerInfo {
                        steam_id64: peer.steam_id64.clone(),
                        display_name: peer.display_name.clone(),
                        transport: match peer.transport {
                            PeerTransport::Udp => {
                                basement_bridge_core::protocol::PeerTransport::Udp
                            }
                            PeerTransport::Tcp => {
                                basement_bridge_core::protocol::PeerTransport::Tcp
                            }
                        },
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        peers.sort_by(|left, right| left.steam_id64.cmp(&right.steam_id64));
        peers
    }

    fn room_broadcast(&self, room_name: &str, exclude: Option<PeerId>) -> Option<RoomBroadcast> {
        let room = self.rooms.get(room_name)?;
        let peers = self.room_peer_infos(room_name);
        if peers.is_empty() {
            return None;
        }
        let recipients: Vec<PeerId> = room
            .peers
            .keys()
            .copied()
            .filter(|peer_id| Some(*peer_id) != exclude)
            .collect();
        if recipients.is_empty() {
            return None;
        }
        Some(RoomBroadcast { recipients, peers })
    }

    #[cfg(test)]
    pub(crate) fn peer_ids(&self, room_name: &str) -> Vec<PeerId> {
        self.rooms
            .get(room_name)
            .map(|room| room.peers.keys().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn room_count(&self) -> usize {
        self.rooms.len()
    }

    pub(crate) fn peer_count(&self) -> usize {
        self.rooms.values().map(|room| room.peers.len()).sum()
    }

    pub(crate) fn room_summaries(&self) -> Vec<RoomSummary> {
        let mut summaries = self
            .rooms
            .iter()
            .map(|(name, room)| {
                let tcp_peers = room
                    .peers
                    .values()
                    .filter(|peer| peer.transport == PeerTransport::Tcp)
                    .count();
                let peers = room.peers.len();
                RoomSummary {
                    name: name.clone(),
                    peers,
                    tcp_peers,
                    udp_peers: peers.saturating_sub(tcp_peers),
                }
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| left.name.cmp(&right.name));
        summaries
    }

    pub(crate) fn cleanup(&mut self, now: Instant) -> CleanupOutcome {
        let peer_idle = Duration::from_secs(self.config.peer_idle_seconds);
        let room_idle = Duration::from_secs(self.config.room_idle_seconds);
        let mut removed_peers = Vec::new();
        self.pending.retain(|peer_id, pending| {
            let active = now.duration_since(pending.issued_at) < peer_idle;
            if !active {
                removed_peers.push(*peer_id);
            }
            active
        });
        self.health_pong_rates
            .retain(|_, window| now.duration_since(window.started_at) < Duration::from_secs(60));

        let mut changed_rooms: std::collections::HashSet<String> = std::collections::HashSet::new();
        self.rooms.retain(|room_name, room| {
            room.peers.retain(|peer_id, peer| {
                let active = now.duration_since(peer.last_seen) < peer_idle;
                if !active {
                    removed_peers.push(*peer_id);
                    changed_rooms.insert(room_name.clone());
                    info!(
                        %peer_id,
                        room = %room_name,
                        steam_id64 = %peer.steam_id64,
                        display_name = peer.display_name.as_deref().unwrap_or(""),
                        transport = %peer.transport,
                        "peer expired"
                    );
                }
                active
            });
            !room.peers.is_empty()
                || room
                    .last_seen
                    .is_some_and(|seen| now.duration_since(seen) < room_idle)
        });
        for peer_id in &removed_peers {
            self.peer_rooms.remove(peer_id);
            self.rates.remove(peer_id);
        }
        let stale_rate_peers = self
            .rates
            .iter()
            .filter_map(|(peer_id, rate)| {
                (!self.peer_rooms.contains_key(peer_id)
                    && !self.pending.contains_key(peer_id)
                    && now.duration_since(rate.last_seen) >= peer_idle)
                    .then_some(*peer_id)
            })
            .collect::<Vec<_>>();
        for peer_id in stale_rate_peers {
            self.rates.remove(&peer_id);
            removed_peers.push(peer_id);
        }
        let broadcasts = changed_rooms
            .iter()
            .filter_map(|room_name| self.room_broadcast(room_name, None))
            .collect();
        CleanupOutcome {
            broadcasts,
            removed_peers,
        }
    }

    pub(crate) fn remove_peer(&mut self, peer_id: PeerId) -> Option<RoomBroadcast> {
        let Some(room_name) = self.peer_rooms.remove(&peer_id) else {
            self.pending.remove(&peer_id);
            self.rates.remove(&peer_id);
            return None;
        };
        self.pending.remove(&peer_id);
        self.rates.remove(&peer_id);
        if let Some(room) = self.rooms.get_mut(&room_name)
            && let Some(peer) = room.peers.remove(&peer_id)
        {
            info!(
                %peer_id,
                room = %room_name,
                steam_id64 = %peer.steam_id64,
                display_name = peer.display_name.as_deref().unwrap_or(""),
                transport = %peer.transport,
                peers = room.peers.len(),
                "peer disconnected"
            );
        }
        self.room_broadcast(&room_name, Some(peer_id))
    }

    fn validate_join(
        &self,
        peer_id: PeerId,
        room_name: &str,
        admission: &str,
    ) -> Result<(), Box<ControlMessage>> {
        let room_name = room_name.trim();
        if room_name.is_empty() {
            return Err(Box::new(error_message("empty_room", "room is required")));
        }
        if room_name.len() > self.config.max_room_name_len {
            return Err(Box::new(error_message(
                "room_name_too_long",
                format!(
                    "room must be at most {} bytes",
                    self.config.max_room_name_len
                ),
            )));
        }

        let already_joined = self
            .peer_rooms
            .get(&peer_id)
            .is_some_and(|current| current == room_name);

        if let Some(room) = self.rooms.get(room_name) {
            if room.admission != admission {
                return Err(Box::new(error_message(
                    "room_admission_mismatch",
                    "room admission material did not match",
                )));
            }
            if !already_joined
                && !room.peers.contains_key(&peer_id)
                && room.peers.len() >= self.config.max_peers_per_room
            {
                return Err(Box::new(error_message("room_full", "room is full")));
            }
            return Ok(());
        }

        if !already_joined && self.rooms.len() >= self.config.max_rooms {
            return Err(Box::new(error_message(
                "too_many_rooms",
                "relay room limit reached",
            )));
        }

        Ok(())
    }

    fn remove_duplicate_peer(&mut self, room_name: &str, steam_id64: &str, peer_id: PeerId) {
        let Some(room) = self.rooms.get_mut(room_name) else {
            return;
        };

        let duplicate_peers = room
            .peers
            .iter()
            .filter_map(|(existing_peer_id, peer)| {
                (peer.steam_id64 == steam_id64 && *existing_peer_id != peer_id)
                    .then_some(*existing_peer_id)
            })
            .collect::<Vec<_>>();

        for duplicate_peer_id in duplicate_peers {
            let Some(peer) = room.peers.remove(&duplicate_peer_id) else {
                continue;
            };
            self.peer_rooms.remove(&duplicate_peer_id);
            self.rates.remove(&duplicate_peer_id);
            info!(
                %duplicate_peer_id,
                replacement = %peer_id,
                room = %room_name,
                steam_id64 = %peer.steam_id64,
                display_name = peer.display_name.as_deref().unwrap_or(""),
                transport = %peer.transport,
                "duplicate peer replaced"
            );
        }
    }
}

pub(crate) fn error_message(code: impl Into<String>, message: impl Into<String>) -> ControlMessage {
    ControlMessage::Error {
        code: code.into(),
        message: message.into(),
    }
}

fn validate_pow(
    pending: &PendingJoin,
    proof: Option<&PowProof>,
) -> Result<(), Box<ControlMessage>> {
    let Some(challenge) = &pending.pow else {
        if proof.is_some() {
            return Err(Box::new(error_message(
                "pow_unexpected",
                "proof of work was not requested for room admission",
            )));
        }
        return Ok(());
    };
    let Some(proof) = proof else {
        return Err(Box::new(error_message(
            "pow_required",
            "proof of work is required for room admission",
        )));
    };
    if !proof.verify(
        challenge,
        &pending.token,
        &pending.room,
        &pending.steam_id64,
    ) {
        return Err(Box::new(error_message(
            "pow_failed",
            "proof of work did not satisfy the challenge",
        )));
    }
    Ok(())
}

fn validate_client_metadata(client: Option<&ClientMetadata>) -> Result<(), Box<ControlMessage>> {
    let Some(client) = client else {
        return Err(Box::new(error_message(
            "client_metadata_required",
            "client metadata is required for room admission",
        )));
    };
    if client.protocol_major != PROTOCOL_MAJOR || client.protocol_minor != PROTOCOL_MINOR {
        return Err(Box::new(error_message(
            "unsupported_protocol",
            format!(
                "client protocol {}.{} is not supported by relay protocol {}.{}",
                client.protocol_major, client.protocol_minor, PROTOCOL_MAJOR, PROTOCOL_MINOR
            ),
        )));
    }
    Ok(())
}

fn join_token() -> String {
    let value: u128 = rand::rng().random();
    format!("{value:032x}")
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
