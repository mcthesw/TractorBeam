use std::{
    collections::HashMap,
    net::SocketAddr,
    time::{Duration, Instant},
};

use basement_bridge_core::protocol::ControlMessage;
use rand::RngExt as _;
use tracing::info;

use crate::config::RelayConfig;

#[derive(Clone, Debug)]
struct PendingJoin {
    room: String,
    steam_id64: String,
    display_name: Option<String>,
    token: String,
    issued_at: Instant,
}

#[derive(Clone, Debug)]
struct Peer {
    steam_id64: String,
    display_name: Option<String>,
    last_seen: Instant,
}

#[derive(Debug, Default)]
struct Room {
    peers: HashMap<SocketAddr, Peer>,
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
    pending: HashMap<SocketAddr, PendingJoin>,
    rooms: HashMap<String, Room>,
    address_rooms: HashMap<SocketAddr, String>,
    rates: HashMap<SocketAddr, RateWindow>,
}

impl RelayState {
    pub(crate) fn new(config: RelayConfig) -> Self {
        Self {
            config,
            pending: HashMap::new(),
            rooms: HashMap::new(),
            address_rooms: HashMap::new(),
            rates: HashMap::new(),
        }
    }

    pub(crate) fn allow_packet(&mut self, address: SocketAddr, now: Instant) -> bool {
        let limit = self.config.rate_limit_per_second;
        let window = self.rates.entry(address).or_insert(RateWindow {
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
        address: SocketAddr,
        room: String,
        steam_id64: String,
        display_name: Option<String>,
        now: Instant,
    ) -> ControlMessage {
        if let Err(error) = self.validate_join(address, &room) {
            return error;
        }

        let token = join_token();
        self.pending.insert(
            address,
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

    pub(crate) fn complete_join(
        &mut self,
        address: SocketAddr,
        room: String,
        steam_id64: String,
        challenge: String,
        now: Instant,
    ) -> ControlMessage {
        let Some(pending) = self.pending.remove(&address) else {
            return error_message("missing_challenge", "join challenge was not issued");
        };
        if pending.room != room || pending.steam_id64 != steam_id64 || pending.token != challenge {
            return error_message("bad_challenge", "join challenge did not match");
        }
        self.remove_duplicate_peer(&pending.room, &pending.steam_id64, address);
        if let Err(error) = self.validate_join(address, &pending.room) {
            return error;
        }

        self.address_rooms.insert(address, pending.room.clone());
        let room = self.rooms.entry(pending.room.clone()).or_default();
        room.last_seen = Some(now);
        room.peers.insert(
            address,
            Peer {
                steam_id64: pending.steam_id64,
                display_name: pending.display_name,
                last_seen: now,
            },
        );
        info!(%address, room = %pending.room, peers = room.peers.len(), "peer joined");
        ControlMessage::Ready {
            peer_count: room.peers.len(),
        }
    }

    pub(crate) fn touch_peer(&mut self, address: SocketAddr, now: Instant) -> Option<String> {
        let room_name = self.address_rooms.get(&address)?.clone();
        let room = self.rooms.get_mut(&room_name)?;
        room.last_seen = Some(now);
        let peer = room.peers.get_mut(&address)?;
        peer.last_seen = now;
        Some(room_name)
    }

    pub(crate) fn target_address(&self, room_name: &str, steam_id64: u64) -> Option<SocketAddr> {
        let target = steam_id64.to_string();
        self.rooms.get(room_name).and_then(|room| {
            room.peers
                .iter()
                .find_map(|(address, peer)| (peer.steam_id64 == target).then_some(*address))
        })
    }

    #[cfg(test)]
    pub(crate) fn peer_addresses(&self, room_name: &str) -> Vec<SocketAddr> {
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

    pub(crate) fn cleanup(&mut self, now: Instant) {
        let peer_idle = Duration::from_secs(self.config.peer_idle_seconds);
        let room_idle = Duration::from_secs(self.config.room_idle_seconds);
        self.pending
            .retain(|_, pending| now.duration_since(pending.issued_at) < peer_idle);

        let mut removed_addresses = Vec::new();
        self.rooms.retain(|room_name, room| {
            room.peers.retain(|address, peer| {
                let active = now.duration_since(peer.last_seen) < peer_idle;
                if !active {
                    removed_addresses.push(*address);
                    info!(
                        %address,
                        room = %room_name,
                        steam_id64 = %peer.steam_id64,
                        display_name = peer.display_name.as_deref().unwrap_or(""),
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
        for address in removed_addresses {
            self.address_rooms.remove(&address);
            self.rates.remove(&address);
        }
    }

    fn validate_join(&self, address: SocketAddr, room_name: &str) -> Result<(), ControlMessage> {
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
            .address_rooms
            .get(&address)
            .is_some_and(|current| current == room_name);

        if let Some(room) = self.rooms.get(room_name) {
            if !already_joined
                && !room.peers.contains_key(&address)
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

    fn remove_duplicate_peer(&mut self, room_name: &str, steam_id64: &str, address: SocketAddr) {
        let Some(room) = self.rooms.get_mut(room_name) else {
            return;
        };

        let duplicate_addresses = room
            .peers
            .iter()
            .filter_map(|(peer_address, peer)| {
                (peer.steam_id64 == steam_id64 && *peer_address != address).then_some(*peer_address)
            })
            .collect::<Vec<_>>();

        for duplicate_address in duplicate_addresses {
            room.peers.remove(&duplicate_address);
            self.address_rooms.remove(&duplicate_address);
            self.rates.remove(&duplicate_address);
            info!(
                %duplicate_address,
                replacement = %address,
                room = %room_name,
                steam_id64 = %steam_id64,
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
mod tests {
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

    use super::*;

    fn address(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
    }

    fn challenge_token(message: ControlMessage) -> String {
        let ControlMessage::Challenge { token } = message else {
            panic!("expected challenge message");
        };
        token
    }

    fn error_code(message: ControlMessage) -> String {
        let ControlMessage::Error { code, .. } = message else {
            panic!("expected error message");
        };
        code
    }

    #[test]
    fn matches_blocked_cidrs() {
        let config = RelayConfig {
            blocked_cidrs: vec!["127.0.0.0/8".parse().unwrap()],
            ..RelayConfig::default()
        };
        let state = RelayState::new(config);

        assert!(state.is_blocked(address(25_910)));
        assert!(!state.is_blocked("192.0.2.1:25910".parse().unwrap()));
    }

    #[test]
    fn rejects_room_names_over_limit() {
        let config = RelayConfig {
            max_room_name_len: 4,
            ..RelayConfig::default()
        };
        let mut state = RelayState::new(config);

        let response = state.challenge_join(
            address(1),
            "abcde".to_owned(),
            "76561198000000001".to_owned(),
            None,
            Instant::now(),
        );

        assert_eq!(error_code(response), "room_name_too_long");
    }

    #[test]
    fn rejects_new_rooms_over_limit() {
        let config = RelayConfig {
            max_rooms: 1,
            ..RelayConfig::default()
        };
        let mut state = RelayState::new(config);
        let now = Instant::now();

        let token = challenge_token(state.challenge_join(
            address(1),
            "one".to_owned(),
            "76561198000000001".to_owned(),
            None,
            now,
        ));
        assert!(matches!(
            state.complete_join(
                address(1),
                "one".to_owned(),
                "76561198000000001".to_owned(),
                token,
                now
            ),
            ControlMessage::Ready { .. }
        ));

        let response = state.challenge_join(
            address(2),
            "two".to_owned(),
            "76561198000000002".to_owned(),
            None,
            now,
        );

        assert_eq!(error_code(response), "too_many_rooms");
    }

    #[test]
    fn rejects_new_peers_over_room_limit() {
        let config = RelayConfig {
            max_peers_per_room: 1,
            ..RelayConfig::default()
        };
        let mut state = RelayState::new(config);
        let now = Instant::now();

        let token = challenge_token(state.challenge_join(
            address(1),
            "room".to_owned(),
            "76561198000000001".to_owned(),
            None,
            now,
        ));
        assert!(matches!(
            state.complete_join(
                address(1),
                "room".to_owned(),
                "76561198000000001".to_owned(),
                token,
                now
            ),
            ControlMessage::Ready { .. }
        ));

        let response = state.challenge_join(
            address(2),
            "room".to_owned(),
            "76561198000000002".to_owned(),
            None,
            now,
        );

        assert_eq!(error_code(response), "room_full");
    }

    #[test]
    fn replaces_duplicate_steam_id_in_same_room() {
        let mut state = RelayState::new(RelayConfig::default());
        let now = Instant::now();

        let first_token = challenge_token(state.challenge_join(
            address(1),
            "room".to_owned(),
            "76561198000000001".to_owned(),
            None,
            now,
        ));
        assert!(matches!(
            state.complete_join(
                address(1),
                "room".to_owned(),
                "76561198000000001".to_owned(),
                first_token,
                now
            ),
            ControlMessage::Ready { .. }
        ));

        let second_token = challenge_token(state.challenge_join(
            address(2),
            "room".to_owned(),
            "76561198000000001".to_owned(),
            None,
            now,
        ));
        assert!(matches!(
            state.complete_join(
                address(2),
                "room".to_owned(),
                "76561198000000001".to_owned(),
                second_token,
                now
            ),
            ControlMessage::Ready { peer_count: 1 }
        ));

        assert_eq!(state.peer_addresses("room"), vec![address(2)]);
        assert_eq!(state.peer_count(), 1);
    }

    #[test]
    fn finds_target_address_by_steam_id() {
        let mut state = RelayState::new(RelayConfig::default());
        let now = Instant::now();

        let first_token = challenge_token(state.challenge_join(
            address(1),
            "room".to_owned(),
            "76561198000000001".to_owned(),
            None,
            now,
        ));
        assert!(matches!(
            state.complete_join(
                address(1),
                "room".to_owned(),
                "76561198000000001".to_owned(),
                first_token,
                now
            ),
            ControlMessage::Ready { .. }
        ));

        let second_token = challenge_token(state.challenge_join(
            address(2),
            "room".to_owned(),
            "76561198000000002".to_owned(),
            None,
            now,
        ));
        assert!(matches!(
            state.complete_join(
                address(2),
                "room".to_owned(),
                "76561198000000002".to_owned(),
                second_token,
                now
            ),
            ControlMessage::Ready { .. }
        ));

        assert_eq!(
            state.target_address("room", 76_561_198_000_000_002),
            Some(address(2))
        );
        assert_eq!(state.target_address("room", 76_561_198_000_000_003), None);
    }
}
