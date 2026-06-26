use std::{
    io,
    net::SocketAddr,
    time::{Duration, Instant},
};

use basement_bridge_core::{
    protocol::{ControlMessage, Envelope, GamePacket, MessageType, UdpFecControl},
    udp_fec::{UdpFecDecoder, UdpFecEncoder, UdpFecProfile, UdpFecProfileName},
};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    time::timeout,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use super::*;

#[tokio::test]
async fn forwards_udp_data_to_target_peer_only() {
    let (server, udp_address, _) = spawn_test_relay(false).await;
    let mut peer_a = TestPeer::udp(udp_address).await;
    let mut peer_b = TestPeer::udp(udp_address).await;
    let mut peer_c = TestPeer::udp(udp_address).await;
    join_peer(&mut peer_a, "room", "76561198000000101").await;
    join_peer(&mut peer_b, "room", "76561198000000102").await;
    join_peer(&mut peer_c, "room", "76561198000000103").await;

    assert_forwards_to_target_only(&mut peer_a, &mut peer_b, &mut peer_c).await;

    server.abort();
}

#[tokio::test]
async fn forwards_tcp_data_to_target_peer_only() {
    let (server, _, tcp_address) = spawn_test_relay(true).await;
    let tcp_address = tcp_address.unwrap();
    let mut peer_a = TestPeer::tcp(tcp_address).await;
    let mut peer_b = TestPeer::tcp(tcp_address).await;
    let mut peer_c = TestPeer::tcp(tcp_address).await;
    join_peer(&mut peer_a, "room", "76561198000000101").await;
    join_peer(&mut peer_b, "room", "76561198000000102").await;
    join_peer(&mut peer_c, "room", "76561198000000103").await;

    assert_forwards_to_target_only(&mut peer_a, &mut peer_b, &mut peer_c).await;

    server.abort();
}

#[tokio::test]
async fn forwards_between_udp_and_tcp_peers() {
    let (server, udp_address, tcp_address) = spawn_test_relay(true).await;
    let mut peer_a = TestPeer::udp(udp_address).await;
    let mut peer_b = TestPeer::tcp(tcp_address.unwrap()).await;
    let mut peer_c = TestPeer::udp(udp_address).await;
    join_peer(&mut peer_a, "room", "76561198000000101").await;
    join_peer(&mut peer_b, "room", "76561198000000102").await;
    join_peer(&mut peer_c, "room", "76561198000000103").await;

    assert_forwards_to_target_only(&mut peer_a, &mut peer_b, &mut peer_c).await;

    server.abort();
}

#[tokio::test]
async fn forwards_plain_udp_data_to_udp_fec_peer() {
    let (server, udp_address, _) = spawn_test_relay(false).await;
    let mut peer_a = TestPeer::udp(udp_address).await;
    let mut peer_b = TestPeer::udp(udp_address).await;
    join_peer(&mut peer_a, "room", "76561198000000101").await;
    join_peer_with_udp_fec(&mut peer_b, "room", "76561198000000102").await;

    let payload = Bytes::from(vec![7; 128]);
    send_game(
        &mut peer_a,
        "76561198000000101",
        76_561_198_000_000_102,
        payload.clone(),
    )
    .await;

    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let mut decoder = UdpFecDecoder::new(profile);
    let game = recv_fec_game(&mut peer_b, &mut decoder).await;
    assert_eq!(game.from_steam_id64, "76561198000000101");
    assert_eq!(game.to_steam_id64, 76_561_198_000_000_102);
    assert_eq!(game.payload, payload);
    assert_eq!(decoder.snapshot().original_packets, 1);

    server.abort();
}

#[tokio::test]
async fn plain_udp_rejoin_clears_stale_fec_egress() {
    let (server, udp_address, _) = spawn_test_relay(false).await;
    let mut peer_a = TestPeer::udp(udp_address).await;
    let mut peer_b = TestPeer::udp(udp_address).await;
    join_peer(&mut peer_a, "room", "76561198000000101").await;
    join_peer_with_udp_fec(&mut peer_b, "room", "76561198000000102").await;

    let fec_payload = Bytes::from(vec![7; 128]);
    send_game(
        &mut peer_a,
        "76561198000000101",
        76_561_198_000_000_102,
        fec_payload.clone(),
    )
    .await;
    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let mut decoder = UdpFecDecoder::new(profile);
    assert_eq!(
        recv_fec_game(&mut peer_b, &mut decoder).await.payload,
        fec_payload
    );

    join_peer(&mut peer_b, "room", "76561198000000102").await;
    let plain_payload = Bytes::from(vec![11; 128]);
    send_game(
        &mut peer_a,
        "76561198000000101",
        76_561_198_000_000_102,
        plain_payload.clone(),
    )
    .await;

    let game = recv_game(&mut peer_b).await;
    assert_eq!(game.from_steam_id64, "76561198000000101");
    assert_eq!(game.to_steam_id64, 76_561_198_000_000_102);
    assert_eq!(game.payload, plain_payload);

    server.abort();
}

#[tokio::test]
async fn forwards_udp_fec_data_to_plain_udp_peer() {
    let (server, udp_address, _) = spawn_test_relay(false).await;
    let mut peer_a = TestPeer::udp(udp_address).await;
    let mut peer_b = TestPeer::udp(udp_address).await;
    join_peer_with_udp_fec(&mut peer_a, "room", "76561198000000101").await;
    join_peer(&mut peer_b, "room", "76561198000000102").await;

    let payload = Bytes::from(vec![8; 128]);
    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let mut encoder = UdpFecEncoder::new(profile);
    send_fec_game(
        &mut peer_a,
        &mut encoder,
        "76561198000000101",
        76_561_198_000_000_102,
        payload.clone(),
    )
    .await;

    let game = recv_game(&mut peer_b).await;
    assert_eq!(game.from_steam_id64, "76561198000000101");
    assert_eq!(game.to_steam_id64, 76_561_198_000_000_102);
    assert_eq!(game.payload, payload);

    server.abort();
}

#[tokio::test]
async fn forwards_udp_fec_data_to_tcp_peer() {
    let (server, udp_address, tcp_address) = spawn_test_relay(true).await;
    let mut peer_a = TestPeer::udp(udp_address).await;
    let mut peer_b = TestPeer::tcp(tcp_address.unwrap()).await;
    join_peer_with_udp_fec(&mut peer_a, "room", "76561198000000101").await;
    join_peer(&mut peer_b, "room", "76561198000000102").await;

    let payload = Bytes::from(vec![9; 128]);
    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let mut encoder = UdpFecEncoder::new(profile);
    send_fec_game(
        &mut peer_a,
        &mut encoder,
        "76561198000000101",
        76_561_198_000_000_102,
        payload.clone(),
    )
    .await;

    let game = recv_game(&mut peer_b).await;
    assert_eq!(game.from_steam_id64, "76561198000000101");
    assert_eq!(game.to_steam_id64, 76_561_198_000_000_102);
    assert_eq!(game.payload, payload);

    server.abort();
}

#[tokio::test]
async fn forwards_tcp_data_to_udp_fec_peer() {
    let (server, udp_address, tcp_address) = spawn_test_relay(true).await;
    let mut peer_a = TestPeer::tcp(tcp_address.unwrap()).await;
    let mut peer_b = TestPeer::udp(udp_address).await;
    join_peer(&mut peer_a, "room", "76561198000000101").await;
    join_peer_with_udp_fec(&mut peer_b, "room", "76561198000000102").await;

    let payload = Bytes::from(vec![10; 128]);
    send_game(
        &mut peer_a,
        "76561198000000101",
        76_561_198_000_000_102,
        payload.clone(),
    )
    .await;

    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let mut decoder = UdpFecDecoder::new(profile);
    let game = recv_fec_game(&mut peer_b, &mut decoder).await;
    assert_eq!(game.from_steam_id64, "76561198000000101");
    assert_eq!(game.to_steam_id64, 76_561_198_000_000_102);
    assert_eq!(game.payload, payload);

    server.abort();
}

#[tokio::test]
async fn answers_health_ping_to_source_only() {
    let (server, udp_address, _) = spawn_test_relay(false).await;
    let mut peer_a = TestPeer::udp(udp_address).await;
    let mut peer_b = TestPeer::udp(udp_address).await;
    join_peer(&mut peer_a, "room", "76561198000000101").await;
    join_peer(&mut peer_b, "room", "76561198000000102").await;

    send_control(
        &mut peer_a,
        MessageType::Heartbeat,
        &ControlMessage::HealthPing { id: 42 },
    )
    .await;

    assert!(matches!(
        recv_control(&mut peer_a).await,
        ControlMessage::HealthPong { id: 42 }
    ));
    assert!(
        timeout(Duration::from_millis(150), peer_b.recv_raw())
            .await
            .is_err()
    );

    server.abort();
}

#[test]
fn tcp_egress_channel_uses_configured_capacity() {
    let (sender, _receiver) = tcp_egress_channel(1);

    assert!(sender.try_send(Bytes::from_static(b"one")).is_ok());
    assert!(sender.try_send(Bytes::from_static(b"two")).is_err());
}

#[test]
fn queue_full_metrics_are_separate_from_packet_errors() {
    let mut metrics = RelayMetrics::new(512);

    metrics.add(PacketOutcome {
        tcp_egress_queue_full: 1,
        tcp_egress_dropped_packets: 1,
        ..PacketOutcome::default()
    });

    assert_eq!(metrics.tcp_egress_queue_capacity, 512);
    assert_eq!(metrics.tcp_egress_queue_full, 1);
    assert_eq!(metrics.tcp_egress_dropped_packets, 1);
    assert_eq!(metrics.packet_handling_errors, 0);
}

#[test]
fn room_metrics_track_attributed_packet_outcomes() {
    let mut metrics = RelayMetrics::new(512);

    metrics.record_packet_in();
    metrics.add(PacketOutcome {
        room: Some("room-a".to_owned()),
        room_packets_in: 1,
        data_in: 1,
        forwarded_packets: 1,
        forwarded_bytes: 128,
        missing_target: 1,
        ..PacketOutcome::default()
    });
    metrics.record_rate_limited(Some("room-a"));

    let room = metrics.room_metrics.get("room-a").unwrap();
    assert_eq!(metrics.packets_in, 1);
    assert_eq!(metrics.data_in, 1);
    assert_eq!(metrics.rate_limited, 1);
    assert_eq!(room.packets_in, 2);
    assert_eq!(room.data_in, 1);
    assert_eq!(room.forwarded_packets, 1);
    assert_eq!(room.forwarded_bytes, 128);
    assert_eq!(room.missing_target, 1);
    assert_eq!(room.rate_limited, 1);
}

async fn spawn_test_relay(
    tcp_enabled: bool,
) -> (
    tokio::task::JoinHandle<io::Result<()>>,
    SocketAddr,
    Option<SocketAddr>,
) {
    let udp_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let udp_address = udp_socket.local_addr().unwrap();
    let tcp_listener = if tcp_enabled {
        Some(TcpListener::bind("127.0.0.1:0").await.unwrap())
    } else {
        None
    };
    let tcp_address = tcp_listener
        .as_ref()
        .map(|listener| listener.local_addr().unwrap());
    let config = RelayConfig {
        bind: udp_address.to_string(),
        tcp_enabled,
        tcp_bind: tcp_address
            .map(|address| address.to_string())
            .unwrap_or_else(|| "127.0.0.1:0".to_owned()),
        ..RelayConfig::default()
    };
    let server = tokio::spawn(run_with_listeners(udp_socket, tcp_listener, config));
    (server, udp_address, tcp_address)
}

async fn assert_forwards_to_target_only(
    peer_a: &mut TestPeer,
    peer_b: &mut TestPeer,
    peer_c: &mut TestPeer,
) {
    let payload = Bytes::from(vec![7; 2_048]);
    send_game(
        peer_a,
        "76561198000000101",
        76_561_198_000_000_102,
        payload.clone(),
    )
    .await;

    let game = recv_game(peer_b).await;
    assert_eq!(game.from_steam_id64, "76561198000000101");
    assert_eq!(game.to_steam_id64, 76_561_198_000_000_102);
    assert_eq!(game.payload, payload);
    assert!(
        timeout(Duration::from_millis(150), recv_game(peer_c))
            .await
            .is_err()
    );
}

enum TestPeer {
    Udp {
        socket: UdpSocket,
        relay_address: SocketAddr,
    },
    Tcp(Framed<TcpStream, LengthDelimitedCodec>),
}

impl TestPeer {
    async fn udp(relay_address: SocketAddr) -> Self {
        Self::Udp {
            socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
            relay_address,
        }
    }

    async fn tcp(relay_address: SocketAddr) -> Self {
        let stream = TcpStream::connect(relay_address).await.unwrap();
        let framed = LengthDelimitedCodec::builder()
            .max_frame_length(65_535)
            .new_framed(stream);
        Self::Tcp(framed)
    }

    async fn send_raw(&mut self, bytes: Bytes) {
        match self {
            Self::Udp {
                socket,
                relay_address,
            } => {
                socket.send_to(&bytes, *relay_address).await.unwrap();
            }
            Self::Tcp(framed) => framed.send(bytes).await.unwrap(),
        }
    }

    async fn recv_raw(&mut self) -> Bytes {
        match self {
            Self::Udp { socket, .. } => {
                let mut buffer = [0_u8; 65_535];
                let (size, _) = timeout(Duration::from_secs(1), socket.recv_from(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
                Bytes::copy_from_slice(&buffer[..size])
            }
            Self::Tcp(framed) => timeout(Duration::from_secs(1), framed.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .freeze(),
        }
    }
}

async fn join_peer(peer: &mut TestPeer, room: &str, steam_id64: &str) {
    send_join(peer, room, steam_id64, None).await;
    let challenge = match recv_control(peer).await {
        ControlMessage::Challenge { token } => token,
        other => panic!("expected challenge, got {other:?}"),
    };
    send_join(peer, room, steam_id64, Some(challenge)).await;
    assert!(matches!(
        recv_control(peer).await,
        ControlMessage::Ready { .. }
    ));
}

async fn join_peer_with_udp_fec(peer: &mut TestPeer, room: &str, steam_id64: &str) {
    let fec = Some(UdpFecControl {
        profile: UdpFecProfileName::Rs8_2_4ms,
    });
    send_join_with_udp_fec(peer, room, steam_id64, None, fec).await;
    let challenge = match recv_control(peer).await {
        ControlMessage::Challenge { token } => token,
        other => panic!("expected challenge, got {other:?}"),
    };
    send_join_with_udp_fec(peer, room, steam_id64, Some(challenge), fec).await;
    assert!(matches!(
        recv_control(peer).await,
        ControlMessage::Ready {
            udp_fec: Some(_),
            ..
        }
    ));
}

async fn send_join(peer: &mut TestPeer, room: &str, steam_id64: &str, challenge: Option<String>) {
    send_join_with_udp_fec(peer, room, steam_id64, challenge, None).await;
}

async fn send_join_with_udp_fec(
    peer: &mut TestPeer,
    room: &str,
    steam_id64: &str,
    challenge: Option<String>,
    udp_fec: Option<UdpFecControl>,
) {
    let message = ControlMessage::Join {
        room: room.to_owned(),
        steam_id64: steam_id64.to_owned(),
        display_name: None,
        challenge,
        udp_fec,
    };
    send_control(peer, MessageType::Join, &message).await;
}

async fn send_control(peer: &mut TestPeer, message_type: MessageType, message: &ControlMessage) {
    let payload = message.encode().unwrap();
    let bytes = Envelope::new(message_type, payload).encode().unwrap();
    peer.send_raw(bytes).await;
}

async fn recv_control(peer: &mut TestPeer) -> ControlMessage {
    let raw = peer.recv_raw().await;
    let envelope = Envelope::decode(raw).unwrap();
    ControlMessage::decode(&envelope.payload).unwrap()
}

async fn send_game(peer: &mut TestPeer, from_steam_id64: &str, to_steam_id64: u64, payload: Bytes) {
    peer.send_raw(game_datagram(from_steam_id64, to_steam_id64, payload))
        .await;
}

async fn send_fec_game(
    peer: &mut TestPeer,
    encoder: &mut UdpFecEncoder,
    from_steam_id64: &str,
    to_steam_id64: u64,
    payload: Bytes,
) {
    let raw = game_datagram(from_steam_id64, to_steam_id64, payload);
    let frames = encoder.encode_or_passthrough(raw, Instant::now()).unwrap();
    for frame in frames {
        peer.send_raw(frame).await;
    }
}

fn game_datagram(from_steam_id64: &str, to_steam_id64: u64, payload: Bytes) -> Bytes {
    let game = GamePacket {
        from_steam_id64: from_steam_id64.to_owned(),
        to_steam_id64,
        source_sequence: 1,
        channel: 0,
        send_type: 0,
        payload,
    };
    let payload = game.encode().unwrap();
    Envelope::new(MessageType::Data, payload).encode().unwrap()
}

async fn recv_game(peer: &mut TestPeer) -> GamePacket {
    let raw = peer.recv_raw().await;
    let envelope = Envelope::decode(raw).unwrap();
    assert_eq!(envelope.message_type, MessageType::Data);
    GamePacket::decode(&envelope.payload).unwrap()
}

async fn recv_fec_game(peer: &mut TestPeer, decoder: &mut UdpFecDecoder) -> GamePacket {
    loop {
        let raw = peer.recv_raw().await;
        let datagrams = decoder.decode(raw, std::time::Instant::now()).unwrap();
        for datagram in datagrams {
            let envelope = Envelope::decode(datagram).unwrap();
            if envelope.message_type == MessageType::Data {
                return GamePacket::decode(&envelope.payload).unwrap();
            }
        }
    }
}
