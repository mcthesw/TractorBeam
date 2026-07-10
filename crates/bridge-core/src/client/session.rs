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

use tokio::{
    runtime::Builder,
    sync::mpsc::{self as tokio_mpsc, Receiver as TokioReceiver, Sender as TokioSender},
    task::JoinSet,
    time::{self, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

use crate::protocol::v2::ClientControl;

use super::{
    LogLevel, SessionConfig, SessionMode,
    hook_ipc::{self, HookIpcSession, InputDelayCall},
    packet_flow::{
        InboundGamePacket, InboundRelayDatagram, OutboundRelayPacket, PacketObserver,
        decode_inbound_relay_datagram, encode_inbound_hook_packet, encode_outbound_relay_packet,
        hook_counter, relay_counter, send_error,
    },
    process_lifecycle,
    relay_transport::{RelayTransport, send_control},
    session_health::{SessionHealth, SessionHealthSnapshot},
    state::{
        HookStartupPhase, HookStartupState, RelayLinkState, RuntimeEvent, RuntimeEventSender,
        SessionStopReason, error_counter, log_event, send_critical_event, send_event, unix_seconds,
    },
};

mod data_plane;

use data_plane::{
    RelayTransportTaskContext, emit_health_summary, hook_in_task, hook_out_task,
    relay_transport_task,
};

const EVENT_QUEUE_CAPACITY: usize = 512;
const PACKET_QUEUE_CAPACITY: usize = 256;
#[cfg(test)]
const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

type SharedSessionHealth = Arc<Mutex<SessionHealth>>;

#[derive(Debug)]
pub(super) struct SessionHandle {
    cancellation: CancellationToken,
    pub(super) events: Receiver<RuntimeEvent>,
    ipc_control: Option<SyncSender<InputDelayCall>>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionNativeHook {
    pub(super) paths: tractor_beam_isaac_injector::NativeHookPaths,
    pub(super) ipc: HookIpcSession,
}

impl SessionNativeHook {
    pub(super) fn new(
        paths: tractor_beam_isaac_injector::NativeHookPaths,
        ipc: HookIpcSession,
    ) -> Self {
        Self { paths, ipc }
    }
}

struct RuntimeTasks {
    essential: JoinSet<io::Result<()>>,
    support: JoinSet<io::Result<()>>,
    health: Option<SharedSessionHealth>,
}

#[cfg(test)]
pub(super) fn spawn_bridge_worker(
    config: SessionConfig,
    native_hook_paths: tractor_beam_isaac_injector::NativeHookPaths,
) -> io::Result<SessionHandle> {
    let (handle, startup_rx) = spawn_bridge_worker_handle(
        config,
        Some(SessionNativeHook::new(
            native_hook_paths,
            HookIpcSession::test(),
        )),
    );
    let cancellation = handle.cancellation.clone();

    match startup_rx.recv_timeout(STARTUP_TIMEOUT) {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(error)) => {
            cancellation.cancel();
            let mut handle = handle;
            if let Some(worker) = handle.worker.take() {
                let _ = worker.join();
            }
            Err(error)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            cancellation.cancel();
            let mut handle = handle;
            if let Some(worker) = handle.worker.take() {
                let _ = worker.join();
            }
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "bridge runtime startup timed out",
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            cancellation.cancel();
            let mut handle = handle;
            if let Some(worker) = handle.worker.take() {
                let _ = worker.join();
            }
            Err(io::Error::other("bridge runtime exited during startup"))
        }
    }
}

pub(super) fn spawn_bridge_worker_background(
    config: SessionConfig,
    native_hook: Option<SessionNativeHook>,
) -> SessionHandle {
    let (handle, _startup_rx) = spawn_bridge_worker_handle(config, native_hook);
    handle
}

fn spawn_bridge_worker_handle(
    config: SessionConfig,
    native_hook: Option<SessionNativeHook>,
) -> (SessionHandle, Receiver<io::Result<()>>) {
    let cancellation = CancellationToken::new();
    let (event_tx, event_rx) = mpsc::channel();
    let (startup_tx, startup_rx) = mpsc::sync_channel(1);
    let (ipc_control, ipc_control_rx) = if native_hook.is_some() {
        let (sender, receiver) = hook_ipc::control_channel();
        (Some(sender), Some(receiver))
    } else {
        (None, None)
    };
    let worker_cancellation = cancellation.clone();

    let worker = thread::spawn(move || {
        let startup_event_tx = event_tx.clone();
        let runtime = match Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("tractor-beam-core")
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                send_startup(
                    &startup_tx,
                    Err(io::Error::other(format!("runtime startup failed: {error}"))),
                );
                let _ = startup_event_tx.send(log_event(
                    LogLevel::Error,
                    format!("Bridge runtime startup failed: {error}"),
                ));
                let _ =
                    startup_event_tx.send(RuntimeEvent::HookStartup(Box::new(HookStartupState {
                        phase: HookStartupPhase::Failed,
                        message: Some(format!("Bridge runtime startup failed: {error}")),
                        updated_at: unix_seconds(),
                        ..HookStartupState::default()
                    })));
                let _ = startup_event_tx.send(RuntimeEvent::SessionEnded(
                    SessionStopReason::RuntimeEnded {
                        message: format!("Bridge runtime startup failed: {error}"),
                    },
                ));
                let _ = startup_event_tx.send(RuntimeEvent::Stopped);
                return;
            }
        };

        runtime.block_on(supervise_session(
            config,
            native_hook,
            ipc_control_rx,
            worker_cancellation,
            event_tx,
            startup_tx,
        ));
        runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    });

    (
        SessionHandle {
            cancellation,
            events: event_rx,
            ipc_control,
            worker: Some(worker),
        },
        startup_rx,
    )
}

impl SessionHandle {
    pub(super) fn request_input_delay(
        &self,
        id: u32,
        command: tractor_beam_hook_ipc::InputDelayCommand,
    ) -> io::Result<Result<i32, tractor_beam_hook_ipc::ErrorCode>> {
        let Some(control) = &self.ipc_control else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Native Hook local IPC is unavailable",
            ));
        };
        hook_ipc::request_input_delay(control, id, command)
    }

    pub(super) fn stop(mut self) -> Vec<RuntimeEvent> {
        self.cancellation.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.events.try_iter().collect()
    }

    #[cfg(test)]
    pub(super) fn with_test_events(events: Vec<RuntimeEvent>) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        for event in events {
            event_tx
                .send(event)
                .expect("test session event receiver should remain connected");
        }
        Self {
            cancellation: CancellationToken::new(),
            events: event_rx,
            ipc_control: None,
            worker: None,
        }
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.cancellation.cancel();
        drop(self.worker.take());
    }
}

async fn supervise_session(
    config: SessionConfig,
    native_hook: Option<SessionNativeHook>,
    ipc_control_rx: Option<Receiver<InputDelayCall>>,
    cancellation: CancellationToken,
    std_event_tx: mpsc::Sender<RuntimeEvent>,
    startup_tx: SyncSender<io::Result<()>>,
) {
    let (event_tx, event_rx) = tokio_mpsc::channel(EVENT_QUEUE_CAPACITY);
    let event_forwarder = tokio::spawn(forward_events(event_rx, std_event_tx));

    match start_runtime_tasks(
        &config,
        native_hook,
        ipc_control_rx,
        &cancellation,
        &event_tx,
    )
    .await
    {
        Ok(mut runtime_tasks) => {
            send_startup(&startup_tx, Ok(()));
            send_event(
                &event_tx,
                log_event(LogLevel::Info, "Session runtime is running"),
            );
            if config.mode != SessionMode::Official {
                send_event(
                    &event_tx,
                    log_event(
                        LogLevel::Debug,
                        format!(
                            "Bridge local IPC ready: version={}.{} relay={} transport={} packet_queue={PACKET_QUEUE_CAPACITY}",
                            tractor_beam_hook_ipc::PROTOCOL_MAJOR,
                            tractor_beam_hook_ipc::PROTOCOL_MINOR,
                            config.relay,
                            config.transport
                        ),
                    ),
                );
            }

            let stop_reason = wait_for_session_end(
                &cancellation,
                &mut runtime_tasks.essential,
                &mut runtime_tasks.support,
            )
            .await;
            cancellation.cancel();
            if let Some(message) = stop_reason {
                send_critical_event(
                    &event_tx,
                    RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded {
                        message: message.clone(),
                    }),
                )
                .await;
                send_event(&event_tx, log_event(LogLevel::Warn, message));
            }
            shutdown_tasks(runtime_tasks.essential, &event_tx).await;
            shutdown_tasks(runtime_tasks.support, &event_tx).await;
            emit_health_summary(&event_tx, &runtime_tasks.health).await;
        }
        Err(error) => {
            let kind = error.kind();
            let message = error.to_string();
            send_startup(&startup_tx, Err(io::Error::new(kind, message.clone())));
            send_event(
                &event_tx,
                log_event(LogLevel::Error, format!("Bridge runtime failed: {message}")),
            );
            send_event(
                &event_tx,
                RuntimeEvent::HookStartup(Box::new(HookStartupState {
                    phase: HookStartupPhase::Failed,
                    message: Some(format!("Bridge runtime failed: {message}")),
                    updated_at: unix_seconds(),
                    ..HookStartupState::default()
                })),
            );
            send_critical_event(
                &event_tx,
                RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded {
                    message: message.clone(),
                }),
            )
            .await;
            send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
        }
    }

    send_critical_event(&event_tx, RuntimeEvent::Stopped).await;
    drop(event_tx);
    let _ = event_forwarder.await;
}

async fn start_runtime_tasks(
    config: &SessionConfig,
    native_hook: Option<SessionNativeHook>,
    ipc_control_rx: Option<Receiver<InputDelayCall>>,
    cancellation: &CancellationToken,
    event_tx: &RuntimeEventSender,
) -> io::Result<RuntimeTasks> {
    tokio::select! {
        result = start_runtime_tasks_inner(
            config,
            native_hook,
            ipc_control_rx,
            cancellation,
            event_tx,
        ) => result,
        () = cancellation.cancelled() => Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "bridge runtime startup cancelled",
        )),
    }
}

async fn start_runtime_tasks_inner(
    config: &SessionConfig,
    native_hook: Option<SessionNativeHook>,
    ipc_control_rx: Option<Receiver<InputDelayCall>>,
    cancellation: &CancellationToken,
    event_tx: &RuntimeEventSender,
) -> io::Result<RuntimeTasks> {
    if config.mode != SessionMode::Official && native_hook.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Native Hook paths are required outside Official mode",
        ));
    }

    if config.mode == SessionMode::Official {
        let mut support = JoinSet::new();
        support.spawn(process_lifecycle::run(
            None,
            event_tx.clone(),
            cancellation.clone(),
        ));
        return Ok(RuntimeTasks {
            essential: JoinSet::new(),
            support,
            health: None,
        });
    }

    let native_hook = native_hook.expect("Native Hook presence was validated above");
    let ipc_control_rx = ipc_control_rx.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Native Hook local IPC control channel is required outside Official mode",
        )
    })?;
    let (hook_packets_rx, to_hook, ipc_worker) = hook_ipc::start(
        native_hook.ipc,
        ipc_control_rx,
        event_tx.clone(),
        cancellation.clone(),
    )?;
    let mut tasks = JoinSet::new();
    tasks.spawn(ipc_worker);
    let mut support = JoinSet::new();
    support.spawn(process_lifecycle::run(
        Some(native_hook.paths),
        event_tx.clone(),
        cancellation.clone(),
    ));
    let (relay, peers) = RelayTransport::connect_session(config).await?;
    let peer_count = peers.len();
    send_event(event_tx, RuntimeEvent::RoomPeersUpdated(peers));
    send_event(
        event_tx,
        RuntimeEvent::RelayLinkChanged(RelayLinkState::Connected),
    );
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
    tasks.spawn(hook_in_task(
        hook_packets_rx,
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
        to_hook,
        inbound_rx,
        event_tx.clone(),
        cancellation.clone(),
        health.clone(),
    ));

    Ok(RuntimeTasks {
        essential: tasks,
        support,
        health,
    })
}

#[cfg(test)]
fn test_native_hook_paths() -> tractor_beam_isaac_injector::NativeHookPaths {
    tractor_beam_isaac_injector::NativeHookPaths {
        injector: PathBuf::from("tractor-beam-isaac-injector.exe"),
        hook: PathBuf::from("tractor_beam_native_hook.dll"),
    }
}

async fn wait_for_session_end(
    cancellation: &CancellationToken,
    essential: &mut JoinSet<io::Result<()>>,
    support: &mut JoinSet<io::Result<()>>,
) -> Option<String> {
    tokio::select! {
        () = cancellation.cancelled() => None,
        result = essential.join_next(), if !essential.is_empty() => {
            task_exit_message("Bridge session task", cancellation, result)
        }
        result = support.join_next(), if !support.is_empty() => {
            task_exit_message("Bridge lifecycle task", cancellation, result)
        }
    }
}

fn task_exit_message(
    task_name: &str,
    cancellation: &CancellationToken,
    result: Option<Result<io::Result<()>, tokio::task::JoinError>>,
) -> Option<String> {
    if cancellation.is_cancelled() {
        return None;
    }
    match result {
        Some(Ok(Ok(()))) => Some(format!("{task_name} exited")),
        Some(Ok(Err(error))) => Some(format!("{task_name} failed: {error}")),
        Some(Err(error)) => Some(format!("{task_name} panicked: {error}")),
        None => Some(format!("{task_name}s exited")),
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
