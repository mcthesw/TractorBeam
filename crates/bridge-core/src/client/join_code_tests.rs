use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

use bs58::{FromBase58 as _, ToBase58 as _};
use tractor_beam_direct_protocol::{InstanceId, MAX_CANDIDATES, PeerIdentity};

use super::*;

fn credential() -> SessionCredential {
    SessionCredential::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
}

fn relay_code(relay_id: Option<&str>) -> JoinCode {
    JoinCode::ExternalRelay(RelayJoinCode {
        relay_id: relay_id.map(str::to_owned),
        relay_host: "relay.example.test".to_owned(),
        relay_port: 25_910,
        session_credential: credential(),
    })
}

fn lan_code() -> JoinCode {
    JoinCode::LanDirect(LanJoinCode {
        introducer: PeerIdentity::new(76_561_198_000_000_001, InstanceId::from_bytes([0x11; 16])),
        control_endpoints: vec![
            SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 7), 25_910).into(),
            SocketAddrV6::new("fd00::7".parse::<Ipv6Addr>().unwrap(), 25_910, 0, 4).into(),
        ],
        session_credential: credential(),
    })
}

#[test]
fn round_trips_relay_v5_and_lan_v6_with_surrounding_whitespace() {
    for code in [relay_code(Some("guangzhou")), relay_code(None), lan_code()] {
        let encoded = code.encode().unwrap();
        assert!(encoded.starts_with('T'));
        assert!(encoded.ends_with('T'));
        assert!(!encoded.contains("relay.example.test"));
        assert_eq!(
            JoinCode::decode(&format!(" \r\n{encoded}\n ")).unwrap(),
            code
        );
    }
}

#[test]
fn relay_v5_and_lan_v6_match_golden_fixtures() {
    let relay = include_str!("fixtures/join-code/relay-v5.txt").trim();
    assert_eq!(relay_code(Some("guangzhou")).encode().unwrap(), relay);
    assert_eq!(
        JoinCode::decode(relay).unwrap(),
        relay_code(Some("guangzhou"))
    );

    let lan = include_str!("fixtures/join-code/lan-v6.txt").trim();
    assert_eq!(lan_code().encode().unwrap(), lan);
    assert_eq!(JoinCode::decode(lan).unwrap(), lan_code());
}

#[test]
fn generated_credentials_are_random_and_redacted() {
    let first = SessionCredential::generate();
    let second = SessionCredential::generate();
    assert_ne!(first, second);
    let rendered = format!("{first:?}");
    assert!(rendered.contains("REDACTED"));
    assert_eq!(rendered, "SessionCredential([REDACTED])");
}

#[test]
fn rejects_legacy_v4_with_specific_error() {
    assert!(matches!(
        JoinCode::decode("BlegacyB"),
        Err(JoinCodeError::LegacyV4)
    ));
}

#[test]
fn detects_corruption_truncation_and_wrong_version() {
    let encoded = relay_code(Some("relay")).encode().unwrap();
    let inner = encoded.trim_matches('T');
    let original = inner.from_base58().unwrap();

    let mut corrupted = original.clone();
    corrupted[10] ^= 1;
    let corrupted = format!("T{}T", corrupted.to_base58());
    assert!(matches!(
        JoinCode::decode(&corrupted),
        Err(JoinCodeError::ChecksumMismatch)
    ));

    let mut truncated = original.clone();
    truncated.pop();
    let truncated = format!("T{}T", truncated.to_base58());
    assert!(matches!(
        JoinCode::decode(&truncated),
        Err(JoinCodeError::Truncated)
    ));

    let mut wrong_version = original;
    wrong_version[2] = 99;
    let wrong_version = format!("T{}T", wrong_version.to_base58());
    assert!(matches!(
        JoinCode::decode(&wrong_version),
        Err(JoinCodeError::UnsupportedVersion(99))
    ));
}

#[test]
fn rejects_relay_bounds_before_encoding() {
    let JoinCode::ExternalRelay(mut code) = relay_code(None) else {
        panic!("fixture is Relay")
    };
    code.relay_host = "x".repeat(relay::MAX_RELAY_HOST_LEN_FOR_TEST + 1);
    assert!(matches!(
        JoinCode::ExternalRelay(code.clone()).encode(),
        Err(JoinCodeError::RelayHostTooLong)
    ));
    code.relay_host = "host".to_owned();
    code.relay_port = 0;
    assert!(matches!(
        JoinCode::ExternalRelay(code).encode(),
        Err(JoinCodeError::InvalidRelayPort)
    ));
}

#[test]
fn lan_candidate_bounds_duplicates_and_scope_are_rejected() {
    let JoinCode::LanDirect(mut code) = lan_code() else {
        panic!("fixture is LAN")
    };
    code.control_endpoints = (0..=MAX_CANDIDATES)
        .map(|index| {
            SocketAddrV4::new(
                Ipv4Addr::new(10, 0, 0, u8::try_from(index + 1).unwrap()),
                25_910,
            )
            .into()
        })
        .collect();
    assert!(matches!(
        JoinCode::LanDirect(code.clone()).encode(),
        Err(JoinCodeError::TooManyLanCandidates(size)) if size == MAX_CANDIDATES + 1
    ));

    code.control_endpoints = vec!["10.0.0.7:25910".parse().unwrap(); 2];
    assert!(matches!(
        JoinCode::LanDirect(code.clone()).encode(),
        Err(JoinCodeError::DuplicateLanCandidate(_))
    ));

    code.control_endpoints =
        vec![SocketAddrV6::new("fe80::1".parse().unwrap(), 25_910, 0, 0).into()];
    assert!(matches!(
        JoinCode::LanDirect(code).encode(),
        Err(JoinCodeError::InvalidLanCandidate(
            tractor_beam_direct_protocol::CandidateValidationError::MissingIpv6Scope
        ))
    ));
}

#[test]
fn lan_payload_bound_is_tight_for_eight_ipv6_candidates() {
    let JoinCode::LanDirect(mut code) = lan_code() else {
        panic!("fixture is LAN")
    };
    code.control_endpoints = (0..MAX_CANDIDATES)
        .map(|index| {
            SocketAddrV6::new(
                Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, u16::try_from(index + 1).unwrap()),
                25_910,
                0,
                0,
            )
            .into()
        })
        .collect();
    let encoded = JoinCode::LanDirect(code.clone()).encode().unwrap();
    let payload = encoded.trim_matches('T').from_base58().unwrap();
    assert_eq!(payload.len(), 244);
    assert_eq!(
        JoinCode::decode(&encoded).unwrap(),
        JoinCode::LanDirect(code)
    );
}

#[test]
fn every_truncated_lan_payload_fails_without_panicking() {
    let encoded = lan_code().encode().unwrap();
    let bytes = encoded.trim_matches('T').from_base58().unwrap();
    for length in 0..bytes.len() {
        let truncated = format!("T{}T", bytes[..length].to_base58());
        assert!(JoinCode::decode(&truncated).is_err());
    }
}
