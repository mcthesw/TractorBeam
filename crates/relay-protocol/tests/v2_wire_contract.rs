use bytes::Bytes;
use tractor_beam_relay_protocol::v2::{
    BOOTSTRAP_SCHEMA, BootstrapDecodeError, BootstrapMessage, BuildMetadata, CAP_RESUME,
    CAP_TCP_DATA, CAP_UDP_DATA, COMMON_HEADER_LEN, CapabilityError, ClientControl,
    DATA_FRAME_HEADER_LEN, DATA_FRAME_OVERHEAD, DataFrame, DataProfile, DuplicateDecision, Frame,
    FrameDecodeError, FrameIdWindow, IPV4_SAFE_DATA_PAYLOAD, MAX_BOOTSTRAP_PAYLOAD,
    MAX_CONTROL_PAYLOAD, PROBE_FRAME_HEADER_LEN, ProbeFrame, ProbePhase, ProtocolRange,
    SecretString, decode_bootstrap, decode_client_control, decode_frame, decode_server_control,
    encode_bootstrap, encode_client_control, encode_server_control, select_capabilities,
    select_protocol,
};

fn fixture_hex(path: &str) -> Vec<u8> {
    let text = match path {
        "bootstrap-client-hello" => include_str!("fixtures/v2/bootstrap-client-hello.hex"),
        "data-16" => include_str!("fixtures/v2/data-16.hex"),
        _ => panic!("unknown fixture {path}"),
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

fn client_hello() -> BootstrapMessage {
    BootstrapMessage::ClientHello {
        bootstrap_schema: BOOTSTRAP_SCHEMA,
        supported_protocol_ranges: vec![ProtocolRange {
            major: 2,
            min_minor: 0,
            max_minor: 0,
        }],
        required_capabilities: CAP_RESUME,
        optional_capabilities: CAP_TCP_DATA | CAP_UDP_DATA,
        client: BuildMetadata {
            version: "0.2.1".to_owned(),
            git_hash: Some("0123456789ab".to_owned()),
        },
    }
}

#[test]
fn bootstrap_client_hello_matches_golden_fixture() {
    let expected = fixture_hex("bootstrap-client-hello");
    let encoded = encode_bootstrap(&client_hello()).unwrap();

    assert_eq!(encoded.as_ref(), expected);
    assert_eq!(decode_bootstrap(&expected).unwrap(), client_hello());
}

#[test]
fn bootstrap_ignores_unknown_optional_json_fields() {
    let json = br#"{"type":"server_hello","bootstrap_schema":1,"selected_protocol":{"major":2,"minor":0},"enabled_capabilities":5,"relay":{"version":"0.2.1"},"future_optional":{"enabled":true}}"#;
    let mut framed = Vec::from(u32::try_from(json.len()).unwrap().to_be_bytes());
    framed.extend_from_slice(json);

    let decoded = decode_bootstrap(&framed).unwrap();
    assert!(matches!(decoded, BootstrapMessage::ServerHello { .. }));
}

#[test]
fn bootstrap_enforces_length_prefix_and_bound() {
    assert!(matches!(
        decode_bootstrap(&[0, 0, 0, 1]),
        Err(BootstrapDecodeError::TooShort)
    ));
    let oversized = u32::try_from(MAX_BOOTSTRAP_PAYLOAD + 1)
        .unwrap()
        .to_be_bytes();
    assert!(matches!(
        decode_bootstrap(&oversized),
        Err(BootstrapDecodeError::PayloadTooLarge(size))
            if size == MAX_BOOTSTRAP_PAYLOAD + 1
    ));

    let mut trailing = encode_bootstrap(&client_hello()).unwrap().to_vec();
    trailing.push(0);
    assert!(matches!(
        decode_bootstrap(&trailing),
        Err(BootstrapDecodeError::TrailingBytes)
    ));
}

#[test]
fn capability_selection_rejects_required_and_ignores_unknown_optional_bits() {
    assert_eq!(
        select_capabilities(
            CAP_RESUME,
            CAP_UDP_DATA | (1 << 63),
            CAP_RESUME | CAP_UDP_DATA
        )
        .unwrap(),
        CAP_RESUME | CAP_UDP_DATA
    );
    assert_eq!(
        select_capabilities(CAP_UDP_DATA, 0, CAP_TCP_DATA).unwrap_err(),
        CapabilityError::MissingRequired(CAP_UDP_DATA)
    );
}

#[test]
fn protocol_selection_chooses_newest_common_version() {
    let client = [
        ProtocolRange {
            major: 2,
            min_minor: 0,
            max_minor: 3,
        },
        ProtocolRange {
            major: 3,
            min_minor: 0,
            max_minor: 0,
        },
    ];
    let relay = [ProtocolRange {
        major: 2,
        min_minor: 1,
        max_minor: 2,
    }];
    assert_eq!(
        select_protocol(&client, &relay).unwrap(),
        tractor_beam_relay_protocol::v2::ProtocolVersion { major: 2, minor: 2 }
    );
    assert!(select_protocol(&client[1..], &relay).is_err());
}

#[test]
fn direction_specific_control_json_matches_golden_fixtures() {
    let client = ClientControl::JoinBegin {
        session_credential: SecretString::new("3mJr7AoUXx2Wqd"),
        steam_id64: 76_561_198_000_000_001,
        display_name: Some("Alice".to_owned()),
        data_profile: DataProfile::Udp,
    };
    let client_json = include_bytes!("fixtures/v2/client-join.json")
        .strip_suffix(b"\n")
        .unwrap_or(include_bytes!("fixtures/v2/client-join.json"));
    assert_eq!(
        encode_client_control(&client).unwrap().as_ref(),
        client_json
    );
    assert_eq!(decode_client_control(client_json).unwrap(), client);

    let server_json = include_bytes!("fixtures/v2/server-ready.json")
        .strip_suffix(b"\n")
        .unwrap_or(include_bytes!("fixtures/v2/server-ready.json"));
    let server = decode_server_control(server_json).unwrap();
    assert_eq!(
        encode_server_control(&server).unwrap().as_ref(),
        server_json
    );
}

#[test]
fn control_secrets_are_redacted_from_debug_output() {
    let secret = SecretString::new("must-not-appear");
    assert_eq!(secret.expose_secret(), "must-not-appear");
    let rendered = format!("{secret:?}");
    assert!(!rendered.contains("must-not-appear"));
    assert!(rendered.contains("REDACTED"));
}

#[test]
fn control_json_is_bounded_before_parsing() {
    let oversized = vec![b' '; MAX_CONTROL_PAYLOAD + 1];
    assert!(decode_client_control(&oversized).is_err());
    assert!(decode_server_control(&oversized).is_err());
}

fn data_frame() -> DataFrame {
    DataFrame {
        connection_id: 0x0102_0304_0506_0708,
        frame_id: 0x1112_1314_1516_1718,
        from_steam_id64: 0x2122_2324_2526_2728,
        to_steam_id64: 0x3132_3334_3536_3738,
        source_sequence: 0xa1a2_a3a4,
        channel: -2,
        send_type: 3,
        payload: Bytes::from_static(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]),
    }
}

#[test]
fn data_frame_matches_golden_fixture_and_has_sixty_byte_overhead() {
    let expected = fixture_hex("data-16");
    let encoded = data_frame().encode().unwrap();

    assert_eq!(COMMON_HEADER_LEN, 16);
    assert_eq!(DATA_FRAME_HEADER_LEN, 60);
    assert_eq!(DATA_FRAME_OVERHEAD, 60);
    assert_eq!(encoded.len(), DATA_FRAME_OVERHEAD + 16);
    assert_eq!(encoded.as_ref(), expected);
    assert_eq!(
        decode_frame(Bytes::from(expected)).unwrap(),
        Frame::Data(data_frame())
    );
}

#[test]
fn data_frame_accepts_maximum_payload_and_rejects_one_byte_more() {
    let mut frame = data_frame();
    frame.payload = Bytes::from(vec![0; IPV4_SAFE_DATA_PAYLOAD]);
    let encoded = frame.encode().unwrap();
    assert_eq!(encoded.len(), 1_472);
    assert_eq!(decode_frame(encoded).unwrap(), Frame::Data(frame.clone()));

    frame.payload = Bytes::from(vec![0; IPV4_SAFE_DATA_PAYLOAD + 1]);
    assert!(frame.encode().is_err());
}

#[test]
fn probe_frame_is_fixed_binary_and_round_trips() {
    let probe = ProbeFrame {
        connection_id: 0x0102_0304_0506_0708,
        probe_id: 0x1112_1314_1516_1718,
        from_steam_id64: 0x2122_2324_2526_2728,
        to_steam_id64: 0x3132_3334_3536_3738,
        phase: ProbePhase::Request,
    };
    let encoded = probe.encode().unwrap();

    assert_eq!(PROBE_FRAME_HEADER_LEN, 56);
    assert_eq!(encoded.len(), PROBE_FRAME_HEADER_LEN);
    assert_eq!(decode_frame(encoded).unwrap(), Frame::Probe(probe));
}

#[test]
fn probe_frame_rejects_zero_id_and_reserved_bytes() {
    let probe = ProbeFrame {
        connection_id: 1,
        probe_id: 1,
        from_steam_id64: 2,
        to_steam_id64: 3,
        phase: ProbePhase::Echo,
    };
    let mut zero_id = probe;
    zero_id.probe_id = 0;
    assert!(zero_id.encode().is_err());

    let mut reserved = probe.encode().unwrap().to_vec();
    reserved[PROBE_FRAME_HEADER_LEN - 1] = 1;
    assert_eq!(
        decode_frame(Bytes::from(reserved)).unwrap_err(),
        FrameDecodeError::NonZeroProbeReserved
    );
}

#[test]
fn frame_rejects_bad_wire_boundaries() {
    let good = data_frame().encode().unwrap();

    let mut bad_magic = good.to_vec();
    bad_magic[0] = b'X';
    assert_eq!(
        decode_frame(Bytes::from(bad_magic)).unwrap_err(),
        FrameDecodeError::BadMagic
    );

    let mut bad_major = good.to_vec();
    bad_major[4] = 3;
    assert_eq!(
        decode_frame(Bytes::from(bad_major)).unwrap_err(),
        FrameDecodeError::UnsupportedMajor(3)
    );

    let mut flags = good.to_vec();
    flags[7] = 1;
    assert_eq!(
        decode_frame(Bytes::from(flags)).unwrap_err(),
        FrameDecodeError::UnsupportedFlags(1)
    );

    let mut reserved = good.to_vec();
    reserved[11] = 1;
    assert_eq!(
        decode_frame(Bytes::from(reserved)).unwrap_err(),
        FrameDecodeError::NonZeroReserved(1)
    );

    let mut short_header = good.to_vec();
    short_header[8..10].copy_from_slice(&16_u16.to_be_bytes());
    assert_eq!(
        decode_frame(Bytes::from(short_header)).unwrap_err(),
        FrameDecodeError::BadHeaderLength(16)
    );

    let mut trailing = good.to_vec();
    trailing.push(0);
    assert_eq!(
        decode_frame(Bytes::from(trailing)).unwrap_err(),
        FrameDecodeError::TrailingBytes
    );
}

#[test]
fn frame_id_window_handles_duplicates_reordering_and_expiry() {
    let mut window = FrameIdWindow::new();
    assert_eq!(window.observe(100), DuplicateDecision::New);
    assert_eq!(window.observe(102), DuplicateDecision::New);
    assert_eq!(window.observe(101), DuplicateDecision::Reordered);
    assert_eq!(window.observe(101), DuplicateDecision::Duplicate);
    assert_eq!(window.observe(230), DuplicateDecision::New);
    assert_eq!(window.observe(102), DuplicateDecision::TooOld);
    assert_eq!(window.highest(), Some(230));
}

#[test]
fn data_codec_hot_loop_remains_fixed_binary() {
    let frame = data_frame();
    for _ in 0..10_000 {
        let encoded = frame.encode().unwrap();
        assert!(matches!(decode_frame(encoded).unwrap(), Frame::Data(_)));
    }
}
