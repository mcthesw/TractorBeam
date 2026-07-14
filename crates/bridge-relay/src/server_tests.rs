use std::{net::SocketAddr, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::{
    io::AsyncWriteExt as _,
    net::{TcpListener, TcpStream, UdpSocket},
    time,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use super::control::read_bootstrap;
use super::{SharedMetrics, run_with_listeners};
use crate::{config::RelayConfig, metrics::RelayMetrics};
use tractor_beam_relay_protocol::{
    BOOTSTRAP_SCHEMA, BootstrapMessage, BuildMetadata, CAP_RESUME, CAP_ROOM_PATH_PROBE,
    CAP_TCP_DATA, CAP_UDP_DATA, ClientControl, CompatibilityReject, DataProfile, Frame, ProbeFrame,
    ProbePhase, ProtocolRange, ProtocolVersion, RejectCode, SecretString, ServerControl,
    decode_bootstrap, decode_frame, decode_server_control, encode_bootstrap, encode_client_control,
};

fn test_metrics() -> SharedMetrics {
    Arc::new(RelayMetrics::new(&opentelemetry::global::meter(
        "tractor-beam-relay-test",
    )))
}

#[tokio::test]
async fn real_tcp_socket_negotiates_and_joins_v2() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = RelayConfig {
        pow_difficulty_bits: 0,
        udp_bind: None,
        ..RelayConfig::default()
    };
    let server = tokio::spawn(run_with_listeners(listener, None, config, test_metrics()));

    let mut stream = TcpStream::connect(address).await.unwrap();
    let hello = BootstrapMessage::ClientHello {
        bootstrap_schema: BOOTSTRAP_SCHEMA,
        supported_protocol_ranges: vec![ProtocolRange {
            major: 2,
            min_minor: 0,
            max_minor: 0,
        }],
        required_capabilities: CAP_TCP_DATA,
        optional_capabilities: CAP_RESUME,
        client: BuildMetadata {
            version: "test".into(),
            git_hash: None,
        },
    };
    stream
        .write_all(&encode_bootstrap(&hello).unwrap())
        .await
        .unwrap();
    let response = decode_bootstrap(&read_bootstrap(&mut stream).await.unwrap()).unwrap();
    assert!(matches!(
        response,
        BootstrapMessage::ServerHello {
            selected_protocol: ProtocolVersion { major: 2, minor: 0 },
            ..
        }
    ));

    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
    send_client_control(
        &mut framed,
        &ClientControl::JoinBegin {
            session_credential: SecretString::new("1111111111111111"),
            steam_id64: 101,
            display_name: Some("Test".into()),
            data_profile: DataProfile::Tcp,
        },
    )
    .await;
    let challenge = receive_server_control(&mut framed).await;
    let ServerControl::AdmissionChallenge { challenge_id, .. } = challenge else {
        panic!("expected admission challenge");
    };
    send_client_control(
        &mut framed,
        &ClientControl::JoinProof {
            challenge_id,
            proof: SecretString::new(""),
        },
    )
    .await;
    assert!(matches!(
        receive_server_control(&mut framed).await,
        ServerControl::JoinReady { peers, .. } if peers.len() == 1 && peers[0].steam_id64 == 101
    ));
    server.abort();
}

#[tokio::test]
async fn real_tcp_socket_returns_structured_bootstrap_rejection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = RelayConfig {
        udp_bind: None,
        ..RelayConfig::default()
    };
    let server = tokio::spawn(run_with_listeners(listener, None, config, test_metrics()));

    let mut stream = TcpStream::connect(address).await.unwrap();
    let hello = BootstrapMessage::ClientHello {
        bootstrap_schema: BOOTSTRAP_SCHEMA + 1,
        supported_protocol_ranges: vec![ProtocolRange {
            major: 2,
            min_minor: 0,
            max_minor: 0,
        }],
        required_capabilities: CAP_TCP_DATA,
        optional_capabilities: CAP_RESUME,
        client: BuildMetadata {
            version: "incompatible-test".into(),
            git_hash: None,
        },
    };
    stream
        .write_all(&encode_bootstrap(&hello).unwrap())
        .await
        .unwrap();

    let response = decode_bootstrap(&read_bootstrap(&mut stream).await.unwrap()).unwrap();
    assert!(matches!(
        response,
        BootstrapMessage::CompatibilityReject(CompatibilityReject {
            code: RejectCode::UnsupportedBootstrapSchema,
            ..
        })
    ));
    server.abort();
}

#[tokio::test]
async fn real_tcp_socket_forwards_probe_between_capable_peers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = RelayConfig {
        pow_difficulty_bits: 0,
        udp_bind: None,
        ..RelayConfig::default()
    };
    let server = tokio::spawn(run_with_listeners(listener, None, config, test_metrics()));
    let (mut first, first_connection_id) = connect_joined_peer(address, 101).await;
    let (mut second, _) = connect_joined_peer(address, 202).await;

    let probe = ProbeFrame {
        connection_id: first_connection_id,
        probe_id: 7,
        from_steam_id64: 101,
        to_steam_id64: 202,
        phase: ProbePhase::Request,
    };
    first
        .send(Frame::Probe(probe).encode().unwrap())
        .await
        .unwrap();
    let received = loop {
        let raw = second.next().await.unwrap().unwrap().freeze();
        if let Frame::Probe(probe) = decode_frame(raw).unwrap() {
            break probe;
        }
    };
    assert_eq!(received, probe);
    server.abort();
}

#[tokio::test]
async fn rejected_tcp_probe_does_not_close_control_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = RelayConfig {
        pow_difficulty_bits: 0,
        udp_bind: None,
        ..RelayConfig::default()
    };
    let server = tokio::spawn(run_with_listeners(listener, None, config, test_metrics()));
    let (mut peer, connection_id) = connect_joined_peer(address, 101).await;

    peer.send(
        Frame::Probe(ProbeFrame {
            connection_id,
            probe_id: 8,
            from_steam_id64: 101,
            to_steam_id64: 999,
            phase: ProbePhase::Request,
        })
        .encode()
        .unwrap(),
    )
    .await
    .unwrap();
    send_client_control(&mut peer, &ClientControl::ControlPing { id: 42 }).await;

    assert!(matches!(
        time::timeout(Duration::from_secs(1), receive_server_control(&mut peer))
            .await
            .unwrap(),
        ServerControl::ControlPong { id: 42 }
    ));
    server.abort();
}

#[tokio::test]
async fn real_udp_socket_forwards_probe_without_tcp_fallback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_address = listener.local_addr().unwrap();
    let relay_udp = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let udp_address = relay_udp.local_addr().unwrap();
    let config = RelayConfig {
        pow_difficulty_bits: 0,
        udp_bind: Some(udp_address.to_string()),
        ..RelayConfig::default()
    };
    let server = tokio::spawn(run_with_listeners(
        listener,
        Some(relay_udp),
        config,
        test_metrics(),
    ));
    let (_first_control, first_udp, first_connection_id) =
        connect_joined_udp_peer(tcp_address, udp_address, 301).await;
    let (_second_control, second_udp, _) =
        connect_joined_udp_peer(tcp_address, udp_address, 302).await;

    let probe = ProbeFrame {
        connection_id: first_connection_id,
        probe_id: 9,
        from_steam_id64: 301,
        to_steam_id64: 302,
        phase: ProbePhase::Request,
    };
    first_udp.send(&probe.encode().unwrap()).await.unwrap();
    let mut buffer = [0_u8; 128];
    let size = time::timeout(Duration::from_secs(1), second_udp.recv(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        decode_frame(Bytes::copy_from_slice(&buffer[..size])).unwrap(),
        Frame::Probe(probe)
    );
    server.abort();
}

async fn connect_joined_peer(
    address: SocketAddr,
    steam_id64: u64,
) -> (Framed<TcpStream, LengthDelimitedCodec>, u64) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let hello = BootstrapMessage::ClientHello {
        bootstrap_schema: BOOTSTRAP_SCHEMA,
        supported_protocol_ranges: vec![ProtocolRange {
            major: 2,
            min_minor: 0,
            max_minor: 0,
        }],
        required_capabilities: CAP_TCP_DATA,
        optional_capabilities: CAP_RESUME | CAP_ROOM_PATH_PROBE,
        client: BuildMetadata {
            version: "probe-test".into(),
            git_hash: None,
        },
    };
    stream
        .write_all(&encode_bootstrap(&hello).unwrap())
        .await
        .unwrap();
    let response = decode_bootstrap(&read_bootstrap(&mut stream).await.unwrap()).unwrap();
    assert!(matches!(
        response,
        BootstrapMessage::ServerHello { enabled_capabilities, .. }
            if enabled_capabilities & CAP_ROOM_PATH_PROBE != 0
    ));
    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
    send_client_control(
        &mut framed,
        &ClientControl::JoinBegin {
            session_credential: SecretString::new("1111111111111111"),
            steam_id64,
            display_name: None,
            data_profile: DataProfile::Tcp,
        },
    )
    .await;
    let ServerControl::AdmissionChallenge { challenge_id, .. } =
        receive_server_control(&mut framed).await
    else {
        panic!("expected challenge")
    };
    send_client_control(
        &mut framed,
        &ClientControl::JoinProof {
            challenge_id,
            proof: SecretString::new(""),
        },
    )
    .await;
    let ready = receive_server_control(&mut framed).await;
    let ServerControl::JoinReady { connection_id, .. } = ready else {
        panic!("expected join ready")
    };
    (framed, connection_id)
}

async fn connect_joined_udp_peer(
    tcp_address: SocketAddr,
    udp_address: SocketAddr,
    steam_id64: u64,
) -> (Framed<TcpStream, LengthDelimitedCodec>, UdpSocket, u64) {
    let mut stream = TcpStream::connect(tcp_address).await.unwrap();
    let hello = BootstrapMessage::ClientHello {
        bootstrap_schema: BOOTSTRAP_SCHEMA,
        supported_protocol_ranges: vec![ProtocolRange {
            major: 2,
            min_minor: 0,
            max_minor: 0,
        }],
        required_capabilities: CAP_UDP_DATA,
        optional_capabilities: CAP_RESUME | CAP_ROOM_PATH_PROBE,
        client: BuildMetadata {
            version: "udp-probe-test".into(),
            git_hash: None,
        },
    };
    stream
        .write_all(&encode_bootstrap(&hello).unwrap())
        .await
        .unwrap();
    let response = decode_bootstrap(&read_bootstrap(&mut stream).await.unwrap()).unwrap();
    assert!(matches!(
        response,
        BootstrapMessage::ServerHello { enabled_capabilities, .. }
            if enabled_capabilities & (CAP_UDP_DATA | CAP_ROOM_PATH_PROBE)
                == CAP_UDP_DATA | CAP_ROOM_PATH_PROBE
    ));
    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
    send_client_control(
        &mut framed,
        &ClientControl::JoinBegin {
            session_credential: SecretString::new("1111111111111111"),
            steam_id64,
            display_name: None,
            data_profile: DataProfile::Udp,
        },
    )
    .await;
    let ServerControl::AdmissionChallenge { challenge_id, .. } =
        receive_server_control(&mut framed).await
    else {
        panic!("expected challenge")
    };
    send_client_control(
        &mut framed,
        &ClientControl::JoinProof {
            challenge_id,
            proof: SecretString::new(""),
        },
    )
    .await;
    let ServerControl::JoinReady { connection_id, .. } = receive_server_control(&mut framed).await
    else {
        panic!("expected join ready")
    };
    send_client_control(&mut framed, &ClientControl::UdpPathRequest).await;
    let ServerControl::UdpPathToken { path_token, .. } = receive_server_control(&mut framed).await
    else {
        panic!("expected path token")
    };
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    udp.connect(udp_address).await.unwrap();
    let payload = encode_client_control(&ClientControl::UdpPathHello {
        connection_id,
        path_token,
    })
    .unwrap();
    udp.send(&Frame::ClientControl(payload).encode().unwrap())
        .await
        .unwrap();
    assert!(matches!(
        receive_server_control(&mut framed).await,
        ServerControl::UdpPathReady { connection_id: ready } if ready == connection_id
    ));
    (framed, udp, connection_id)
}

async fn send_client_control(
    framed: &mut Framed<TcpStream, LengthDelimitedCodec>,
    message: &ClientControl,
) {
    let payload = encode_client_control(message).unwrap();
    framed
        .send(Frame::ClientControl(payload).encode().unwrap())
        .await
        .unwrap();
}

async fn receive_server_control(
    framed: &mut Framed<TcpStream, LengthDelimitedCodec>,
) -> ServerControl {
    let raw = framed.next().await.unwrap().unwrap().freeze();
    let Frame::ServerControl(payload) = decode_frame(raw).unwrap() else {
        panic!("expected server control frame");
    };
    decode_server_control(&payload).unwrap()
}
