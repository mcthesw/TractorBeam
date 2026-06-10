use std::{
    io,
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
    time::Duration,
};

use bytes::Bytes;
use tokio::{
    net::UdpSocket,
    runtime::Builder,
    sync::mpsc::{self as tokio_mpsc, Receiver as TokioReceiver, Sender as TokioSender},
    task::JoinSet,
    time::{self, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

use crate::protocol::{ControlMessage, MessageType};

use super::{
    LogLevel, SessionConfig,
    hook_config::{HOOK_IN, HOOK_OUT},
    packet_flow::{
        InboundGamePacket, OutboundRelayPacket, PacketObserver, decode_inbound_relay_datagram,
        encode_inbound_local_packet, encode_outbound_relay_packet, hook_counter, relay_counter,
        send_error,
    },
    relay_transport::{RelayTransport, complete_relay_join, send_control},
    state::{RuntimeEvent, RuntimeEventSender, error_counter, log_event, send_event},
};

const EVENT_QUEUE_CAPACITY: usize = 512;
const PACKET_QUEUE_CAPACITY: usize = 256;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const INJECTOR_WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const HOOK_BUFFER_SIZE: usize = 65_535;

#[derive(Debug)]
pub(super) struct SessionHandle {
    cancellation: CancellationToken,
    pub(super) events: Receiver<RuntimeEvent>,
    worker: Option<JoinHandle<()>>,
}

pub(super) fn spawn_bridge_worker(config: SessionConfig) -> io::Result<SessionHandle> {
    let cancellation = CancellationToken::new();
    let (event_tx, event_rx) = mpsc::channel();
    let (startup_tx, startup_rx) = mpsc::sync_channel(1);
    let worker_cancellation = cancellation.clone();

    let worker = thread::spawn(move || {
        let runtime = match Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("basement-bridge-core")
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                send_startup(
                    &startup_tx,
                    Err(io::Error::other(format!("runtime startup failed: {error}"))),
                );
                return;
            }
        };

        runtime.block_on(supervise_session(
            config,
            worker_cancellation,
            event_tx,
            startup_tx,
        ));
        runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    });

    match startup_rx.recv_timeout(STARTUP_TIMEOUT) {
        Ok(Ok(())) => Ok(SessionHandle {
            cancellation,
            events: event_rx,
            worker: Some(worker),
        }),
        Ok(Err(error)) => {
            cancellation.cancel();
            let _ = worker.join();
            Err(error)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            cancellation.cancel();
            let _ = worker.join();
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "bridge runtime startup timed out",
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            cancellation.cancel();
            let _ = worker.join();
            Err(io::Error::other("bridge runtime exited during startup"))
        }
    }
}

impl SessionHandle {
    pub(super) fn stop(mut self) {
        self.cancellation.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn supervise_session(
    config: SessionConfig,
    cancellation: CancellationToken,
    std_event_tx: mpsc::Sender<RuntimeEvent>,
    startup_tx: SyncSender<io::Result<()>>,
) {
    let (event_tx, event_rx) = tokio_mpsc::channel(EVENT_QUEUE_CAPACITY);
    let event_forwarder = tokio::spawn(forward_events(event_rx, std_event_tx));

    match start_runtime_tasks(&config, &cancellation, &event_tx).await {
        Ok(mut tasks) => {
            send_startup(&startup_tx, Ok(()));
            send_event(
                &event_tx,
                log_event(LogLevel::Info, "Local bridge is running"),
            );
            send_event(
                &event_tx,
                log_event(
                    LogLevel::Debug,
                    format!(
                        "Bridge sockets ready: hook_in={HOOK_IN} hook_out={HOOK_OUT} relay={} transport={} packet_queue={PACKET_QUEUE_CAPACITY}",
                        config.relay, config.transport
                    ),
                ),
            );

            let stop_reason = wait_for_essential_task(&cancellation, &mut tasks).await;
            cancellation.cancel();
            if let Some(message) = stop_reason {
                send_event(&event_tx, log_event(LogLevel::Warn, message));
            }
            shutdown_tasks(tasks, &event_tx).await;
        }
        Err(error) => {
            let kind = error.kind();
            let message = error.to_string();
            send_startup(&startup_tx, Err(io::Error::new(kind, message.clone())));
            send_event(
                &event_tx,
                log_event(LogLevel::Error, format!("Bridge runtime failed: {message}")),
            );
            send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
        }
    }

    send_event(&event_tx, RuntimeEvent::Stopped);
    drop(event_tx);
    let _ = event_forwarder.await;
}

async fn start_runtime_tasks(
    config: &SessionConfig,
    cancellation: &CancellationToken,
    event_tx: &RuntimeEventSender,
) -> io::Result<JoinSet<io::Result<()>>> {
    let hook_in_socket = UdpSocket::bind(HOOK_IN).await?;
    let hook_out_socket = UdpSocket::bind("127.0.0.1:0").await?;
    let mut relay = RelayTransport::connect(&config.relay, config.transport).await?;
    let peer_count = complete_relay_join(&mut relay.sender, &mut relay.receiver, config).await?;
    send_event(
        event_tx,
        log_event(
            LogLevel::Info,
            format!("Joined relay room with {peer_count} peer(s)"),
        ),
    );

    let (outbound_tx, outbound_rx) = tokio_mpsc::channel(PACKET_QUEUE_CAPACITY);
    let (inbound_tx, inbound_rx) = tokio_mpsc::channel(PACKET_QUEUE_CAPACITY);
    let mut tasks = JoinSet::new();

    tasks.spawn(hook_in_task(
        hook_in_socket,
        config.steam_id64.clone(),
        outbound_tx,
        event_tx.clone(),
        cancellation.clone(),
    ));
    tasks.spawn(relay_transport_task(
        relay,
        outbound_rx,
        inbound_tx,
        event_tx.clone(),
        cancellation.clone(),
    ));
    tasks.spawn(hook_out_task(
        hook_out_socket,
        inbound_rx,
        event_tx.clone(),
        cancellation.clone(),
    ));

    tokio::spawn(injector_task(event_tx.clone(), cancellation.clone()));

    Ok(tasks)
}

async fn hook_in_task(
    socket: UdpSocket,
    steam_id64: String,
    outbound_tx: TokioSender<OutboundRelayPacket>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) -> io::Result<()> {
    let mut buffer = vec![0_u8; HOOK_BUFFER_SIZE];
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            received = socket.recv_from(&mut buffer) => {
                let (size, _) = received?;
                match encode_outbound_relay_packet(&steam_id64, Bytes::copy_from_slice(&buffer[..size])) {
                    Ok(packet) => {
                        if outbound_tx.try_send(packet).is_err() {
                            send_error(&event_tx, "Relay outbound queue is full; dropping hook packet");
                        }
                    }
                    Err(error) => send_error(&event_tx, format!("Bad hook packet: {error}")),
                }
            }
        }
    }
}

async fn relay_transport_task(
    mut relay: RelayTransport,
    mut outbound_rx: TokioReceiver<OutboundRelayPacket>,
    inbound_tx: TokioSender<InboundGamePacket>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) -> io::Result<()> {
    let mut observer = PacketObserver::default();
    let mut heartbeat = time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            Some(packet) = outbound_rx.recv() => {
                relay.sender.send_datagram(packet.raw).await?;
                send_event(&event_tx, RuntimeEvent::CounterDelta(hook_counter(packet.sent_bytes)));
                observer.observe_hook_packet(&event_tx, &packet.summary);
            }
            raw = relay.receiver.recv_datagram() => {
                match decode_inbound_relay_datagram(raw?) {
                    Ok(Some(packet)) => {
                        if inbound_tx.try_send(packet).is_err() {
                            send_error(&event_tx, "Hook inbound queue is full; dropping relay packet");
                        }
                    }
                    Ok(None) => {}
                    Err(error) => send_error(&event_tx, format!("Bad relay packet: {error}")),
                }
            }
            _ = heartbeat.tick() => {
                send_control(&mut relay.sender, MessageType::Heartbeat, &ControlMessage::Heartbeat).await?;
            }
        }
    }
}

async fn hook_out_task(
    socket: UdpSocket,
    mut inbound_rx: TokioReceiver<InboundGamePacket>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) -> io::Result<()> {
    let mut local_sequence = 1_u32;
    let mut observer = PacketObserver::default();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            Some(packet) = inbound_rx.recv() => {
                match encode_inbound_local_packet(packet, &mut local_sequence) {
                    Ok((bytes, summary, received_bytes)) => {
                        socket.send_to(&bytes, HOOK_OUT).await?;
                        send_event(&event_tx, RuntimeEvent::CounterDelta(relay_counter(received_bytes)));
                        observer.observe_relay_packet(&event_tx, &summary);
                    }
                    Err(error) => send_error(&event_tx, format!("Bad inbound game packet: {error}")),
                }
            }
        }
    }
}

async fn injector_task(event_tx: RuntimeEventSender, cancellation: CancellationToken) {
    send_event(
        &event_tx,
        log_event(LogLevel::Info, "Waiting for Isaac process"),
    );

    let Some(process) = wait_for_isaac_process(&cancellation).await else {
        send_event(
            &event_tx,
            log_event(LogLevel::Info, "Native Hook injection cancelled"),
        );
        return;
    };

    let process_name = process.name.clone();
    let process_id = process.pid;
    let injection = tokio::task::spawn_blocking(move || {
        basement_isaac_injector::resolve_native_hook_paths()
            .and_then(|paths| basement_isaac_injector::run_injector(&paths, process_id))
    });

    tokio::select! {
        () = cancellation.cancelled() => {
            send_event(&event_tx, log_event(LogLevel::Info, "Native Hook injection cancelled"));
        }
        result = time::timeout(INJECTOR_WAIT_TIMEOUT, injection) => {
            match result {
                Ok(Ok(Ok(()))) => send_event(
                    &event_tx,
                    log_event(
                        LogLevel::Info,
                        format!("Native Hook injected into {process_name} ({process_id})"),
                    ),
                ),
                Ok(Ok(Err(error))) => {
                    send_event(
                        &event_tx,
                        log_event(
                            LogLevel::Error,
                            format!("Native Hook injection failed: {}", injection_support_message(&error)),
                        ),
                    );
                    send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
                }
                Ok(Err(error)) => {
                    send_event(
                        &event_tx,
                        log_event(
                            LogLevel::Error,
                            format!("Native Hook injection task failed: {error}"),
                        ),
                    );
                    send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
                }
                Err(_) => {
                    send_event(
                        &event_tx,
                        log_event(
                            LogLevel::Error,
                            "Native Hook injection timed out",
                        ),
                    );
                    send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
                }
            }
        }
    }
}

async fn wait_for_isaac_process(
    cancellation: &CancellationToken,
) -> Option<basement_isaac_injector::IsaacProcess> {
    let wait = async {
        loop {
            if let Some(process) = basement_isaac_injector::find_isaac_process() {
                return Some(process);
            }
            time::sleep(Duration::from_millis(250)).await;
        }
    };

    tokio::select! {
        () = cancellation.cancelled() => None,
        result = time::timeout(INJECTOR_WAIT_TIMEOUT, wait) => result.unwrap_or(None),
    }
}

fn injection_support_message(error: &basement_isaac_injector::InjectorError) -> String {
    let message = error.to_string();
    if is_access_denied(&message) {
        format!(
            "{message}; access denied usually means Bridge GUI, Steam, and Isaac need matching privilege levels or security software allowed the helper"
        )
    } else {
        message
    }
}

fn is_access_denied(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("access is denied")
        || lower.contains("access denied")
        || lower.contains("os error 5")
}

async fn wait_for_essential_task(
    cancellation: &CancellationToken,
    tasks: &mut JoinSet<io::Result<()>>,
) -> Option<String> {
    tokio::select! {
        () = cancellation.cancelled() => None,
        result = tasks.join_next() => match result {
            Some(Ok(Ok(()))) => Some("Bridge session task exited".to_owned()),
            Some(Ok(Err(error))) => Some(format!("Bridge session task failed: {error}")),
            Some(Err(error)) => Some(format!("Bridge session task panicked: {error}")),
            None => Some("Bridge session tasks exited".to_owned()),
        },
    }
}

async fn shutdown_tasks(mut tasks: JoinSet<io::Result<()>>, event_tx: &RuntimeEventSender) {
    if time::timeout(SHUTDOWN_TIMEOUT, drain_tasks(&mut tasks))
        .await
        .is_ok()
    {
        return;
    }
    tasks.abort_all();
    send_event(
        event_tx,
        log_event(
            LogLevel::Warn,
            "Bridge session shutdown timed out; aborted remaining tasks".to_owned(),
        ),
    );
    while tasks.join_next().await.is_some() {}
}

async fn drain_tasks(tasks: &mut JoinSet<io::Result<()>>) {
    while tasks.join_next().await.is_some() {}
}

async fn forward_events(
    mut event_rx: tokio_mpsc::Receiver<RuntimeEvent>,
    std_event_tx: mpsc::Sender<RuntimeEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        if std_event_tx.send(event).is_err() {
            break;
        }
    }
}

fn send_startup(sender: &SyncSender<io::Result<()>>, result: io::Result<()>) {
    let _ = sender.send(result);
}

#[cfg(test)]
mod tests {
    use std::{
        net::{SocketAddr, UdpSocket as StdUdpSocket},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    use bytes::Bytes;

    use crate::protocol::{Envelope, MessageType};

    use super::*;
    use crate::protocol::ControlMessage;

    static SESSION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn session_reports_malformed_hook_packet_and_stops() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        let relay = TestRelay::spawn();
        let handle = spawn_bridge_worker(SessionConfig {
            relay: super::super::RelayEndpoint::new("127.0.0.1", relay.address.port()),
            relay_name: None,
            transport: super::super::TransportChoice::Udp,
            room: "test-room".to_owned(),
            mode: super::super::SessionMode::Pure,
            steam_id64: "76561198000000001".to_owned(),
            display_name: "Test".to_owned(),
        })
        .unwrap();

        let sender = StdUdpSocket::bind("127.0.0.1:0").unwrap();
        sender.send_to(b"not-a-local-packet", HOOK_IN).unwrap();

        let event = recv_matching(&handle.events, |event| {
            matches!(
                event,
                RuntimeEvent::Log(level, message) if *level == LogLevel::Warn && message.contains("Bad hook packet")
            )
        });
        assert!(event.is_some());

        handle.stop();
        relay.stop();
    }

    #[test]
    fn session_start_reports_relay_join_timeout() {
        let _guard = SESSION_TEST_LOCK.lock().unwrap();
        let relay = SilentRelay::spawn();

        let error = spawn_bridge_worker(SessionConfig {
            relay: super::super::RelayEndpoint::new("127.0.0.1", relay.address.port()),
            relay_name: None,
            transport: super::super::TransportChoice::Udp,
            room: "test-room".to_owned(),
            mode: super::super::SessionMode::Pure,
            steam_id64: "76561198000000001".to_owned(),
            display_name: "Test".to_owned(),
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        relay.stop();
    }

    fn recv_matching(
        receiver: &Receiver<RuntimeEvent>,
        predicate: impl Fn(&RuntimeEvent) -> bool,
    ) -> Option<RuntimeEvent> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(event) = receiver.recv_timeout(Duration::from_millis(50))
                && predicate(&event)
            {
                return Some(event);
            }
        }
        None
    }

    struct TestRelay {
        address: SocketAddr,
        stop: Arc<AtomicBool>,
        worker: thread::JoinHandle<()>,
    }

    impl TestRelay {
        fn spawn() -> Self {
            let socket = StdUdpSocket::bind("127.0.0.1:0").unwrap();
            socket
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            let address = socket.local_addr().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = Arc::clone(&stop);
            let worker = thread::spawn(move || run_test_relay(socket, &worker_stop));
            Self {
                address,
                stop,
                worker,
            }
        }

        fn stop(self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = self.worker.join();
        }
    }

    struct SilentRelay {
        address: SocketAddr,
        stop: Arc<AtomicBool>,
        worker: thread::JoinHandle<()>,
    }

    impl SilentRelay {
        fn spawn() -> Self {
            let socket = StdUdpSocket::bind("127.0.0.1:0").unwrap();
            socket
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            let address = socket.local_addr().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = Arc::clone(&stop);
            let worker = thread::spawn(move || {
                let mut buffer = [0_u8; 4096];
                while !worker_stop.load(Ordering::Relaxed) {
                    let _ = socket.recv_from(&mut buffer);
                }
            });
            Self {
                address,
                stop,
                worker,
            }
        }

        fn stop(self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = self.worker.join();
        }
    }

    fn run_test_relay(socket: StdUdpSocket, stop: &AtomicBool) {
        let mut buffer = [0_u8; 4096];
        while !stop.load(Ordering::Relaxed) {
            let Ok((size, address)) = socket.recv_from(&mut buffer) else {
                continue;
            };
            let Ok(envelope) = Envelope::decode(Bytes::copy_from_slice(&buffer[..size])) else {
                continue;
            };
            if envelope.message_type != MessageType::Join {
                continue;
            }
            let Ok(control) = ControlMessage::decode(&envelope.payload) else {
                continue;
            };
            let response = match control {
                ControlMessage::Join {
                    challenge: None, ..
                } => (
                    MessageType::JoinChallenge,
                    ControlMessage::Challenge {
                        token: "token".to_owned(),
                    },
                ),
                ControlMessage::Join {
                    challenge: Some(_), ..
                } => (
                    MessageType::JoinReady,
                    ControlMessage::Ready { peer_count: 1 },
                ),
                _ => continue,
            };
            let payload = response.1.encode().unwrap();
            let raw = Envelope::new(response.0, payload).encode().unwrap();
            socket.send_to(&raw, address).unwrap();
        }
    }
}
