use bytes::Bytes;

use tractor_beam_relay_protocol::*;

#[test]
fn envelope_v1_bytes_are_stable() {
    let envelope = Envelope {
        message_type: MessageType::Data,
        flags: 0xa5,
        capabilities: 0x0102_0304_0506_0708,
        sequence: 0x1112_1314_1516_1718,
        nonce: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        payload: Bytes::from_static(b"abc"),
    };
    let expected = fixture_hex(include_str!("fixtures/v1/envelope-data.hex"));

    assert_eq!(envelope.encode().unwrap().as_ref(), expected);
    assert_eq!(Envelope::decode(Bytes::from(expected)).unwrap(), envelope);
}

#[test]
fn game_packet_v1_bytes_are_stable() {
    let game = GamePacket {
        from_steam_id64: "76561198000000001".to_owned(),
        to_steam_id64: 42,
        source_sequence: 7,
        channel: 1,
        send_type: 2,
        payload: Bytes::from_static(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]),
    };
    let expected = fixture_hex(include_str!("fixtures/v1/game-16.hex"));

    assert_eq!(game.encode().unwrap().as_ref(), expected);
    assert_eq!(GamePacket::decode(&expected).unwrap(), game);
}

#[test]
fn control_message_v1_json_is_stable() {
    let peers = vec![PeerInfo {
        steam_id64: "76561198000000001".to_owned(),
        display_name: Some("Alice".to_owned()),
        transport: PeerTransport::Tcp,
    }];
    let cases = [
        (
            ControlMessage::Join {
                room: "contract-room".to_owned(),
                steam_id64: "76561198000000001".to_owned(),
                display_name: Some("Alice".to_owned()),
                client: Some(ClientMetadata {
                    app_version: "0.2.1".to_owned(),
                    git_hash: Some("0123456789ab".to_owned()),
                    protocol_major: 1,
                    protocol_minor: 0,
                    features: 13,
                }),
                challenge: None,
                pow_proof: None,
                admission: Some("AbCdEfGhIjKlMn12".to_owned()),
            },
            include_str!("fixtures/v1/join.json"),
        ),
        (
            ControlMessage::Challenge {
                token: "token".to_owned(),
                pow: Some(PowChallenge::sha256("nonce".to_owned(), 18)),
            },
            include_str!("fixtures/v1/challenge.json"),
        ),
        (
            ControlMessage::Ready { peers },
            include_str!("fixtures/v1/ready.json"),
        ),
        (
            ControlMessage::RoomUpdate {
                peers: vec![PeerInfo {
                    steam_id64: "76561198000000002".to_owned(),
                    display_name: None,
                    transport: PeerTransport::Udp,
                }],
            },
            include_str!("fixtures/v1/room-update.json"),
        ),
        (
            ControlMessage::Error {
                code: "bad_join".to_owned(),
                message: "expected join message".to_owned(),
            },
            include_str!("fixtures/v1/error.json"),
        ),
        (
            ControlMessage::HealthPing { id: 42 },
            include_str!("fixtures/v1/health-ping.json"),
        ),
        (
            ControlMessage::HealthPong { id: 42 },
            include_str!("fixtures/v1/health-pong.json"),
        ),
    ];

    for (message, fixture) in cases {
        let expected = fixture.trim().as_bytes();
        assert_eq!(message.encode().unwrap().as_ref(), expected);
        assert_eq!(ControlMessage::decode(expected).unwrap(), message);
    }
}

#[test]
fn legacy_ready_without_peers_remains_accepted() {
    let decoded = ControlMessage::decode(br#"{"type":"ready","peer_count":3}"#).unwrap();
    assert_eq!(decoded, ControlMessage::Ready { peers: Vec::new() });
}

#[test]
fn envelope_rejects_bad_magic_and_accepts_future_header_extensions() {
    let encoded = Envelope::new(MessageType::Heartbeat, b"payload".as_slice())
        .encode()
        .unwrap();
    let mut bad_magic = encoded.to_vec();
    bad_magic[0] = b'X';
    assert_eq!(
        Envelope::decode(Bytes::from(bad_magic)).unwrap_err(),
        DecodeError::BadMagic
    );

    let extension = [0xde, 0xad, 0xbe, 0xef];
    let mut extended = encoded.to_vec();
    extended[8..10].copy_from_slice(
        &u16::try_from(ENVELOPE_HEADER_LEN + extension.len())
            .unwrap()
            .to_be_bytes(),
    );
    extended.splice(ENVELOPE_HEADER_LEN..ENVELOPE_HEADER_LEN, extension);
    let decoded = Envelope::decode(Bytes::from(extended)).unwrap();
    assert_eq!(decoded.message_type, MessageType::Heartbeat);
    assert_eq!(decoded.payload.as_ref(), b"payload");
}

#[test]
fn pow_solution_round_trips_and_is_bound_to_admission_inputs() {
    let challenge = PowChallenge::sha256("nonce".to_owned(), 8);
    let proof = PowProof::solve(&challenge, "token", "room", "76561198000000001").unwrap();

    assert!(proof.verify(&challenge, "token", "room", "76561198000000001"));
    assert!(!proof.verify(&challenge, "other", "room", "76561198000000001"));
}

#[test]
fn ipv4_fragmentation_budget_boundaries_are_explicit() {
    assert_eq!(V1_GAME_WIRE_OVERHEAD, 82);
    assert_eq!(IPV4_SAFE_GAME_PAYLOAD, 1_390);
    assert_eq!(encoded_game_datagram_len(1_100), 1_182);
    assert_eq!(encoded_game_datagram_len(1_390), IPV4_UDP_DATAGRAM_BUDGET);
    assert_eq!(
        encoded_game_datagram_len(1_391),
        IPV4_UDP_DATAGRAM_BUDGET + 1
    );
}

#[test]
fn metadata_factory_keeps_exact_v1_and_non_negotiating_capabilities() {
    let metadata = ClientMetadata::for_build("0.2.1", Some("0123456789ab"));
    assert_eq!(metadata.protocol_major, PROTOCOL_MAJOR);
    assert_eq!(metadata.protocol_minor, PROTOCOL_MINOR);
    assert_eq!(
        metadata.features,
        CAP_PATH_VALIDATION | CAP_POW_ADMISSION | CAP_ADMISSION_MATERIAL
    );
}

fn encoded_game_datagram_len(payload_len: usize) -> usize {
    let game = GamePacket {
        from_steam_id64: "76561198000000001".to_owned(),
        to_steam_id64: 42,
        source_sequence: 7,
        channel: 1,
        send_type: 2,
        payload: Bytes::from(vec![0; payload_len]),
    };
    let game = game.encode().unwrap();
    Envelope::new(MessageType::Data, game)
        .encode()
        .unwrap()
        .len()
}

fn fixture_hex(value: &str) -> Vec<u8> {
    let value = value.trim();
    assert!(value.len().is_multiple_of(2));
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(pair, 16).unwrap()
        })
        .collect()
}
