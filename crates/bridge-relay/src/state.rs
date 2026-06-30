use std::{
    collections::HashMap,
    fmt::{self, Display},
    net::SocketAddr,
    time::{Duration, Instant},
};

use basement_bridge_core::protocol::ControlMessage;
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
    token: String,
    issued_at: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct JoinCompletion {
    pub(crate) peer_id: PeerId,
    pub(crate) room: String,
    pub(crate) steam_id64: String,
    pub(crate) challenge: String,
    pub(crate) transport: PeerTransport,
    pub(crate) now: Instant,
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
    peers: HashMap<PeerId, Peer>,
    last_seen: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
struct RateWindow {
    started_at: Instant,
    packets: u32,
}

#[derive(Debug)]
pub(crate) struct RelayState {
    config: RelayConfig,
    pending: HashMap<PeerId, PendingJoin>,
    rooms: HashMap<String, Room>,
    peer_rooms: HashMap<PeerId, String>,
    rates: HashMap<PeerId, RateWindow>,
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
            missing_target_logs: HashMap::new(),
        }
    }

    pub(crate) fn allow_packet(&mut self, peer_id: PeerId, now: Instant) -> bool {
        let limit = self.config.rate_limit_per_second;
        let window = self.rates.entry(peer_id).or_insert(RateWindow {
            started_at: now,
            packets: 0,
        });
        if now.duration_since(window.started_at) >= Duration::from_secs(1) {
            window.started_at = now;
            window.packets = 0;
        }
        window.packets = window.packets.saturating_add(1);
        window.packets <= limit
    }

    pub(crate) fn is_blocked(&self, address: SocketAddr) -> bool {
        self.config
            .blocked_cidrs
            .iter()
            .any(|network| network.contains(&address.ip()))
    }

    pub(crate) fn challenge_join(
        &mut self,
        peer_id: PeerId,
        room: String,
        steam_id64: String,
        display_name: Option<String>,
        now: Instant,
    ) -> ControlMessage {
        if let Err(error) = self.validate_join(peer_id, &room) {
            return error;
        }

        let token = join_token();
        self.pending.insert(
            peer_id,
            PendingJoin {
                room,
                steam_id64,
                display_name,
                token: token.clone(),
                issued_at: now,
            },
        );
        ControlMessage::Challenge { token }
    }

    pub(crate) fn complete_join(&mut self, completion: JoinCompletion) -> ControlMessage {
        let JoinCompletion {
            peer_id,
            room,
            steam_id64,
            challenge,
            transport,
            now,
        } = completion;
        let Some(pending) = self.pending.remove(&peer_id) else {
            return error_message("missing_challenge", "join challenge was not issued");
        };
        if pending.room != room || pending.steam_id64 != steam_id64 || pending.token != challenge {
            return error_message("bad_challenge", "join challenge did not match");
        }
        self.remove_duplicate_peer(&pending.room, &pending.steam_id64, peer_id);
        if let Err(error) = self.validate_join(peer_id, &pending.room) {
            return error;
        }

        self.peer_rooms.insert(peer_id, pending.room.clone());
        let room = self.rooms.entry(pending.room.clone()).or_default();
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
        ControlMessage::Ready {
            peer_count: room.peers.len(),
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

    pub(crate) fn cleanup(&mut self, now: Instant) {
        let peer_idle = Duration::from_secs(self.config.peer_idle_seconds);
        let room_idle = Duration::from_secs(self.config.room_idle_seconds);
        self.pending
            .retain(|_, pending| now.duration_since(pending.issued_at) < peer_idle);

        let mut removed_peers = Vec::new();
        self.rooms.retain(|room_name, room| {
            room.peers.retain(|peer_id, peer| {
                let active = now.duration_since(peer.last_seen) < peer_idle;
                if !active {
                    removed_peers.push(*peer_id);
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
        for peer_id in removed_peers {
            self.peer_rooms.remove(&peer_id);
            self.rates.remove(&peer_id);
        }
    }

    pub(crate) fn remove_peer(&mut self, peer_id: PeerId) {
        let Some(room_name) = self.peer_rooms.remove(&peer_id) else {
            self.pending.remove(&peer_id);
            self.rates.remove(&peer_id);
            return;
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
    }

    fn validate_join(&self, peer_id: PeerId, room_name: &str) -> Result<(), ControlMessage> {
        let room_name = room_name.trim();
        if room_name.is_empty() {
            return Err(error_message("empty_room", "room is required"));
        }
        if room_name.len() > self.config.max_room_name_len {
            return Err(error_message(
                "room_name_too_long",
                format!(
                    "room must be at most {} bytes",
                    self.config.max_room_name_len
                ),
            ));
        }

        let already_joined = self
            .peer_rooms
            .get(&peer_id)
            .is_some_and(|current| current == room_name);

        if let Some(room) = self.rooms.get(room_name) {
            if !already_joined
                && !room.peers.contains_key(&peer_id)
                && room.peers.len() >= self.config.max_peers_per_room
            {
                return Err(error_message("room_full", "room is full"));
            }
            return Ok(());
        }

        if !already_joined && self.rooms.len() >= self.config.max_rooms {
            return Err(error_message("too_many_rooms", "relay room limit reached"));
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

fn join_token() -> String {
    let value: u128 = rand::rng().random();
    format!("{value:032x}")
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
