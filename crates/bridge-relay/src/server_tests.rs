use std::{io, net::SocketAddr, time::Duration};

use basement_bridge_core::protocol::{ControlMessage, Envelope, GamePacket, MessageType};
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

async fn send_join(peer: &mut TestPeer, room: &str, steam_id64: &str, challenge: Option<String>) {
    let message = ControlMessage::Join {
        room: room.to_owned(),
        steam_id64: steam_id64.to_owned(),
        display_name: None,
        challenge,
    };
    let payload = message.encode().unwrap();
    let bytes = Envelope::new(MessageType::Join, payload).encode().unwrap();
    peer.send_raw(bytes).await;
}

async fn recv_control(peer: &mut TestPeer) -> ControlMessage {
    let raw = peer.recv_raw().await;
    let envelope = Envelope::decode(raw).unwrap();
    ControlMessage::decode(&envelope.payload).unwrap()
}

async fn send_game(peer: &mut TestPeer, from_steam_id64: &str, to_steam_id64: u64, payload: Bytes) {
    let game = GamePacket {
        from_steam_id64: from_steam_id64.to_owned(),
        to_steam_id64,
        source_sequence: 1,
        channel: 0,
        send_type: 0,
        payload,
    };
    let payload = game.encode().unwrap();
    let bytes = Envelope::new(MessageType::Data, payload).encode().unwrap();
    peer.send_raw(bytes).await;
}

async fn recv_game(peer: &mut TestPeer) -> GamePacket {
    let raw = peer.recv_raw().await;
    let envelope = Envelope::decode(raw).unwrap();
    assert_eq!(envelope.message_type, MessageType::Data);
    GamePacket::decode(&envelope.payload).unwrap()
}
