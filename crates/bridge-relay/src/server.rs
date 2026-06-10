use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use basement_bridge_core::protocol::{ControlMessage, Envelope, GamePacket, MessageType};
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
    state::{PeerId, RelayState, error_message},
};

type SharedState = Arc<Mutex<RelayState>>;
type SharedEgress = Arc<Mutex<EgressTable>>;
type SharedMetrics = Arc<Mutex<RelayMetrics>>;
type SharedPeerRegistry = Arc<Mutex<PeerRegistry>>;

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
        egress
            .lock()
            .await
            .insert(peer_id, PeerEgress::Udp(address));
        handle_datagram(
            Arc::clone(&socket),
            Arc::clone(&state),
            Arc::clone(&egress),
            Arc::clone(&metrics),
            DatagramSource { peer_id, address },
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
        egress.lock().await.insert(peer_id, PeerEgress::Tcp(tx));
        let runtime = TcpConnectionRuntime {
            udp_socket: Arc::clone(&udp_socket),
            state: Arc::clone(&state),
            egress: Arc::clone(&egress),
            metrics: Arc::clone(&metrics),
            max_packet_size: config.max_packet_size,
        };
        tokio::spawn(tcp_connection_task(
            stream,
            DatagramSource { peer_id, address },
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
                            warn!(peer_id = %source.peer_id, address = %source.address, %error, "TCP frame handling failed");
                            break;
                        }
                    }
                    Err(error) => {
                        warn!(peer_id = %source.peer_id, address = %source.address, %error, "TCP frame rejected");
                        break;
                    }
                }
            }
            Some(raw) = outbound_rx.recv() => {
                if let Err(error) = sink.send(raw).await {
                    warn!(peer_id = %source.peer_id, address = %source.address, %error, "TCP frame send failed");
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

#[derive(Clone, Copy, Debug)]
struct DatagramSource {
    peer_id: PeerId,
    address: SocketAddr,
}

#[derive(Clone)]
struct TcpConnectionRuntime {
    udp_socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedEgress,
    metrics: SharedMetrics,
    max_packet_size: usize,
}

#[derive(Clone, Debug)]
enum PeerEgress {
    Udp(SocketAddr),
    Tcp(mpsc::Sender<Bytes>),
}

#[derive(Debug, Default)]
struct EgressTable {
    peers: HashMap<PeerId, PeerEgress>,
}

impl EgressTable {
    fn insert(&mut self, peer_id: PeerId, egress: PeerEgress) {
        self.peers.insert(peer_id, egress);
    }

    fn get(&self, peer_id: PeerId) -> Option<PeerEgress> {
        self.peers.get(&peer_id).cloned()
    }

    fn remove(&mut self, peer_id: PeerId) {
        self.peers.remove(&peer_id);
    }
}

#[derive(Debug)]
struct PeerRegistry {
    next_id: u64,
    udp_peers: HashMap<SocketAddr, PeerId>,
}

impl Default for PeerRegistry {
    fn default() -> Self {
        Self {
            next_id: 1,
            udp_peers: HashMap::new(),
        }
    }
}

impl PeerRegistry {
    fn allocate(&mut self) -> PeerId {
        let peer_id = PeerId::new(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        peer_id
    }

    fn udp_peer(&mut self, address: SocketAddr) -> PeerId {
        if let Some(peer_id) = self.udp_peers.get(&address) {
            return *peer_id;
        }
        let peer_id = self.allocate();
        self.udp_peers.insert(address, peer_id);
        peer_id
    }
}

#[derive(Debug, Default)]
struct RelayMetrics {
    tcp_egress_queue_capacity: usize,
    packets_in: u64,
    data_in: u64,
    forwarded_packets: u64,
    forwarded_bytes: u64,
    tcp_egress_queue_full: u64,
    tcp_egress_dropped_packets: u64,
    decode_errors: u64,
    unjoined_data: u64,
    missing_target: u64,
    blocked: u64,
    rate_limited: u64,
    packet_handling_errors: u64,
}

impl RelayMetrics {
    fn new(tcp_egress_queue_capacity: usize) -> Self {
        Self {
            tcp_egress_queue_capacity,
            ..Self::default()
        }
    }

    fn add(&mut self, outcome: PacketOutcome) {
        self.data_in = self.data_in.saturating_add(outcome.data_in);
        self.forwarded_packets = self
            .forwarded_packets
            .saturating_add(outcome.forwarded_packets);
        self.forwarded_bytes = self.forwarded_bytes.saturating_add(outcome.forwarded_bytes);
        self.tcp_egress_queue_full = self
            .tcp_egress_queue_full
            .saturating_add(outcome.tcp_egress_queue_full);
        self.tcp_egress_dropped_packets = self
            .tcp_egress_dropped_packets
            .saturating_add(outcome.tcp_egress_dropped_packets);
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
            tcp_egress_queue_capacity = self.tcp_egress_queue_capacity,
            tcp_egress_queue_full = self.tcp_egress_queue_full,
            tcp_egress_dropped_packets = self.tcp_egress_dropped_packets,
            decode_errors = self.decode_errors,
            unjoined_data = self.unjoined_data,
            missing_target = self.missing_target,
            blocked = self.blocked,
            rate_limited = self.rate_limited,
            packet_handling_errors = self.packet_handling_errors,
            "relay stats"
        );
        *self = Self::new(self.tcp_egress_queue_capacity);
    }
}

#[derive(Debug, Default)]
struct PacketOutcome {
    data_in: u64,
    forwarded_packets: u64,
    forwarded_bytes: u64,
    tcp_egress_queue_full: u64,
    tcp_egress_dropped_packets: u64,
    decode_errors: u64,
    unjoined_data: u64,
    missing_target: u64,
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
        metrics.packets_in = metrics.packets_in.saturating_add(1);
    }
    {
        let mut state = state.lock().await;
        if state.is_blocked(source.address) {
            let mut metrics = metrics.lock().await;
            metrics.blocked = metrics.blocked.saturating_add(1);
            debug!(address = %source.address, "packet rejected by blocklist");
            return Ok(());
        }
        if !state.allow_packet(source.peer_id, now) {
            let mut metrics = metrics.lock().await;
            metrics.rate_limited = metrics.rate_limited.saturating_add(1);
            debug!(peer_id = %source.peer_id, address = %source.address, "rate limit exceeded");
            return Ok(());
        }
    }

    match handle_packet(
        Arc::clone(&udp_socket),
        Arc::clone(&state),
        Arc::clone(&egress),
        source,
        raw,
        now,
    )
    .await
    {
        Ok(outcome) => metrics.lock().await.add(outcome),
        Err(error) => {
            let mut metrics = metrics.lock().await;
            metrics.packet_handling_errors = metrics.packet_handling_errors.saturating_add(1);
            warn!(peer_id = %source.peer_id, address = %source.address, %error, "packet handling failed");
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
            state.lock().await.touch_peer(source.peer_id, now);
            Ok(PacketOutcome::default())
        }
        _ => Ok(PacketOutcome::default()),
    }
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
            } => state.complete_join(source.peer_id, room, steam_id64, challenge, now),
            ControlMessage::Join {
                room,
                steam_id64,
                display_name,
                challenge: None,
            } => state.challenge_join(source.peer_id, room, steam_id64, display_name, now),
            _ => error_message("bad_join", "expected join message"),
        }
    };
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
    let mut outcome = PacketOutcome {
        data_in: 1,
        ..PacketOutcome::default()
    };

    let data_envelope = match Envelope::decode(raw.clone()) {
        Ok(envelope) => envelope,
        Err(error) => {
            warn!(peer_id = %source.peer_id, address = %source.address, %error, "bad data envelope");
            outcome.decode_errors = outcome.decode_errors.saturating_add(1);
            return Ok(outcome);
        }
    };
    let game = match GamePacket::decode(&data_envelope.payload) {
        Ok(packet) => packet,
        Err(error) => {
            warn!(peer_id = %source.peer_id, address = %source.address, %error, "bad data packet");
            outcome.decode_errors = outcome.decode_errors.saturating_add(1);
            return Ok(outcome);
        }
    };

    let Some(target_peer) = state
        .lock()
        .await
        .target_peer(&room_name, game.to_steam_id64)
    else {
        outcome.missing_target = outcome.missing_target.saturating_add(1);
        debug!(
            peer_id = %source.peer_id,
            address = %source.address,
            room = %room_name,
            to_steam_id64 = game.to_steam_id64,
            "data target is not joined"
        );
        return Ok(outcome);
    };

    if target_peer != source.peer_id {
        match send_to_peer(udp_socket, egress, target_peer, raw.clone()).await {
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
                debug!(peer_id = %target_peer, "TCP egress queue is full; dropping relay datagram");
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
    send_to_peer(udp_socket, egress, peer_id, raw).await
}

async fn send_to_peer(
    udp_socket: Arc<UdpSocket>,
    egress: SharedEgress,
    peer_id: PeerId,
    raw: Bytes,
) -> io::Result<()> {
    let Some(target) = egress.lock().await.get(peer_id) else {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            format!("missing egress for {peer_id}"),
        ));
    };
    match target {
        PeerEgress::Udp(address) => {
            udp_socket.send_to(&raw, address).await?;
            Ok(())
        }
        PeerEgress::Tcp(sender) => sender.try_send(raw).map_err(|_| {
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
