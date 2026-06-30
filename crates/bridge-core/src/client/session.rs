use std::{
    io,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(test)]
use std::path::PathBuf;

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
    hook_lifecycle,
    packet_flow::{
        InboundGamePacket, InboundRelayDatagram, OutboundRelayPacket, PacketObserver,
        decode_inbound_relay_datagram, encode_inbound_local_packet, encode_outbound_relay_packet,
        hook_counter, relay_counter, send_error,
    },
    relay_transport::{RelayTransport, complete_relay_join, send_control},
    session_health::{SessionHealth, SessionHealthSnapshot},
    state::{
        RuntimeEvent, RuntimeEventSender, SessionStopReason, error_counter, log_event, send_event,
    },
};

const EVENT_QUEUE_CAPACITY: usize = 512;
const PACKET_QUEUE_CAPACITY: usize = 256;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const HOOK_BUFFER_SIZE: usize = 65_535;

type SharedSessionHealth = Arc<Mutex<SessionHealth>>;

#[derive(Debug)]
pub(super) struct SessionHandle {
    cancellation: CancellationToken,
    pub(super) events: Receiver<RuntimeEvent>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionNativeHook {
    paths: basement_isaac_injector::NativeHookPaths,
}

struct RuntimeTasks {
    essential: JoinSet<io::Result<()>>,
    health: Option<SharedSessionHealth>,
}

struct RelayTransportTaskContext {
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    health: Option<SharedSessionHealth>,
    health_snapshot_interval: Duration,
    runtime_rtt_interval: Duration,
}

pub(super) fn spawn_bridge_worker(
    config: SessionConfig,
    native_hook_paths: basement_isaac_injector::NativeHookPaths,
) -> io::Result<SessionHandle> {
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
            SessionNativeHook {
                paths: native_hook_paths,
            },
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
    pub(super) fn stop(mut self) -> Vec<RuntimeEvent> {
        self.cancellation.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.events.try_iter().collect()
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
    native_hook: SessionNativeHook,
    cancellation: CancellationToken,
    std_event_tx: mpsc::Sender<RuntimeEvent>,
    startup_tx: SyncSender<io::Result<()>>,
) {
    let (event_tx, event_rx) = tokio_mpsc::channel(EVENT_QUEUE_CAPACITY);
    let event_forwarder = tokio::spawn(forward_events(event_rx, std_event_tx));

    match start_runtime_tasks(&config, native_hook, &cancellation, &event_tx).await {
        Ok(mut runtime_tasks) => {
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

            let stop_reason =
                wait_for_essential_task(&cancellation, &mut runtime_tasks.essential).await;
            cancellation.cancel();
            if let Some(message) = stop_reason {
                send_event(
                    &event_tx,
                    RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded {
                        message: message.clone(),
                    }),
                );
                send_event(&event_tx, log_event(LogLevel::Warn, message));
            }
            shutdown_tasks(runtime_tasks.essential, &event_tx).await;
            emit_health_summary(&event_tx, &runtime_tasks.health);
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
    native_hook: SessionNativeHook,
    cancellation: &CancellationToken,
    event_tx: &RuntimeEventSender,
) -> io::Result<RuntimeTasks> {
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
    let health = config.session_health.enabled.then(|| {
        Arc::new(Mutex::new(SessionHealth::new(
            config.session_health.runtime_rtt_enabled,
            Duration::from_secs(config.session_health.runtime_rtt_timeout_seconds),
            Instant::now(),
        )))
    });
    let mut tasks = JoinSet::new();

    tasks.spawn(hook_in_task(
        hook_in_socket,
        config.steam_id64.clone(),
        outbound_tx,
        event_tx.clone(),
        cancellation.clone(),
        health.clone(),
    ));
    tasks.spawn(relay_transport_task(
        relay,
        outbound_rx,
        inbound_tx,
        RelayTransportTaskContext {
            event_tx: event_tx.clone(),
            cancellation: cancellation.clone(),
            health: health.clone(),
            health_snapshot_interval: Duration::from_secs(
                config.session_health.snapshot_interval_seconds,
            ),
            runtime_rtt_interval: Duration::from_secs(
                config.session_health.runtime_rtt_interval_seconds,
            ),
        },
    ));
    tasks.spawn(hook_out_task(
        hook_out_socket,
        inbound_rx,
        event_tx.clone(),
        cancellation.clone(),
        health.clone(),
    ));

    tokio::spawn(hook_lifecycle::injector_task(
        native_hook.paths,
        event_tx.clone(),
        cancellation.clone(),
    ));

    Ok(RuntimeTasks {
        essential: tasks,
        health,
    })
}

async fn hook_in_task(
    socket: UdpSocket,
    steam_id64: String,
    outbound_tx: TokioSender<OutboundRelayPacket>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    health: Option<SharedSessionHealth>,
) -> io::Result<()> {
    let mut buffer = vec![0_u8; HOOK_BUFFER_SIZE];
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            received = socket.recv_from(&mut buffer) => {
                let (size, _) = received?;
                observe_health(&health, |health| health.observe_hook_in_recv(size, Instant::now()));
                match encode_outbound_relay_packet(&steam_id64, Bytes::copy_from_slice(&buffer[..size])) {
                    Ok(packet) => {
                        let accepted = outbound_tx.try_send(packet).is_ok();
                        observe_health(&health, |health| health.observe_outbound_enqueue(accepted));
                        if !accepted {
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
    context: RelayTransportTaskContext,
) -> io::Result<()> {
    let mut observer = PacketObserver::default();
    let mut heartbeat = time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut health_snapshot = time::interval(context.health_snapshot_interval);
    health_snapshot.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut runtime_rtt = time::interval(context.runtime_rtt_interval);
    runtime_rtt.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            () = context.cancellation.cancelled() => {
                return Ok(());
            }
            Some(packet) = outbound_rx.recv() => {
                let started = Instant::now();
                relay.sender.send_data_datagram(packet.raw).await?;
                observe_health(&context.health, |health| {
                    health.observe_relay_send_duration(started.elapsed());
                });
                send_event(&context.event_tx, RuntimeEvent::CounterDelta(hook_counter(packet.sent_bytes)));
                observer.observe_hook_packet(&context.event_tx, &packet.summary);
            }
            raw = relay.receiver.recv_datagram() => {
                let raw = raw?;
                observe_health(&context.health, |health| {
                    health.observe_relay_recv(raw.len(), Instant::now());
                });
                match decode_inbound_relay_datagram(raw) {
                    Ok(Some(InboundRelayDatagram::Game(packet))) => {
                        observe_health(&context.health, |health| {
                            let peer = packet.game.from_steam_id64.parse::<u64>().unwrap_or_default();
                            health.observe_source_sequence(peer, packet.game.source_sequence);
                        });
                        let accepted = inbound_tx.try_send(packet).is_ok();
                        observe_health(&context.health, |health| health.observe_inbound_enqueue(accepted));
                        if !accepted {
                            send_error(&context.event_tx, "Hook inbound queue is full; dropping relay packet");
                        }
                    }
                    Ok(Some(InboundRelayDatagram::HealthPong { id })) => {
                        observe_health(&context.health, |health| health.observe_health_pong(id, Instant::now()));
                    }
                    Ok(None) => {}
                    Err(error) => send_error(&context.event_tx, format!("Bad relay packet: {error}")),
                }
            }
            _ = heartbeat.tick() => {
                send_control(&mut relay.sender, MessageType::Heartbeat, &ControlMessage::Heartbeat).await?;
            }
            _ = health_snapshot.tick(), if context.health.is_some() => {
                emit_health_snapshot(&context.event_tx, &context.health);
            }
            _ = runtime_rtt.tick(), if context.health.is_some() => {
                if let Some(id) = next_health_ping(&context.health) {
                    send_control(&mut relay.sender, MessageType::Heartbeat, &ControlMessage::HealthPing { id }).await?;
                }
            }
        }
    }
}

async fn hook_out_task(
    socket: UdpSocket,
    mut inbound_rx: TokioReceiver<InboundGamePacket>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    health: Option<SharedSessionHealth>,
) -> io::Result<()> {
    let mut local_sequence = 1_u32;
    let mut observer = PacketObserver::default();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            Some(packet) = inbound_rx.recv() => {
                match encode_inbound_local_packet(packet, &mut local_sequence) {
                    Ok((bytes, summary, received_bytes)) => {
                        let started = Instant::now();
                        socket.send_to(&bytes, HOOK_OUT).await?;
                        observe_health(&health, |health| {
                            health.observe_hook_out_send_duration(started.elapsed());
                        });
                        send_event(&event_tx, RuntimeEvent::CounterDelta(relay_counter(received_bytes)));
                        observer.observe_relay_packet(&event_tx, &summary);
                    }
                    Err(error) => send_error(&event_tx, format!("Bad inbound game packet: {error}")),
                }
            }
        }
    }
}

fn observe_health(health: &Option<SharedSessionHealth>, observe: impl FnOnce(&mut SessionHealth)) {
    let Some(health) = health else {
        return;
    };
    if let Ok(mut health) = health.lock() {
        observe(&mut health);
    }
}

fn next_health_ping(health: &Option<SharedSessionHealth>) -> Option<u64> {
    health
        .as_ref()
        .and_then(|health| health.lock().ok()?.next_health_ping(Instant::now()))
}

fn emit_health_snapshot(event_tx: &RuntimeEventSender, health: &Option<SharedSessionHealth>) {
    if let Some(snapshot) = current_health_snapshot(health) {
        send_event(
            event_tx,
            log_event(LogLevel::Info, snapshot.compact_log_line("Session health")),
        );
        send_event(
            event_tx,
            RuntimeEvent::SessionHealthSnapshot(Box::new(snapshot)),
        );
    }
}

fn emit_health_summary(event_tx: &RuntimeEventSender, health: &Option<SharedSessionHealth>) {
    if let Some(snapshot) = current_health_snapshot(health) {
        send_event(
            event_tx,
            log_event(
                LogLevel::Info,
                snapshot.compact_log_line("Session health summary"),
            ),
        );
        send_event(
            event_tx,
            RuntimeEvent::SessionHealthSummary(Box::new(snapshot)),
        );
    }
}

fn current_health_snapshot(health: &Option<SharedSessionHealth>) -> Option<SessionHealthSnapshot> {
    health
        .as_ref()
        .and_then(|health| Some(health.lock().ok()?.snapshot(Instant::now())))
}

#[cfg(test)]
fn test_native_hook_paths() -> basement_isaac_injector::NativeHookPaths {
    basement_isaac_injector::NativeHookPaths {
        injector: PathBuf::from("basement-isaac-injector.exe"),
        hook: PathBuf::from("basement_native_hook.dll"),
    }
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
#[path = "session_tests.rs"]
mod tests;
