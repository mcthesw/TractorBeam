use std::{
    io,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use basement_bridge_core::{
    protocol::{ControlMessage, Envelope, GamePacket, MessageType},
    udp_fec::UdpFecProfile,
};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Mutex, mpsc},
    task::JoinSet,
    time::{self, MissedTickBehavior},
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, info, warn};

use crate::{
    config::RelayConfig,
    egress::{EgressTable, PeerOutput},
    metrics::{PacketOutcome, RelayMetrics},
    peer_registry::PeerRegistry,
    state::{JoinCompletion, PeerId, PeerTransport, RelayState, error_message},
};

type SharedState = Arc<Mutex<RelayState>>;
type SharedEgress = Arc<Mutex<EgressTable>>;
type SharedMetrics = Arc<Mutex<RelayMetrics>>;
type SharedPeerRegistry = Arc<Mutex<PeerRegistry>>;

const UDP_FEC_FLUSH_INTERVAL: Duration = Duration::from_millis(1);

pub(crate) async fn run(config: RelayConfig) -> io::Result<()> {
    let udp_socket = UdpSocket::bind(&config.bind).await?;
    let tcp_listener = if config.tcp_enabled {
        Some(TcpListener::bind(&config.tcp_bind).await?)
    } else {
        None
    };
    info!(
        udp_bind = %config.bind,
        tcp_bind = if config.tcp_enabled { config.tcp_bind.as_str() } else { "disabled" },
        tcp_egress_queue_capacity = config.tcp_egress_queue_capacity,
        max_packet_size = config.max_packet_size,
        rate_limit_per_second = config.rate_limit_per_second,
        max_rooms = config.max_rooms,
        max_peers_per_room = config.max_peers_per_room,
        blocked_cidrs = config.blocked_cidrs.len(),
        "relay listening"
    );

    run_with_listeners(udp_socket, tcp_listener, config).await
}

async fn run_with_listeners(
    udp_socket: UdpSocket,
    tcp_listener: Option<TcpListener>,
    config: RelayConfig,
) -> io::Result<()> {
    let udp_socket = Arc::new(udp_socket);
    let state = Arc::new(Mutex::new(RelayState::new(config.clone())));
    let egress = Arc::new(Mutex::new(EgressTable::default()));
    let metrics = Arc::new(Mutex::new(RelayMetrics::new(
        config.tcp_egress_queue_capacity,
    )));
    let peer_registry = Arc::new(Mutex::new(PeerRegistry::default()));
    let mut tasks = JoinSet::new();

    tasks.spawn(run_udp_listener(
        Arc::clone(&udp_socket),
        Arc::clone(&state),
        Arc::clone(&egress),
        Arc::clone(&metrics),
        Arc::clone(&peer_registry),
        config.clone(),
    ));

    if let Some(listener) = tcp_listener {
        tasks.spawn(run_tcp_listener(
            listener,
            Arc::clone(&udp_socket),
            Arc::clone(&state),
            Arc::clone(&egress),
            Arc::clone(&metrics),
            Arc::clone(&peer_registry),
            config.clone(),
        ));
    }

    tasks.spawn(run_stats_loop(Arc::clone(&state), Arc::clone(&metrics)));
    tasks.spawn(run_udp_fec_flush_loop(
        Arc::clone(&udp_socket),
        Arc::clone(&egress),
    ));

    match tasks.join_next().await {
        Some(Ok(result)) => result,
        Some(Err(error)) => Err(io::Error::other(error)),
        None => Ok(()),
    }
}

async fn run_udp_listener(
    socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedEgress,
    metrics: SharedMetrics,
    peer_registry: SharedPeerRegistry,
    config: RelayConfig,
) -> io::Result<()> {
    let mut buffer = vec![0_u8; config.max_packet_size];
    loop {
        let (size, address) = socket.recv_from(&mut buffer).await?;
        let peer_id = peer_registry.lock().await.udp_peer(address);
        egress.lock().await.upsert_udp(peer_id, address);
        handle_datagram(
            Arc::clone(&socket),
            Arc::clone(&state),
            Arc::clone(&egress),
            Arc::clone(&metrics),
            DatagramSource {
                peer_id,
                address,
                transport: PeerTransport::Udp,
            },
            Bytes::copy_from_slice(&buffer[..size]),
        )
        .await?;
    }
}

async fn run_tcp_listener(
    listener: TcpListener,
    udp_socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedEgress,
    metrics: SharedMetrics,
    peer_registry: SharedPeerRegistry,
    config: RelayConfig,
) -> io::Result<()> {
    loop {
        let (stream, address) = listener.accept().await?;
        stream.set_nodelay(true)?;
        let peer_id = peer_registry.lock().await.allocate();
        let (tx, rx) = tcp_egress_channel(config.tcp_egress_queue_capacity);
        egress.lock().await.insert_tcp(peer_id, tx);
        let runtime = TcpConnectionRuntime {
            udp_socket: Arc::clone(&udp_socket),
            state: Arc::clone(&state),
            egress: Arc::clone(&egress),
            metrics: Arc::clone(&metrics),
            max_packet_size: config.max_packet_size,
        };
        tokio::spawn(tcp_connection_task(
            stream,
            DatagramSource {
                peer_id,
                address,
                transport: PeerTransport::Tcp,
            },
            rx,
            runtime,
        ));
    }
}

async fn tcp_connection_task(
    stream: TcpStream,
    source: DatagramSource,
    mut outbound_rx: mpsc::Receiver<Bytes>,
    runtime: TcpConnectionRuntime,
) {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(runtime.max_packet_size)
        .new_codec();
    let framed = Framed::new(stream, codec);
    let (mut sink, mut stream) = framed.split();

    loop {
        tokio::select! {
            frame = stream.next() => {
                let Some(frame) = frame else {
                    break;
                };
                match frame {
                    Ok(bytes) => {
                        if let Err(error) = handle_datagram(
                            Arc::clone(&runtime.udp_socket),
                            Arc::clone(&runtime.state),
                            Arc::clone(&runtime.egress),
                            Arc::clone(&runtime.metrics),
                            source,
                            bytes.freeze(),
                        ).await {
                            warn!(
                                peer_id = %source.peer_id,
                                address = %source.address,
                                transport = %source.transport,
                                %error,
                                "TCP frame handling failed"
                            );
                            break;
                        }
                    }
                    Err(error) => {
                        warn!(
                            peer_id = %source.peer_id,
                            address = %source.address,
                            transport = %source.transport,
                            %error,
                            "TCP frame rejected"
                        );
                        break;
                    }
                }
            }
            Some(raw) = outbound_rx.recv() => {
                if let Err(error) = sink.send(raw).await {
                    warn!(
                        peer_id = %source.peer_id,
                        address = %source.address,
                        transport = %source.transport,
                        %error,
                        "TCP frame send failed"
                    );
                    break;
                }
            }
        }
    }

    runtime.state.lock().await.remove_peer(source.peer_id);
    runtime.egress.lock().await.remove(source.peer_id);
}

async fn run_stats_loop(state: SharedState, metrics: SharedMetrics) -> io::Result<()> {
    let mut stats_interval = time::interval(Duration::from_secs(5));
    stats_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        stats_interval.tick().await;
        let now = Instant::now();
        let mut state = state.lock().await;
        state.cleanup(now);
        metrics.lock().await.log_and_reset(&state);
    }
}

async fn run_udp_fec_flush_loop(
    udp_socket: Arc<UdpSocket>,
    egress: SharedEgress,
) -> io::Result<()> {
    let mut flush_interval = time::interval(UDP_FEC_FLUSH_INTERVAL);
    flush_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        flush_interval.tick().await;
        let frames = egress.lock().await.flush_udp_fec(Instant::now())?;
        for (address, frame) in frames {
            udp_socket.send_to(&frame, address).await?;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct DatagramSource {
    peer_id: PeerId,
    address: SocketAddr,
    transport: PeerTransport,
}

#[derive(Clone)]
struct TcpConnectionRuntime {
    udp_socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedEgress,
    metrics: SharedMetrics,
    max_packet_size: usize,
}

async fn handle_datagram(
    udp_socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedEgress,
    metrics: SharedMetrics,
    source: DatagramSource,
    raw: Bytes,
) -> io::Result<()> {
    let now = Instant::now();
    {
        let mut metrics = metrics.lock().await;
        metrics.record_packet_in();
    }
    {
        let mut state = state.lock().await;
        if state.is_blocked(source.address) {
            let room = state.peer_room(source.peer_id);
            let mut metrics = metrics.lock().await;
            metrics.record_blocked(room.as_deref());
            debug!(
                peer_id = %source.peer_id,
                address = %source.address,
                transport = %source.transport,
                room = room.as_deref().unwrap_or(""),
                "packet rejected by blocklist"
            );
            return Ok(());
        }
        if !state.allow_packet(source.peer_id, now) {
            let room = state.peer_room(source.peer_id);
            let mut metrics = metrics.lock().await;
            metrics.record_rate_limited(room.as_deref());
            debug!(
                peer_id = %source.peer_id,
                address = %source.address,
                transport = %source.transport,
                room = room.as_deref().unwrap_or(""),
                "rate limit exceeded"
            );
            return Ok(());
        }
    }

    let datagrams = if source.transport == PeerTransport::Udp {
        match egress.lock().await.decode_udp(source.peer_id, raw, now) {
            Ok(datagrams) => datagrams,
            Err(error) => {
                let room = state.lock().await.peer_room(source.peer_id);
                metrics.lock().await.add(PacketOutcome {
                    decode_errors: 1,
                    ..room.as_ref().map_or_else(PacketOutcome::default, |room| {
                        PacketOutcome::for_room(room.clone())
                    })
                });
                warn!(
                    peer_id = %source.peer_id,
                    address = %source.address,
                    transport = %source.transport,
                    room = room.as_deref().unwrap_or(""),
                    %error,
                    "UDP FEC frame decode failed"
                );
                return Ok(());
            }
        }
    } else {
        vec![raw]
    };

    for datagram in datagrams {
        match handle_packet(
            Arc::clone(&udp_socket),
            Arc::clone(&state),
            Arc::clone(&egress),
            source,
            datagram,
            now,
        )
        .await
        {
            Ok(outcome) => metrics.lock().await.add(outcome),
            Err(error) => {
                let room = state.lock().await.peer_room(source.peer_id);
                let mut metrics = metrics.lock().await;
                metrics.record_packet_handling_error(room.as_deref());
                warn!(
                    peer_id = %source.peer_id,
                    address = %source.address,
                    transport = %source.transport,
                    room = room.as_deref().unwrap_or(""),
                    %error,
                    "packet handling failed"
                );
            }
        }
    }
    state.lock().await.cleanup(now);
    Ok(())
}

async fn handle_packet(
    udp_socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedEgress,
    source: DatagramSource,
    raw: Bytes,
    now: Instant,
) -> io::Result<PacketOutcome> {
    let envelope = match Envelope::decode(raw.clone()) {
        Ok(envelope) => envelope,
        Err(error) => {
            send_control(
                udp_socket,
                egress,
                source.peer_id,
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
        MessageType::Join => handle_join(udp_socket, state, egress, source, &envelope, now).await,
        MessageType::Data => forward_data(udp_socket, state, egress, source, &raw, now).await,
        MessageType::Heartbeat => {
            handle_heartbeat(udp_socket, state, egress, source, &envelope, now).await
        }
        _ => Ok(PacketOutcome::default()),
    }
}

async fn handle_heartbeat(
    udp_socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedEgress,
    source: DatagramSource,
    envelope: &Envelope,
    now: Instant,
) -> io::Result<PacketOutcome> {
    let room = state.lock().await.touch_peer(source.peer_id, now);
    if let Ok(ControlMessage::HealthPing { id }) = ControlMessage::decode(&envelope.payload) {
        send_control(
            udp_socket,
            egress,
            source.peer_id,
            MessageType::Heartbeat,
            &ControlMessage::HealthPong { id },
        )
        .await?;
    }
    Ok(room.map_or_else(PacketOutcome::default, PacketOutcome::for_room))
}

async fn handle_join(
    udp_socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedEgress,
    source: DatagramSource,
    envelope: &Envelope,
    now: Instant,
) -> io::Result<PacketOutcome> {
    let message = ControlMessage::decode(&envelope.payload)
        .unwrap_or_else(|error| error_message("bad_join", error.to_string()));
    let response = {
        let mut state = state.lock().await;
        match message {
            ControlMessage::Join {
                room,
                steam_id64,
                display_name: _,
                challenge: Some(challenge),
                udp_fec,
            } => state.complete_join(JoinCompletion {
                peer_id: source.peer_id,
                room,
                steam_id64,
                challenge,
                transport: source.transport,
                udp_fec,
                now,
            }),
            ControlMessage::Join {
                room,
                steam_id64,
                display_name,
                challenge: None,
                udp_fec,
            } => state.challenge_join(source.peer_id, room, steam_id64, display_name, udp_fec, now),
            _ => error_message("bad_join", "expected join message"),
        }
    };
    if let ControlMessage::Ready {
        udp_fec: Some(udp_fec),
        ..
    } = &response
        && source.transport == PeerTransport::Udp
    {
        egress
            .lock()
            .await
            .enable_udp_fec(source.peer_id, UdpFecProfile::for_name(udp_fec.profile));
    }
    let response_type = match response {
        ControlMessage::Challenge { .. } => MessageType::JoinChallenge,
        ControlMessage::Ready { .. } => MessageType::JoinReady,
        ControlMessage::Error { .. } => MessageType::Error,
        _ => MessageType::Error,
    };
    send_control(udp_socket, egress, source.peer_id, response_type, &response).await?;
    Ok(PacketOutcome::default())
}

async fn forward_data(
    udp_socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedEgress,
    source: DatagramSource,
    raw: &Bytes,
    now: Instant,
) -> io::Result<PacketOutcome> {
    let Some(room_name) = state.lock().await.touch_peer(source.peer_id, now) else {
        debug!(
            peer_id = %source.peer_id,
            address = %source.address,
            transport = %source.transport,
            "data rejected before join"
        );
        send_control(
            udp_socket,
            egress,
            source.peer_id,
            MessageType::Error,
            &error_message("not_joined", "join a room before sending data"),
        )
        .await?;
        return Ok(PacketOutcome {
            unjoined_data: 1,
            ..PacketOutcome::default()
        });
    };
    let mut outcome = PacketOutcome::for_room(room_name.clone());
    outcome.data_in = 1;

    let data_envelope = match Envelope::decode(raw.clone()) {
        Ok(envelope) => envelope,
        Err(error) => {
            warn!(
                peer_id = %source.peer_id,
                address = %source.address,
                transport = %source.transport,
                room = %room_name,
                %error,
                "bad data envelope"
            );
            outcome.decode_errors = outcome.decode_errors.saturating_add(1);
            return Ok(outcome);
        }
    };
    let game = match GamePacket::decode(&data_envelope.payload) {
        Ok(packet) => packet,
        Err(error) => {
            warn!(
                peer_id = %source.peer_id,
                address = %source.address,
                transport = %source.transport,
                room = %room_name,
                %error,
                "bad data packet"
            );
            outcome.decode_errors = outcome.decode_errors.saturating_add(1);
            return Ok(outcome);
        }
    };

    let (target_peer, missing_target_incident) = {
        let mut state = state.lock().await;
        match state.target_peer(&room_name, game.to_steam_id64) {
            Some(target_peer) => (Some(target_peer), None),
            None => (None, state.record_missing_target_incident(&room_name, now)),
        }
    };

    let Some(target_peer) = target_peer else {
        outcome.missing_target = outcome.missing_target.saturating_add(1);
        if let Some(incident) = missing_target_incident {
            warn!(
                peer_id = %source.peer_id,
                address = %source.address,
                transport = %source.transport,
                room = %room_name,
                from_steam_id64 = game.from_steam_id64,
                to_steam_id64 = game.to_steam_id64,
                room_peer_count = incident.peer_count(),
                room_tcp_peers = incident.tcp_peer_count(),
                room_udp_peers = incident.udp_peer_count(),
                room_peers = %incident.peer_summary(),
                "data target is not joined"
            );
        } else {
            debug!(
                peer_id = %source.peer_id,
                address = %source.address,
                transport = %source.transport,
                room = %room_name,
                from_steam_id64 = game.from_steam_id64,
                to_steam_id64 = game.to_steam_id64,
                "data target is not joined"
            );
        }
        return Ok(outcome);
    };

    if target_peer != source.peer_id {
        match send_data_to_peer(udp_socket, egress, target_peer, raw.clone(), now).await {
            Ok(()) => {
                outcome.forwarded_packets = outcome.forwarded_packets.saturating_add(1);
                outcome.forwarded_bytes = outcome
                    .forwarded_bytes
                    .saturating_add(u64::try_from(raw.len()).unwrap_or(u64::MAX));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                outcome.tcp_egress_queue_full = outcome.tcp_egress_queue_full.saturating_add(1);
                outcome.tcp_egress_dropped_packets =
                    outcome.tcp_egress_dropped_packets.saturating_add(1);
                debug!(
                    source_peer_id = %source.peer_id,
                    target_peer_id = %target_peer,
                    source_transport = %source.transport,
                    room = %room_name,
                    "TCP egress queue is full; dropping relay datagram"
                );
            }
            Err(error) => return Err(error),
        }
    }

    Ok(outcome)
}

fn tcp_egress_channel(capacity: usize) -> (mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
    mpsc::channel(capacity)
}

async fn send_control(
    udp_socket: Arc<UdpSocket>,
    egress: SharedEgress,
    peer_id: PeerId,
    message_type: MessageType,
    message: &ControlMessage,
) -> io::Result<()> {
    let payload = message.encode().map_err(io::Error::other)?;
    let raw = Envelope::new(message_type, payload)
        .encode()
        .map_err(io::Error::other)?;
    send_control_to_peer(udp_socket, egress, peer_id, raw).await
}

async fn send_control_to_peer(
    udp_socket: Arc<UdpSocket>,
    egress: SharedEgress,
    peer_id: PeerId,
    raw: Bytes,
) -> io::Result<()> {
    let output = egress.lock().await.send_control(peer_id, raw)?;
    send_peer_output(udp_socket, peer_id, output).await
}

async fn send_data_to_peer(
    udp_socket: Arc<UdpSocket>,
    egress: SharedEgress,
    peer_id: PeerId,
    raw: Bytes,
    now: Instant,
) -> io::Result<()> {
    let output = egress.lock().await.send_data(peer_id, raw, now)?;
    send_peer_output(udp_socket, peer_id, output).await
}

async fn send_peer_output(
    udp_socket: Arc<UdpSocket>,
    peer_id: PeerId,
    output: PeerOutput,
) -> io::Result<()> {
    match output {
        PeerOutput::Udp { address, frames } => {
            for frame in frames {
                udp_socket.send_to(&frame, address).await?;
            }
            Ok(())
        }
        PeerOutput::Tcp { sender, frame } => sender.try_send(frame).map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("TCP egress queue is full for {peer_id}"),
            )
        }),
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
