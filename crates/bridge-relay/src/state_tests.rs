use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::{Duration, Instant},
};

use basement_bridge_core::protocol::ControlMessage;

use super::*;
use crate::incident::{MISSING_TARGET_INITIAL_LOGS, MISSING_TARGET_LOG_INTERVAL};

fn address(port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
}

const fn peer(value: u64) -> PeerId {
    PeerId::new(value)
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

fn join_peer(
    state: &mut RelayState,
    peer_id: PeerId,
    room: &str,
    steam_id64: &str,
    transport: PeerTransport,
    now: Instant,
) {
    let token = challenge_token(state.challenge_join(
        peer_id,
        room.to_owned(),
        steam_id64.to_owned(),
        None,
        now,
    ));
    assert!(matches!(
        state.complete_join(JoinCompletion {
            peer_id,
            room: room.to_owned(),
            steam_id64: steam_id64.to_owned(),
            challenge: token,
            transport,
            now,
        }),
        ControlMessage::Ready { .. }
    ));
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
        peer(1),
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

    join_peer(
        &mut state,
        peer(1),
        "one",
        "76561198000000001",
        PeerTransport::Udp,
        now,
    );

    let response = state.challenge_join(
        peer(2),
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

    join_peer(
        &mut state,
        peer(1),
        "room",
        "76561198000000001",
        PeerTransport::Udp,
        now,
    );

    let response = state.challenge_join(
        peer(2),
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

    join_peer(
        &mut state,
        peer(1),
        "room",
        "76561198000000001",
        PeerTransport::Udp,
        now,
    );
    join_peer(
        &mut state,
        peer(2),
        "room",
        "76561198000000001",
        PeerTransport::Udp,
        now,
    );

    assert_eq!(state.peer_ids("room"), vec![peer(2)]);
    assert_eq!(state.peer_count(), 1);
}

#[test]
fn finds_target_peer_by_steam_id() {
    let mut state = RelayState::new(RelayConfig::default());
    let now = Instant::now();

    join_peer(
        &mut state,
        peer(1),
        "room",
        "76561198000000001",
        PeerTransport::Udp,
        now,
    );
    join_peer(
        &mut state,
        peer(2),
        "room",
        "76561198000000002",
        PeerTransport::Udp,
        now,
    );

    assert_eq!(
        state.target_peer("room", 76_561_198_000_000_002),
        Some(peer(2))
    );
    assert_eq!(state.target_peer("room", 76_561_198_000_000_003), None);
}

#[test]
fn room_summaries_count_peer_transports() {
    let mut state = RelayState::new(RelayConfig::default());
    let now = Instant::now();

    join_peer(
        &mut state,
        peer(1),
        "room",
        "76561198000000001",
        PeerTransport::Udp,
        now,
    );
    join_peer(
        &mut state,
        peer(2),
        "room",
        "76561198000000002",
        PeerTransport::Tcp,
        now,
    );

    assert_eq!(
        state.room_summaries(),
        vec![RoomSummary {
            name: "room".to_owned(),
            peers: 2,
            tcp_peers: 1,
            udp_peers: 1,
        }]
    );
}

#[test]
fn missing_target_incidents_snapshot_room_peers_and_are_sampled() {
    let mut state = RelayState::new(RelayConfig::default());
    let now = Instant::now();
    join_peer(
        &mut state,
        peer(1),
        "room",
        "76561198000000001",
        PeerTransport::Udp,
        now,
    );
    join_peer(
        &mut state,
        peer(2),
        "room",
        "76561198000000002",
        PeerTransport::Tcp,
        now,
    );

    let first = state
        .record_missing_target_incident("room", now)
        .expect("first missing target should be logged");
    assert_eq!(first.peer_count(), 2);
    assert_eq!(first.tcp_peer_count(), 1);
    assert_eq!(first.udp_peer_count(), 1);
    assert_eq!(
        first.peer_summary(),
        "peer-1:76561198000000001:udp,peer-2:76561198000000002:tcp"
    );

    for _ in 1..MISSING_TARGET_INITIAL_LOGS {
        assert!(
            state
                .record_missing_target_incident("room", now + Duration::from_secs(1))
                .is_some()
        );
    }
    assert!(
        state
            .record_missing_target_incident("room", now + Duration::from_secs(1))
            .is_none()
    );
    assert!(
        state
            .record_missing_target_incident(
                "room",
                now + Duration::from_secs(1) + MISSING_TARGET_LOG_INTERVAL
            )
            .is_some()
    );
}
