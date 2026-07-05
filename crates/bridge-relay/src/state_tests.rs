use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::{Duration, Instant},
};

use tractor_beam_core::protocol::{ClientMetadata, ControlMessage, PowChallenge, PowProof};

use super::*;
use crate::incident::{MISSING_TARGET_INITIAL_LOGS, MISSING_TARGET_LOG_INTERVAL};

fn address(port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
}

const fn peer(value: u64) -> PeerId {
    PeerId::new(value)
}

fn admission() -> Option<String> {
    Some("AbCdEfGhIjKlMn12".to_owned())
}

fn client() -> Option<ClientMetadata> {
    Some(ClientMetadata::current())
}

fn join_request(peer_id: PeerId, room: &str, steam_id64: &str, now: Instant) -> JoinRequest {
    JoinRequest {
        peer_id,
        room: room.to_owned(),
        steam_id64: steam_id64.to_owned(),
        display_name: None,
        client: client(),
        admission: admission(),
        now,
    }
}

fn challenge(message: ControlMessage) -> (String, Option<PowChallenge>) {
    let ControlMessage::Challenge { token, pow } = message else {
        panic!("expected challenge message");
    };
    (token, pow)
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
    let (token, pow) =
        challenge(state.challenge_join(join_request(peer_id, room, steam_id64, now)));
    let pow_proof = pow
        .as_ref()
        .and_then(|pow| PowProof::solve(pow, &token, room, steam_id64));
    assert!(matches!(
        state
            .complete_join(JoinCompletion {
                peer_id,
                room: room.to_owned(),
                steam_id64: steam_id64.to_owned(),
                client: client(),
                challenge: token,
                pow_proof,
                transport,
                now,
            })
            .response,
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
fn packet_rate_limit_caps_packets_per_peer() {
    let config = RelayConfig {
        rate_limit_per_second: 2,
        byte_rate_limit_burst: 1024,
        ..RelayConfig::default()
    };
    let mut state = RelayState::new(config);
    let now = Instant::now();

    assert!(state.allow_packet(peer(1), 1, now));
    assert!(state.allow_packet(peer(1), 1, now));
    assert!(!state.allow_packet(peer(1), 1, now));
    assert!(state.allow_packet(peer(1), 1, now + Duration::from_secs(1)));
}

#[test]
fn byte_token_bucket_caps_sustained_peer_traffic() {
    let config = RelayConfig {
        rate_limit_per_second: 100,
        byte_rate_limit_per_second: 10,
        byte_rate_limit_burst: 20,
        ..RelayConfig::default()
    };
    let mut state = RelayState::new(config);
    let now = Instant::now();

    assert!(state.allow_packet(peer(1), 15, now));
    assert!(state.allow_packet(peer(1), 5, now));
    assert!(!state.allow_packet(peer(1), 1, now));
    assert!(state.allow_packet(peer(1), 5, now + Duration::from_millis(500)));
}

#[test]
fn byte_token_bucket_denial_does_not_consume_packet_budget() {
    let config = RelayConfig {
        rate_limit_per_second: 2,
        byte_rate_limit_per_second: 20,
        byte_rate_limit_burst: 10,
        ..RelayConfig::default()
    };
    let mut state = RelayState::new(config);
    let now = Instant::now();
    let half_second = now + Duration::from_millis(500);

    assert!(state.allow_packet(peer(1), 10, now));
    assert!(!state.allow_packet(peer(1), 11, half_second));
    assert!(state.allow_packet(peer(1), 1, half_second));
}

#[test]
fn cleanup_reports_idle_unjoined_rate_peers() {
    let config = RelayConfig {
        peer_idle_seconds: 1,
        ..RelayConfig::default()
    };
    let mut state = RelayState::new(config);
    let now = Instant::now();

    assert!(state.allow_packet(peer(1), 1, now));

    let cleanup = state.cleanup(now + Duration::from_secs(1));

    assert_eq!(cleanup.removed_peers, vec![peer(1)]);
    assert!(cleanup.broadcasts.is_empty());
}

#[test]
fn health_pong_rate_limit_caps_replies_per_source_ip() {
    let config = RelayConfig {
        health_pongs_per_second_per_ip: 2,
        ..RelayConfig::default()
    };
    let mut state = RelayState::new(config);
    let source = Ipv4Addr::LOCALHOST.into();
    let now = Instant::now();

    assert!(state.allow_health_pong(source, now));
    assert!(state.allow_health_pong(source, now));
    assert!(!state.allow_health_pong(source, now));
    assert!(state.allow_health_pong(source, now + Duration::from_secs(1)));
}

#[test]
fn rejects_room_names_over_limit() {
    let config = RelayConfig {
        max_room_name_len: 4,
        ..RelayConfig::default()
    };
    let mut state = RelayState::new(config);

    let response = state.challenge_join(join_request(
        peer(1),
        "abcde",
        "76561198000000001",
        Instant::now(),
    ));

    assert_eq!(error_code(response), "room_name_too_long");
}

#[test]
fn rejects_missing_client_metadata() {
    let mut state = RelayState::new(RelayConfig::default());
    let mut request = join_request(peer(1), "room", "76561198000000001", Instant::now());
    request.client = None;

    let response = state.challenge_join(request);

    assert_eq!(error_code(response), "client_metadata_required");
}

#[test]
fn rejects_unsupported_client_protocol() {
    let mut state = RelayState::new(RelayConfig::default());
    let mut metadata = ClientMetadata::current();
    metadata.protocol_major = metadata.protocol_major.saturating_add(1);
    let mut request = join_request(peer(1), "room", "76561198000000001", Instant::now());
    request.client = Some(metadata);

    let response = state.challenge_join(request);

    assert_eq!(error_code(response), "unsupported_protocol");
}

#[test]
fn rejects_missing_client_metadata_on_challenge_completion() {
    let mut state = RelayState::new(RelayConfig::default());
    let now = Instant::now();
    let (token, _pow) =
        challenge(state.challenge_join(join_request(peer(1), "room", "76561198000000001", now)));

    let outcome = state.complete_join(JoinCompletion {
        peer_id: peer(1),
        room: "room".to_owned(),
        steam_id64: "76561198000000001".to_owned(),
        client: None,
        challenge: token,
        pow_proof: None,
        transport: PeerTransport::Udp,
        now,
    });

    assert_eq!(error_code(outcome.response), "client_metadata_required");
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

    let response = state.challenge_join(join_request(peer(2), "two", "76561198000000002", now));

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

    let response = state.challenge_join(join_request(peer(2), "room", "76561198000000002", now));

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

#[test]
fn rejects_missing_admission_material() {
    let mut state = RelayState::new(RelayConfig::default());
    let mut request = join_request(peer(1), "room", "76561198000000001", Instant::now());
    request.admission = None;

    let response = state.challenge_join(request);

    assert_eq!(error_code(response), "admission_required");
}

#[test]
fn rejects_mismatched_room_admission_material() {
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

    let mut request = join_request(peer(2), "room", "76561198000000002", now);
    request.admission = Some("DifferentAdmission".to_owned());
    let response = state.challenge_join(request);

    assert_eq!(error_code(response), "room_admission_mismatch");
}

#[test]
fn rejects_missing_pow_proof_when_required() {
    let config = RelayConfig {
        pow_difficulty_bits: 4,
        ..RelayConfig::default()
    };
    let mut state = RelayState::new(config);
    let now = Instant::now();
    let (token, _pow) =
        challenge(state.challenge_join(join_request(peer(1), "room", "76561198000000001", now)));
    state
        .pending
        .get_mut(&peer(1))
        .and_then(|pending| pending.pow.as_mut())
        .expect("pending pow challenge")
        .algorithm = "unsupported".to_owned();

    let outcome = state.complete_join(JoinCompletion {
        peer_id: peer(1),
        room: "room".to_owned(),
        steam_id64: "76561198000000001".to_owned(),
        client: client(),
        challenge: token,
        pow_proof: None,
        transport: PeerTransport::Udp,
        now,
    });

    assert_eq!(error_code(outcome.response), "pow_required");
}

#[test]
fn rejects_bad_pow_proof() {
    let config = RelayConfig {
        pow_difficulty_bits: 4,
        ..RelayConfig::default()
    };
    let mut state = RelayState::new(config);
    let now = Instant::now();
    let (token, _pow) =
        challenge(state.challenge_join(join_request(peer(1), "room", "76561198000000001", now)));

    let outcome = state.complete_join(JoinCompletion {
        peer_id: peer(1),
        room: "room".to_owned(),
        steam_id64: "76561198000000001".to_owned(),
        client: client(),
        challenge: token,
        pow_proof: Some(PowProof {
            nonce: "bad".to_owned(),
        }),
        transport: PeerTransport::Udp,
        now,
    });

    assert_eq!(error_code(outcome.response), "pow_failed");
}

#[test]
fn rejects_unexpected_pow_proof_when_pow_is_disabled() {
    let config = RelayConfig {
        pow_difficulty_bits: 0,
        ..RelayConfig::default()
    };
    let mut state = RelayState::new(config);
    let now = Instant::now();
    let (token, _pow) =
        challenge(state.challenge_join(join_request(peer(1), "room", "76561198000000001", now)));

    let outcome = state.complete_join(JoinCompletion {
        peer_id: peer(1),
        room: "room".to_owned(),
        steam_id64: "76561198000000001".to_owned(),
        client: client(),
        challenge: token,
        pow_proof: Some(PowProof {
            nonce: "unexpected".to_owned(),
        }),
        transport: PeerTransport::Udp,
        now,
    });

    assert_eq!(error_code(outcome.response), "pow_unexpected");
}
