use std::{
    collections::HashMap,
    io,
    net::UdpSocket,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use bytes::Bytes;

use crate::protocol::{
    ControlMessage, Envelope, GamePacket, LocalPacket, LocalPacketType, MessageType,
};

use super::{
    Counters, LogLevel, SessionConfig,
    hook_config::{HOOK_IN, HOOK_OUT},
    state::{RuntimeEvent, error_counter, send_event},
};

const SOCKET_DRAIN_LIMIT: usize = 64;

#[derive(Debug)]
pub(super) struct SessionHandle {
    stop: Arc<AtomicBool>,
    pub(super) events: Receiver<RuntimeEvent>,
    event_tx: Sender<RuntimeEvent>,
    workers: Vec<JoinHandle<()>>,
}

pub(super) fn spawn_bridge_worker(config: SessionConfig) -> SessionHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let (event_tx, event_rx) = mpsc::channel();
    let worker_stop = Arc::clone(&stop);
    let worker_events = event_tx.clone();
    let worker = thread::spawn(move || {
        if let Err(error) = bridge_loop(&config, &worker_stop, &worker_events) {
            send_event(
                &worker_events,
                RuntimeEvent::Log(LogLevel::Error, format!("Bridge worker stopped: {error}")),
            );
            send_event(&worker_events, RuntimeEvent::CounterDelta(error_counter()));
        }
        send_event(&worker_events, RuntimeEvent::Stopped);
    });
    SessionHandle {
        stop,
        events: event_rx,
        event_tx,
        workers: vec![worker],
    }
}

impl SessionHandle {
    pub(super) fn spawn_injector_worker(&mut self) {
        let stop = Arc::clone(&self.stop);
        let events = self.event_tx.clone();
        self.workers.push(thread::spawn(move || {
            inject_when_ready(&stop, &events);
        }));
    }

    pub(super) fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for worker in self.workers {
            let _ = worker.join();
        }
    }
}

fn bridge_loop(
    config: &SessionConfig,
    stop: &AtomicBool,
    event_tx: &Sender<RuntimeEvent>,
) -> io::Result<()> {
    let hook_socket = UdpSocket::bind(HOOK_IN)?;
    let relay_socket = UdpSocket::bind("0.0.0.0:0")?;
    relay_socket.connect(config.relay.to_string())?;
    relay_socket.set_read_timeout(Some(Duration::from_millis(20)))?;
    complete_relay_join(&relay_socket, config, event_tx)?;
    hook_socket.set_nonblocking(true)?;
    relay_socket.set_nonblocking(true)?;
    send_event(
        event_tx,
        RuntimeEvent::Log(LogLevel::Info, "Local bridge is running".to_owned()),
    );
    send_event(
        event_tx,
        RuntimeEvent::Log(
            LogLevel::Debug,
            format!(
                "Bridge sockets ready: hook_in={HOOK_IN} hook_out={HOOK_OUT} relay={} drain_limit={SOCKET_DRAIN_LIMIT}",
                config.relay,
            ),
        ),
    );

    let hook_out = HOOK_OUT;
    let mut hook_buffer = [0_u8; 65_535];
    let mut relay_buffer = [0_u8; 65_535];
    let mut local_sequence = 1_u32;
    let mut hook_packets = 0_u64;
    let mut relay_packets = 0_u64;
    let mut last_hook_packet_at = None;
    let mut last_relay_packet_at = None;
    let mut last_remote_sequences = HashMap::new();
    let mut last_heartbeat = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut had_activity = false;
        if last_heartbeat.elapsed() >= Duration::from_secs(1) {
            send_control(
                &relay_socket,
                MessageType::Heartbeat,
                &ControlMessage::Heartbeat,
            )?;
            last_heartbeat = Instant::now();
            had_activity = true;
        }

        for _ in 0..SOCKET_DRAIN_LIMIT {
            match hook_socket.recv_from(&mut hook_buffer) {
                Ok((size, _)) => {
                    had_activity = true;
                    observe_packet_gap(event_tx, "Hook -> Relay", &mut last_hook_packet_at);
                    match forward_hook_packet(
                        &relay_socket,
                        &config.steam_id64,
                        Bytes::copy_from_slice(&hook_buffer[..size]),
                        event_tx,
                    ) {
                        Ok(summary) => {
                            hook_packets = hook_packets.saturating_add(1);
                            if should_sample_packet(hook_packets) {
                                send_event(
                                    event_tx,
                                    RuntimeEvent::Log(
                                        LogLevel::Debug,
                                        format!(
                                            "Hook -> Relay packet #{hook_packets}: to={} sequence={} channel={} send_type={} payload_bytes={} wire_bytes={}",
                                            summary.peer,
                                            summary.sequence,
                                            summary.channel,
                                            summary.send_type,
                                            summary.payload_bytes,
                                            summary.wire_bytes
                                        ),
                                    ),
                                );
                            }
                        }
                        Err(error) => {
                            send_event(
                                event_tx,
                                RuntimeEvent::Log(
                                    LogLevel::Warn,
                                    format!("Bad hook packet: {error}"),
                                ),
                            );
                            send_event(event_tx, RuntimeEvent::CounterDelta(error_counter()));
                        }
                    }
                }
                Err(error) if would_wait(&error) => break,
                Err(error) => return Err(error),
            }
        }

        for _ in 0..SOCKET_DRAIN_LIMIT {
            match relay_socket.recv(&mut relay_buffer) {
                Ok(size) => {
                    had_activity = true;
                    observe_packet_gap(event_tx, "Relay -> Hook", &mut last_relay_packet_at);
                    match forward_relay_packet(
                        &hook_socket,
                        hook_out,
                        Bytes::copy_from_slice(&relay_buffer[..size]),
                        &mut local_sequence,
                        event_tx,
                    ) {
                        Ok(Some(summary)) => {
                            observe_source_sequence(event_tx, &mut last_remote_sequences, &summary);
                            relay_packets = relay_packets.saturating_add(1);
                            if should_sample_packet(relay_packets) {
                                send_event(
                                    event_tx,
                                    RuntimeEvent::Log(
                                        LogLevel::Debug,
                                        format!(
                                            "Relay -> Hook packet #{relay_packets}: from={} source_sequence={} local_sequence={} channel={} send_type={} payload_bytes={} local_bytes={}",
                                            summary.peer,
                                            summary.source_sequence,
                                            summary.sequence,
                                            summary.channel,
                                            summary.send_type,
                                            summary.payload_bytes,
                                            summary.wire_bytes
                                        ),
                                    ),
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            send_event(
                                event_tx,
                                RuntimeEvent::Log(
                                    LogLevel::Warn,
                                    format!("Bad relay packet: {error}"),
                                ),
                            );
                            send_event(event_tx, RuntimeEvent::CounterDelta(error_counter()));
                        }
                    }
                }
                Err(error) if would_wait(&error) => break,
                Err(error) => return Err(error),
            }
        }

        if !had_activity {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PacketSummary {
    peer: u64,
    sequence: u32,
    source_sequence: u32,
    channel: i32,
    send_type: i32,
    payload_bytes: usize,
    wire_bytes: usize,
}

fn inject_when_ready(stop: &AtomicBool, event_tx: &Sender<RuntimeEvent>) {
    send_event(
        event_tx,
        RuntimeEvent::Log(LogLevel::Info, "Waiting for Isaac process".to_owned()),
    );

    while !stop.load(Ordering::Relaxed) {
        if let Some(process) = basement_isaac_injector::find_isaac_process() {
            let result = basement_isaac_injector::resolve_native_hook_paths()
                .and_then(|paths| basement_isaac_injector::run_injector(&paths, process.pid));
            match result {
                Ok(()) => send_event(
                    event_tx,
                    RuntimeEvent::Log(
                        LogLevel::Info,
                        format!(
                            "Native Hook injected into {} ({})",
                            process.name, process.pid
                        ),
                    ),
                ),
                Err(error) => {
                    send_event(
                        event_tx,
                        RuntimeEvent::Log(
                            LogLevel::Error,
                            format!("Native Hook injection failed: {error}"),
                        ),
                    );
                    send_event(event_tx, RuntimeEvent::CounterDelta(error_counter()));
                }
            }
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn complete_relay_join(
    socket: &UdpSocket,
    config: &SessionConfig,
    event_tx: &Sender<RuntimeEvent>,
) -> io::Result<()> {
    send_join(socket, config, None)?;
    let mut buffer = [0_u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match socket.recv(&mut buffer) {
            Ok(size) => {
                let envelope = Envelope::decode(Bytes::copy_from_slice(&buffer[..size]))
                    .map_err(io::Error::other)?;
                let control =
                    ControlMessage::decode(&envelope.payload).map_err(io::Error::other)?;
                match control {
                    ControlMessage::Challenge { token } => send_join(socket, config, Some(token))?,
                    ControlMessage::Ready { peer_count } => {
                        send_event(
                            event_tx,
                            RuntimeEvent::Log(
                                LogLevel::Info,
                                format!("Joined relay room with {peer_count} peer(s)"),
                            ),
                        );
                        return Ok(());
                    }
                    ControlMessage::Error { code, message } => {
                        send_event(
                            event_tx,
                            RuntimeEvent::Log(
                                LogLevel::Warn,
                                format!("Relay join rejected: {code}: {message}"),
                            ),
                        );
                        return Err(io::Error::other(format!("{code}: {message}")));
                    }
                    _ => {}
                }
            }
            Err(error) if would_wait(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "relay join timed out",
    ))
}

fn send_join(
    socket: &UdpSocket,
    config: &SessionConfig,
    challenge: Option<String>,
) -> io::Result<()> {
    let message = ControlMessage::Join {
        room: config.room.clone(),
        steam_id64: config.steam_id64.clone(),
        display_name: Some(config.display_name.clone()),
        challenge,
    };
    send_control(socket, MessageType::Join, &message)
}

fn send_control(
    socket: &UdpSocket,
    message_type: MessageType,
    message: &ControlMessage,
) -> io::Result<()> {
    let payload = message.encode().map_err(io::Error::other)?;
    let bytes = Envelope::new(message_type, payload)
        .encode()
        .map_err(io::Error::other)?;
    socket.send(&bytes)?;
    Ok(())
}

fn forward_hook_packet(
    relay_socket: &UdpSocket,
    steam_id64: &str,
    bytes: Bytes,
    event_tx: &Sender<RuntimeEvent>,
) -> io::Result<PacketSummary> {
    let packet = LocalPacket::decode(bytes).map_err(io::Error::other)?;
    if packet.packet_type != LocalPacketType::Outgoing {
        return Err(io::Error::other("expected outgoing local packet"));
    }
    let summary = PacketSummary {
        peer: packet.peer,
        sequence: packet.sequence,
        source_sequence: packet.sequence,
        channel: packet.channel,
        send_type: packet.send_type,
        payload_bytes: packet.payload.len(),
        wire_bytes: 0,
    };
    let sent_bytes = u64::try_from(packet.payload.len()).unwrap_or(u64::MAX);
    let game = GamePacket::from_local(steam_id64.to_owned(), packet);
    let payload = game.encode().map_err(io::Error::other)?;
    let envelope = Envelope::new(MessageType::Data, payload)
        .encode()
        .map_err(io::Error::other)?;
    let wire_bytes = envelope.len();
    relay_socket.send(&envelope)?;
    send_event(
        event_tx,
        RuntimeEvent::CounterDelta(Counters {
            hook_to_relay: 1,
            sent_bytes,
            ..Counters::default()
        }),
    );
    Ok(PacketSummary {
        wire_bytes,
        ..summary
    })
}

fn forward_relay_packet(
    hook_socket: &UdpSocket,
    hook_out: &str,
    bytes: Bytes,
    local_sequence: &mut u32,
    event_tx: &Sender<RuntimeEvent>,
) -> io::Result<Option<PacketSummary>> {
    let envelope = Envelope::decode(bytes).map_err(io::Error::other)?;
    if envelope.message_type != MessageType::Data {
        return Ok(None);
    }
    let game = GamePacket::decode(&envelope.payload).map_err(io::Error::other)?;
    let peer = game.from_steam_id64.parse::<u64>().unwrap_or_default();
    let received_bytes = u64::try_from(game.payload.len()).unwrap_or(u64::MAX);
    let summary = PacketSummary {
        peer,
        sequence: *local_sequence,
        source_sequence: game.source_sequence,
        channel: game.channel,
        send_type: game.send_type,
        payload_bytes: game.payload.len(),
        wire_bytes: 0,
    };
    let packet = LocalPacket::incoming(peer, *local_sequence, game);
    *local_sequence = local_sequence.saturating_add(1);
    let bytes = packet.encode().map_err(io::Error::other)?;
    let wire_bytes = bytes.len();
    hook_socket.send_to(&bytes, hook_out)?;
    send_event(
        event_tx,
        RuntimeEvent::CounterDelta(Counters {
            relay_to_hook: 1,
            received_bytes,
            ..Counters::default()
        }),
    );
    Ok(Some(PacketSummary {
        wire_bytes,
        ..summary
    }))
}

fn would_wait(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn should_sample_packet(count: u64) -> bool {
    count <= 64 || count.is_multiple_of(1_000)
}

fn observe_packet_gap(
    event_tx: &Sender<RuntimeEvent>,
    direction: &str,
    last_packet_at: &mut Option<Instant>,
) {
    let now = Instant::now();
    if let Some(previous) = last_packet_at.replace(now) {
        let gap = now.duration_since(previous);
        if gap >= Duration::from_millis(200) {
            send_event(
                event_tx,
                RuntimeEvent::Log(
                    LogLevel::Warn,
                    format!("{direction} packet gap: {} ms", gap.as_millis()),
                ),
            );
        }
    }
}

fn observe_source_sequence(
    event_tx: &Sender<RuntimeEvent>,
    last_remote_sequences: &mut HashMap<u64, u32>,
    summary: &PacketSummary,
) {
    if summary.source_sequence == 0 {
        return;
    }
    let Some(previous) = last_remote_sequences.get_mut(&summary.peer) else {
        last_remote_sequences.insert(summary.peer, summary.source_sequence);
        return;
    };
    let expected = previous.saturating_add(1);
    if summary.source_sequence == expected {
        *previous = summary.source_sequence;
        return;
    }
    send_event(
        event_tx,
        RuntimeEvent::Log(
            LogLevel::Warn,
            format!(
                "Relay source sequence gap: from={} previous={} expected={} current={}",
                summary.peer, *previous, expected, summary.source_sequence
            ),
        ),
    );
    if summary.source_sequence > *previous {
        *previous = summary.source_sequence;
    }
}
