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

use crate::protocol::{ClientControl, PeerPresenceInfo, ProbePhase};

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
    room_path_quality::RoomPathQuality,
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

mod supervisor;

use supervisor::*;

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
