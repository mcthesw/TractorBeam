use std::net::SocketAddr;

use bytes::Bytes;
use tractor_beam_direct_protocol::{
    CAP_DIRECT_UDP, CAP_HOST_CANDIDATES, CAP_MEMBERSHIP_SNAPSHOT, CHECK_FRAME_HEADER_LEN,
    CheckFrame, CheckPhase, ControlDecodeError, ControlMessage, ControlValidationError,
    DATA_FRAME_HEADER_LEN, DataFrame, DirectFrame, HEARTBEAT_FRAME_HEADER_LEN, HeartbeatFrame,
    HeartbeatPhase, HostCandidate, IPV4_SAFE_DATA_PAYLOAD, InstanceId, MAX_CANDIDATES,
    MAX_CONTROL_PAYLOAD, MAX_DATA_PAYLOAD, MAX_FRAME_LEN, MAX_PEERS, PathContext, PathId,
    PathToken, PeerDescriptor, PeerIdentity, ProtocolVersion, TransactionId, decode_control,
    decode_frame, encode_control,
};

fn fixture_hex(name: &str) -> Vec<u8> {
    let text = match name {
        "check" => include_str!("fixtures/v1/check.hex"),
        "heartbeat" => include_str!("fixtures/v1/heartbeat.hex"),
        "data-4" => include_str!("fixtures/v1/data-4.hex"),
        _ => panic!("unknown fixture {name}"),
    };
    let compact = text.trim();
    assert_eq!(compact.len() % 2, 0);
    compact
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(pair, 16).unwrap()
        })
        .collect()
}

fn path() -> PathContext {
    PathContext {
        path_id: PathId::from_bytes([1; 16]),
        path_token: PathToken::from_bytes([2; 16]),
        from: PeerIdentity::new(3, InstanceId::from_bytes([4; 16])),
        to_steam_id64: 5,
    }
}

fn candidate(index: usize) -> HostCandidate {
    let port = 25_900 + u16::try_from(index).unwrap();
    HostCandidate::new(
        SocketAddr::from(([127, 0, 0, 1], port)),
        u32::try_from(index + 1).unwrap(),
        0,
    )
    .unwrap()
}

fn descriptor(index: usize) -> PeerDescriptor {
    let byte = u8::try_from(index + 1).unwrap();
    PeerDescriptor {
        identity: PeerIdentity::new(
            u64::try_from(index + 1).unwrap(),
            InstanceId::from_bytes([byte; 16]),
        ),
        display_name: None,
        control_candidates: vec![candidate(index)],
        capabilities: CAP_HOST_CANDIDATES | CAP_DIRECT_UDP | CAP_MEMBERSHIP_SNAPSHOT,
    }
}

#[test]
fn control_family_matches_golden_fixture() {
    let expected = include_bytes!("fixtures/v1/control-ping.json")
        .strip_suffix(b"\n")
        .unwrap_or(include_bytes!("fixtures/v1/control-ping.json"));
    let message = ControlMessage::ControlPing {
        id: 0x0102_0304_0506_0708,
    };
    assert_eq!(encode_control(&message).unwrap().as_ref(), expected);
    assert_eq!(decode_control(expected).unwrap(), message);
}

#[test]
fn fixed_udp_frame_families_match_golden_fixtures() {
    let frames = [
        (
            "check",
            DirectFrame::Check(CheckFrame {
                path: path(),
                transaction_id: TransactionId::from_bytes([6; 16]),
                phase: CheckPhase::Request,
            }),
        ),
        (
            "heartbeat",
            DirectFrame::Heartbeat(HeartbeatFrame {
                path: path(),
                heartbeat_id: 7,
                phase: HeartbeatPhase::Response,
            }),
        ),
        (
            "data-4",
            DirectFrame::Data(DataFrame {
                path: path(),
                frame_id: 8,
                source_sequence: 9,
                channel: -2,
                send_type: 3,
                payload: Bytes::from_static(&[0, 1, 2, 3]),
            }),
        ),
    ];
    for (name, frame) in frames {
        let expected = fixture_hex(name);
        let encoded = frame.encode().unwrap();
        assert_eq!(encoded.as_ref(), expected);
        assert_eq!(decode_frame(Bytes::from(expected)).unwrap(), frame);
    }
    assert_eq!(CHECK_FRAME_HEADER_LEN, 104);
    assert_eq!(HEARTBEAT_FRAME_HEADER_LEN, 96);
    assert_eq!(DATA_FRAME_HEADER_LEN, 100);
}

#[test]
fn control_payload_boundaries_are_checked_before_json_allocation() {
    let base = br#"{"type":"leave"}"#;
    for size in [MAX_CONTROL_PAYLOAD - 1, MAX_CONTROL_PAYLOAD] {
        let mut payload = Vec::from(base.as_slice());
        payload.resize(size, b' ');
        assert_eq!(decode_control(&payload).unwrap(), ControlMessage::Leave);
    }
    let oversized = vec![b' '; MAX_CONTROL_PAYLOAD + 1];
    assert!(matches!(
        decode_control(&oversized),
        Err(ControlDecodeError::PayloadTooLarge(size)) if size == MAX_CONTROL_PAYLOAD + 1
    ));
}

#[test]
fn candidate_and_peer_list_boundaries_are_enforced() {
    let path_offer = ControlMessage::PathOffer {
        peer: descriptor(0).identity,
        path_id: PathId::from_bytes([1; 16]),
        path_token: PathToken::from_bytes([2; 16]),
        data_candidates: (0..MAX_CANDIDATES).map(candidate).collect(),
    };
    assert!(encode_control(&path_offer).is_ok());

    let too_many_candidates = ControlMessage::PathOffer {
        peer: descriptor(0).identity,
        path_id: PathId::from_bytes([1; 16]),
        path_token: PathToken::from_bytes([2; 16]),
        data_candidates: (0..=MAX_CANDIDATES).map(candidate).collect(),
    };
    assert!(encode_control(&too_many_candidates).is_err());

    let snapshot = ControlMessage::PeerSnapshot {
        peers: (0..MAX_PEERS).map(descriptor).collect(),
    };
    assert!(encode_control(&snapshot).is_ok());
    let oversized_snapshot = ControlMessage::PeerSnapshot {
        peers: (0..=MAX_PEERS).map(descriptor).collect(),
    };
    assert!(matches!(
        encode_control(&oversized_snapshot),
        Err(tractor_beam_direct_protocol::ControlEncodeError::Validation(
            ControlValidationError::TooManyPeers(size)
        )) if size == MAX_PEERS + 1
    ));
}

#[test]
fn data_payload_boundaries_preserve_safe_datagram_budget() {
    for size in [MAX_DATA_PAYLOAD - 1, MAX_DATA_PAYLOAD] {
        let frame = DataFrame {
            path: path(),
            frame_id: 1,
            source_sequence: 1,
            channel: 0,
            send_type: 0,
            payload: Bytes::from(vec![0; size]),
        };
        assert_eq!(frame.encode().unwrap().len(), DATA_FRAME_HEADER_LEN + size);
    }
    assert_eq!(IPV4_SAFE_DATA_PAYLOAD, MAX_DATA_PAYLOAD);
    assert_eq!(DATA_FRAME_HEADER_LEN + MAX_DATA_PAYLOAD, MAX_FRAME_LEN);

    let oversized = DataFrame {
        path: path(),
        frame_id: 1,
        source_sequence: 1,
        channel: 0,
        send_type: 0,
        payload: Bytes::from(vec![0; MAX_DATA_PAYLOAD + 1]),
    };
    assert!(oversized.encode().is_err());
}

#[test]
fn protocol_version_remains_explicit_at_wire_boundary() {
    let version = ProtocolVersion { major: 1, minor: 0 };
    assert_eq!(version.major, tractor_beam_direct_protocol::PROTOCOL_MAJOR);
    assert_eq!(version.minor, tractor_beam_direct_protocol::PROTOCOL_MINOR);
}
