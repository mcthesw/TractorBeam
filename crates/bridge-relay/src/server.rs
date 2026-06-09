use std::{
    io,
    net::SocketAddr,
    time::{Duration, Instant},
};

use basement_bridge_core::protocol::{ControlMessage, Envelope, GamePacket, MessageType};
use bytes::Bytes;
use tokio::{
    net::UdpSocket,
    time::{self, MissedTickBehavior},
};
use tracing::{debug, info, warn};

use crate::{
    config::RelayConfig,
    state::{RelayState, error_message},
};

pub(crate) async fn run(config: RelayConfig) -> io::Result<()> {
    let socket = UdpSocket::bind(&config.bind).await?;
    info!(
        bind = %config.bind,
        max_packet_size = config.max_packet_size,
        rate_limit_per_second = config.rate_limit_per_second,
        max_rooms = config.max_rooms,
        max_peers_per_room = config.max_peers_per_room,
        blocked_cidrs = config.blocked_cidrs.len(),
        "relay listening"
    );

    run_socket(socket, config).await
}

async fn run_socket(socket: UdpSocket, config: RelayConfig) -> io::Result<()> {
    let mut state = RelayState::new(config.clone());
    let mut buffer = vec![0_u8; config.max_packet_size];
    let mut metrics = RelayMetrics::default();
    let mut stats_interval = time::interval(Duration::from_secs(5));
    stats_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            received = socket.recv_from(&mut buffer) => {
                let (size, address) = received?;
                let now = Instant::now();
                metrics.packets_in = metrics.packets_in.saturating_add(1);
                if state.is_blocked(address) {
                    metrics.blocked = metrics.blocked.saturating_add(1);
                    debug!(%address, "packet rejected by blocklist");
                    continue;
                }
                if !state.allow_packet(address, now) {
                    metrics.rate_limited = metrics.rate_limited.saturating_add(1);
                    debug!(%address, "rate limit exceeded");
                    continue;
                }
                let raw = Bytes::copy_from_slice(&buffer[..size]);
                match handle_packet(&socket, &mut state, address, raw, now).await {
                    Ok(outcome) => metrics.add(outcome),
                    Err(error) => {
                        metrics.errors = metrics.errors.saturating_add(1);
                        warn!(%address, %error, "packet handling failed");
                    }
                }
                state.cleanup(now);
            }
            _ = stats_interval.tick() => {
                let now = Instant::now();
                state.cleanup(now);
                metrics.log_and_reset(&state);
            }
        }
    }
}

#[derive(Debug, Default)]
struct RelayMetrics {
    packets_in: u64,
    data_in: u64,
    forwarded_packets: u64,
    forwarded_bytes: u64,
    decode_errors: u64,
    unjoined_data: u64,
    missing_target: u64,
    blocked: u64,
    rate_limited: u64,
    errors: u64,
}

impl RelayMetrics {
    fn add(&mut self, outcome: PacketOutcome) {
        self.data_in = self.data_in.saturating_add(outcome.data_in);
        self.forwarded_packets = self
            .forwarded_packets
            .saturating_add(outcome.forwarded_packets);
        self.forwarded_bytes = self.forwarded_bytes.saturating_add(outcome.forwarded_bytes);
        self.decode_errors = self.decode_errors.saturating_add(outcome.decode_errors);
        self.unjoined_data = self.unjoined_data.saturating_add(outcome.unjoined_data);
        self.missing_target = self.missing_target.saturating_add(outcome.missing_target);
    }

    fn log_and_reset(&mut self, state: &RelayState) {
        info!(
            rooms = state.room_count(),
            peers = state.peer_count(),
            packets_in = self.packets_in,
            data_in = self.data_in,
            forwarded_packets = self.forwarded_packets,
            forwarded_bytes = self.forwarded_bytes,
            decode_errors = self.decode_errors,
            unjoined_data = self.unjoined_data,
            missing_target = self.missing_target,
            blocked = self.blocked,
            rate_limited = self.rate_limited,
            errors = self.errors,
            "relay stats"
        );
        *self = Self::default();
    }
}

#[derive(Debug, Default)]
struct PacketOutcome {
    data_in: u64,
    forwarded_packets: u64,
    forwarded_bytes: u64,
    decode_errors: u64,
    unjoined_data: u64,
    missing_target: u64,
}

async fn handle_packet(
    socket: &UdpSocket,
    state: &mut RelayState,
    address: SocketAddr,
    raw: Bytes,
    now: Instant,
) -> io::Result<PacketOutcome> {
    let envelope = match Envelope::decode(raw.clone()) {
        Ok(envelope) => envelope,
        Err(error) => {
            send_control(
                socket,
                address,
                MessageType::Error,
                &error_message("decode_error", error.to_string()),
            )
            .await?;
            return Ok(PacketOutcome {
                decode_errors: 1,
                ..PacketOutcome::default()
            });
        }
    };

    match envelope.message_type {
        MessageType::Join => {
            handle_join(socket, state, address, &envelope, now).await?;
            Ok(PacketOutcome::default())
        }
        MessageType::Data => forward_data(socket, state, address, &raw, now).await,
        MessageType::Heartbeat => {
            state.touch_peer(address, now);
            Ok(PacketOutcome::default())
        }
        _ => Ok(PacketOutcome::default()),
    }
}

async fn handle_join(
    socket: &UdpSocket,
    state: &mut RelayState,
    address: SocketAddr,
    envelope: &Envelope,
    now: Instant,
) -> io::Result<()> {
    let message = ControlMessage::decode(&envelope.payload)
        .unwrap_or_else(|error| error_message("bad_join", error.to_string()));
    let response = match message {
        ControlMessage::Join {
            room,
            steam_id64,
            display_name: _,
            challenge: Some(challenge),
        } => state.complete_join(address, room, steam_id64, challenge, now),
        ControlMessage::Join {
            room,
            steam_id64,
            display_name,
            challenge: None,
        } => state.challenge_join(address, room, steam_id64, display_name, now),
        _ => error_message("bad_join", "expected join message"),
    };
    let response_type = match response {
        ControlMessage::Challenge { .. } => MessageType::JoinChallenge,
        ControlMessage::Ready { .. } => MessageType::JoinReady,
        ControlMessage::Error { .. } => MessageType::Error,
        _ => MessageType::Error,
    };
    send_control(socket, address, response_type, &response).await
}

async fn forward_data(
    socket: &UdpSocket,
    state: &mut RelayState,
    address: SocketAddr,
    raw: &Bytes,
    now: Instant,
) -> io::Result<PacketOutcome> {
    let Some(room_name) = state.touch_peer(address, now) else {
        send_control(
            socket,
            address,
            MessageType::Error,
            &error_message("not_joined", "join a room before sending data"),
        )
        .await?;
        return Ok(PacketOutcome {
            unjoined_data: 1,
            ..PacketOutcome::default()
        });
    };
    let mut outcome = PacketOutcome {
        data_in: 1,
        ..PacketOutcome::default()
    };

    let data_envelope = match Envelope::decode(raw.clone()) {
        Ok(envelope) => envelope,
        Err(error) => {
            warn!(%address, %error, "bad data envelope");
            outcome.decode_errors = outcome.decode_errors.saturating_add(1);
            return Ok(outcome);
        }
    };
    let game = match GamePacket::decode(&data_envelope.payload) {
        Ok(packet) => packet,
        Err(error) => {
            warn!(%address, %error, "bad data packet");
            outcome.decode_errors = outcome.decode_errors.saturating_add(1);
            return Ok(outcome);
        }
    };

    let Some(peer_address) = state.target_address(&room_name, game.to_steam_id64) else {
        outcome.missing_target = outcome.missing_target.saturating_add(1);
        debug!(
            %address,
            room = %room_name,
            to_steam_id64 = game.to_steam_id64,
            "data target is not joined"
        );
        return Ok(outcome);
    };

    if peer_address != address {
        socket.send_to(raw, peer_address).await?;
        outcome.forwarded_packets = outcome.forwarded_packets.saturating_add(1);
        outcome.forwarded_bytes = outcome
            .forwarded_bytes
            .saturating_add(u64::try_from(raw.len()).unwrap_or(u64::MAX));
    }

    Ok(outcome)
}

async fn send_control(
    socket: &UdpSocket,
    address: SocketAddr,
    message_type: MessageType,
    message: &ControlMessage,
) -> io::Result<()> {
    let payload = message.encode().map_err(io::Error::other)?;
    let raw = Envelope::new(message_type, payload)
        .encode()
        .map_err(io::Error::other)?;
    socket.send_to(&raw, address).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use basement_bridge_core::protocol::{ControlMessage, Envelope, GamePacket, MessageType};
    use bytes::Bytes;
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn forwards_data_to_target_peer_only() {
        let server_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let relay_address = server_socket.local_addr().unwrap();
        let config = RelayConfig {
            bind: relay_address.to_string(),
            ..RelayConfig::default()
        };
        let server = tokio::spawn(async move {
            let _ = run_socket(server_socket, config).await;
        });

        let peer_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_c = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        join_peer(&peer_a, relay_address, "room", "76561198000000101").await;
        join_peer(&peer_b, relay_address, "room", "76561198000000102").await;
        join_peer(&peer_c, relay_address, "room", "76561198000000103").await;

        let payload = Bytes::from(vec![7; 2_048]);
        send_game(
            &peer_a,
            relay_address,
            "76561198000000101",
            76_561_198_000_000_102,
            payload.clone(),
        )
        .await;

        let game = recv_game(&peer_b).await;
        assert_eq!(game.from_steam_id64, "76561198000000101");
        assert_eq!(game.to_steam_id64, 76_561_198_000_000_102);
        assert_eq!(game.payload, payload);
        assert!(
            timeout(Duration::from_millis(150), recv_game(&peer_c))
                .await
                .is_err()
        );

        server.abort();
    }

    async fn join_peer(
        socket: &UdpSocket,
        relay_address: SocketAddr,
        room: &str,
        steam_id64: &str,
    ) {
        send_join(socket, relay_address, room, steam_id64, None).await;
        let challenge = match recv_control(socket).await {
            ControlMessage::Challenge { token } => token,
            other => panic!("expected challenge, got {other:?}"),
        };
        send_join(socket, relay_address, room, steam_id64, Some(challenge)).await;
        assert!(matches!(
            recv_control(socket).await,
            ControlMessage::Ready { .. }
        ));
    }

    async fn send_join(
        socket: &UdpSocket,
        relay_address: SocketAddr,
        room: &str,
        steam_id64: &str,
        challenge: Option<String>,
    ) {
        let message = ControlMessage::Join {
            room: room.to_owned(),
            steam_id64: steam_id64.to_owned(),
            display_name: None,
            challenge,
        };
        let payload = message.encode().unwrap();
        let bytes = Envelope::new(MessageType::Join, payload).encode().unwrap();
        socket.send_to(&bytes, relay_address).await.unwrap();
    }

    async fn recv_control(socket: &UdpSocket) -> ControlMessage {
        let mut buffer = [0_u8; 4096];
        let (size, _) = timeout(Duration::from_secs(1), socket.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        let envelope = Envelope::decode(Bytes::copy_from_slice(&buffer[..size])).unwrap();
        ControlMessage::decode(&envelope.payload).unwrap()
    }

    async fn send_game(
        socket: &UdpSocket,
        relay_address: SocketAddr,
        from_steam_id64: &str,
        to_steam_id64: u64,
        payload: Bytes,
    ) {
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
        socket.send_to(&bytes, relay_address).await.unwrap();
    }

    async fn recv_game(socket: &UdpSocket) -> GamePacket {
        let mut buffer = [0_u8; 4096];
        let (size, _) = socket.recv_from(&mut buffer).await.unwrap();
        let envelope = Envelope::decode(Bytes::copy_from_slice(&buffer[..size])).unwrap();
        assert_eq!(envelope.message_type, MessageType::Data);
        GamePacket::decode(&envelope.payload).unwrap()
    }
}
