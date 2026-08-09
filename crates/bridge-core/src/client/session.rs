use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
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
    task::{JoinHandle as TokioJoinHandle, JoinSet},
    time::{self, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

use crate::protocol::{ClientControl, PeerPresenceInfo, ProbePhase};

use super::{
    ExternalRelayConfig, LogLevel, SessionConfig, SessionMode, SessionRouteConfig,
    hook_ipc::{self, HookIpcSession, InputDelayCall},
    packet_flow::{
        DeliveryStreamAllocator, InboundGamePacket, InboundRelayDatagram, OutboundGamePacket,
        PacketObserver, PacketSummary, decode_inbound_relay_datagram, encode_inbound_hook_packet,
        network_in_counter, network_out_counter, send_error,
    },
    process_lifecycle,
    relay_transport::{RelayTransport, send_control},
    room_path_quality::RoomPathQuality,
    session_health::{SessionHealth, SessionHealthSnapshot},
    state::{
        HookStartupPhase, HookStartupState, RelayLinkState, RuntimeEvent, RuntimeEventSender,
        SessionStopReason, error_counter, log_event, send_critical_event, send_event,
        try_send_event, unix_seconds,
    },
};

mod data_plane;
mod lan_route;

use data_plane::{
    RelayTransportTaskContext, emit_health_summary, health_snapshot_task, hook_dispatch_task,
    hook_in_task, hook_out_task, observe_health, relay_transport_task,
};
use lan_route::{
    DirectSendObserver, direct_hook_in_task, direct_hook_out_task, direct_monitor_task,
    emit_direct_summary,
};

const EVENT_QUEUE_CAPACITY: usize = 512;
const PACKET_QUEUE_CAPACITY: usize = 256;
#[cfg(test)]
const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

type SharedSessionHealth = Arc<Mutex<SessionHealth>>;

#[derive(Debug)]
pub(super) struct SessionHandle {
    cancellation: CancellationToken,
    pub(super) events: Receiver<RuntimeEvent>,
    ipc_control: Option<SyncSender<InputDelayCall>>,
    commands: Option<tokio_mpsc::UnboundedSender<SessionCommand>>,
    mode: SessionMode,
    worker: Option<JoinHandle<()>>,
}

enum SessionCommand {
    StartGameplay {
        config: Box<SessionConfig>,
        relay_data_plane: Option<RelayRoomDataPlane>,
        reply: SyncSender<io::Result<()>>,
    },
    StopGameplay {
        reply: SyncSender<io::Result<()>>,
    },
}

struct SessionControl {
    ipc: Option<Receiver<InputDelayCall>>,
    gameplay: Option<tokio_mpsc::UnboundedReceiver<SessionCommand>>,
}

#[derive(Debug)]
pub(super) struct RelayRoomHandle {
    cancellation: CancellationToken,
    pub(super) events: Receiver<RuntimeEvent>,
    outbound_tx: TokioSender<OutboundGamePacket>,
    inbound_slot: RelayInboundSlot,
    worker: Option<TokioJoinHandle<io::Result<()>>>,
    event_forwarder: Option<TokioJoinHandle<()>>,
    runtime: tokio::runtime::Runtime,
}

type ActiveRelayInbound = Option<(u64, TokioSender<InboundGamePacket>)>;
type ActiveHookOutbound = Option<(u64, TokioSender<tractor_beam_hook_ipc::GamePacket>)>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AttachmentSend {
    Delivered,
    Detached,
    Full,
}

#[derive(Clone, Debug)]
struct HookOutboundSlot {
    sender: Arc<Mutex<ActiveHookOutbound>>,
    next_generation: Arc<AtomicU64>,
}

impl HookOutboundSlot {
    fn new() -> Self {
        Self {
            sender: Arc::new(Mutex::new(None)),
            next_generation: Arc::new(AtomicU64::new(1)),
        }
    }

    fn attach(&self) -> io::Result<HookOutboundAttachment> {
        let mut current = self
            .sender
            .lock()
            .expect("Hook outbound slot lock poisoned");
        if current.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Gameplay is already attached to the Native Hook",
            ));
        }
        let (sender, receiver) = tokio_mpsc::channel(PACKET_QUEUE_CAPACITY);
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        *current = Some((generation, sender));
        Ok(HookOutboundAttachment {
            receiver: Some(receiver),
            slot: self.clone(),
            generation,
        })
    }

    fn detach(&self, generation: u64) {
        let mut current = self
            .sender
            .lock()
            .expect("Hook outbound slot lock poisoned");
        if current
            .as_ref()
            .is_some_and(|(active, _)| *active == generation)
        {
            *current = None;
        }
    }

    fn try_send(&self, packet: tractor_beam_hook_ipc::GamePacket) -> AttachmentSend {
        let sender = self
            .sender
            .lock()
            .expect("Hook outbound slot lock poisoned")
            .as_ref()
            .map(|(_, sender)| sender.clone());
        sender.map_or(AttachmentSend::Detached, |sender| {
            if sender.try_send(packet).is_ok() {
                AttachmentSend::Delivered
            } else {
                AttachmentSend::Full
            }
        })
    }
}

#[derive(Debug)]
struct HookOutboundAttachment {
    receiver: Option<TokioReceiver<tractor_beam_hook_ipc::GamePacket>>,
    slot: HookOutboundSlot,
    generation: u64,
}

impl Drop for HookOutboundAttachment {
    fn drop(&mut self) {
        self.slot.detach(self.generation);
    }
}

#[derive(Clone, Debug)]
pub(super) struct RelayInboundSlot {
    sender: Arc<Mutex<ActiveRelayInbound>>,
    next_generation: Arc<AtomicU64>,
}

impl RelayInboundSlot {
    fn new() -> Self {
        Self {
            sender: Arc::new(Mutex::new(None)),
            next_generation: Arc::new(AtomicU64::new(1)),
        }
    }

    fn attach(&self, sender: TokioSender<InboundGamePacket>) -> io::Result<u64> {
        let mut current = self
            .sender
            .lock()
            .expect("Relay inbound slot lock poisoned");
        if current.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Relay room gameplay is already attached",
            ));
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        *current = Some((generation, sender));
        Ok(generation)
    }

    fn detach(&self, generation: u64) {
        let mut current = self
            .sender
            .lock()
            .expect("Relay inbound slot lock poisoned");
        if current
            .as_ref()
            .is_some_and(|(active, _)| *active == generation)
        {
            *current = None;
        }
    }

    fn try_send(&self, packet: InboundGamePacket) -> AttachmentSend {
        let sender = self
            .sender
            .lock()
            .expect("Relay inbound slot lock poisoned")
            .as_ref()
            .map(|(_, sender)| sender.clone());
        sender.map_or(AttachmentSend::Detached, |sender| {
            if sender.try_send(packet).is_ok() {
                AttachmentSend::Delivered
            } else {
                AttachmentSend::Full
            }
        })
    }
}

pub(super) enum RelayInboundTarget {
    Fixed(TokioSender<InboundGamePacket>),
    Room(RelayInboundSlot),
}

impl RelayInboundTarget {
    pub(super) fn try_send(&self, packet: InboundGamePacket) -> AttachmentSend {
        match self {
            Self::Fixed(sender) => {
                if sender.try_send(packet).is_ok() {
                    AttachmentSend::Delivered
                } else {
                    AttachmentSend::Full
                }
            }
            Self::Room(slot) => slot.try_send(packet),
        }
    }
}

#[derive(Debug)]
pub(super) struct RelayRoomDataPlane {
    pub(super) outbound_tx: TokioSender<OutboundGamePacket>,
    pub(super) inbound_rx: Option<TokioReceiver<InboundGamePacket>>,
    inbound_slot: RelayInboundSlot,
    generation: u64,
}

impl Drop for RelayRoomDataPlane {
    fn drop(&mut self) {
        self.inbound_slot.detach(self.generation);
    }
}

impl RelayRoomHandle {
    pub(super) fn join(
        route: &ExternalRelayConfig,
        steam_id64: &str,
        display_name: &str,
    ) -> io::Result<Self> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("tractor-beam-room")
            .build()?;
        let (std_event_tx, std_event_rx) = mpsc::channel();
        let (event_tx, event_rx) = tokio_mpsc::channel(EVENT_QUEUE_CAPACITY);
        let event_forwarder = runtime.spawn(supervisor::forward_events(event_rx, std_event_tx));
        let cancellation = CancellationToken::new();
        let (relay, peers) = runtime.block_on(RelayTransport::connect_session(
            route,
            steam_id64,
            display_name,
        ))?;
        send_event(&event_tx, RuntimeEvent::RoomPeersUpdated(peers.clone()));
        send_event(
            &event_tx,
            RuntimeEvent::RelayLinkChanged(RelayLinkState::Connected),
        );
        send_event(
            &event_tx,
            log_event(
                LogLevel::Info,
                format!("Joined relay room with {} peer(s)", peers.len()),
            ),
        );
        let (outbound_tx, outbound_rx) = tokio_mpsc::channel(PACKET_QUEUE_CAPACITY);
        let inbound_slot = RelayInboundSlot::new();
        let worker = runtime.spawn(relay_transport_task(
            relay,
            outbound_rx,
            RelayInboundTarget::Room(inbound_slot.clone()),
            RelayTransportTaskContext {
                event_tx,
                cancellation: cancellation.clone(),
                health: None,
                runtime_rtt_interval: Duration::from_secs(1),
                initial_peers: peers,
            },
        ));
        Ok(Self {
            cancellation,
            events: std_event_rx,
            outbound_tx,
            inbound_slot,
            worker: Some(worker),
            event_forwarder: Some(event_forwarder),
            runtime,
        })
    }

    pub(super) fn attach(&self) -> io::Result<RelayRoomDataPlane> {
        let (inbound_tx, inbound_rx) = tokio_mpsc::channel(PACKET_QUEUE_CAPACITY);
        let generation = self.inbound_slot.attach(inbound_tx)?;
        Ok(RelayRoomDataPlane {
            outbound_tx: self.outbound_tx.clone(),
            inbound_rx: Some(inbound_rx),
            inbound_slot: self.inbound_slot.clone(),
            generation,
        })
    }

    fn finish(&mut self) {
        self.cancellation.cancel();
        let Some(worker) = self.worker.take() else {
            return;
        };
        let _ = self
            .runtime
            .block_on(async { time::timeout(RUNTIME_SHUTDOWN_TIMEOUT, worker).await });
        if let Some(forwarder) = self.event_forwarder.take() {
            let _ = self
                .runtime
                .block_on(async { time::timeout(RUNTIME_SHUTDOWN_TIMEOUT, forwarder).await });
        }
    }
}

impl Drop for RelayRoomHandle {
    fn drop(&mut self) {
        self.finish();
    }
}

#[derive(Clone, Debug)]
pub(super) struct SessionNativeHook {
    pub(super) paths: tractor_beam_isaac_injector::NativeHookPaths,
    pub(super) ipc: HookIpcSession,
    pub(super) preexisting_processes: Vec<tractor_beam_isaac_injector::IsaacProcess>,
}

impl SessionNativeHook {
    pub(super) fn new(
        paths: tractor_beam_isaac_injector::NativeHookPaths,
        ipc: HookIpcSession,
        preexisting_processes: Vec<tractor_beam_isaac_injector::IsaacProcess>,
    ) -> Self {
        Self {
            paths,
            ipc,
            preexisting_processes,
        }
    }
}

struct RuntimeTasks {
    route: Option<RouteTasks>,
    support: JoinSet<io::Result<()>>,
    hook_outbound: Option<HookOutboundSlot>,
    to_hook: Option<hook_ipc::ClientIpcSender>,
}

struct RouteTasks {
    /// Route-wide tasks only. Route adapters own pair-local tasks and absorb a single pair's
    /// failure instead of surfacing it as a gameplay-wide exit here.
    tasks: JoinSet<io::Result<()>>,
    cancellation: CancellationToken,
    health: Option<SharedSessionHealth>,
    direct_monitor: Option<crate::client::lan::LanDataPlaneMonitor>,
    _hook_outbound: Option<HookOutboundAttachment>,
    _relay_data_plane: Option<RelayRoomDataPlane>,
    _lan_data_plane: Option<crate::client::lan::LanDataPlaneAttachment>,
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
            Vec::new(),
        )),
        None,
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
    relay_data_plane: Option<RelayRoomDataPlane>,
) -> SessionHandle {
    let (handle, _startup_rx) = spawn_bridge_worker_handle(config, native_hook, relay_data_plane);
    handle
}

fn spawn_bridge_worker_handle(
    config: SessionConfig,
    native_hook: Option<SessionNativeHook>,
    relay_data_plane: Option<RelayRoomDataPlane>,
) -> (SessionHandle, Receiver<io::Result<()>>) {
    let mode = config.mode;
    let cancellation = CancellationToken::new();
    let (event_tx, event_rx) = mpsc::channel();
    let (startup_tx, startup_rx) = mpsc::sync_channel(1);
    let (ipc_control, ipc_control_rx) = if native_hook.is_some() {
        let (sender, receiver) = hook_ipc::control_channel();
        (Some(sender), Some(receiver))
    } else {
        (None, None)
    };
    let (commands, command_rx) = if native_hook.is_some() {
        let (sender, receiver) = tokio_mpsc::unbounded_channel();
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
            SessionControl {
                ipc: ipc_control_rx,
                gameplay: command_rx,
            },
            worker_cancellation,
            event_tx,
            startup_tx,
            relay_data_plane,
        ));
        runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    });

    (
        SessionHandle {
            cancellation,
            events: event_rx,
            ipc_control,
            commands,
            mode,
            worker: Some(worker),
        },
        startup_rx,
    )
}

impl SessionHandle {
    pub(super) fn is_reusable_for(&self, mode: SessionMode) -> bool {
        self.commands.is_some() && self.mode == mode
    }

    pub(super) fn is_persistent(&self) -> bool {
        self.commands.is_some()
    }

    pub(super) fn start_gameplay(
        &self,
        config: SessionConfig,
        relay_data_plane: Option<RelayRoomDataPlane>,
    ) -> io::Result<()> {
        if config.mode != self.mode {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Native Hook mode cannot change while Isaac is running",
            ));
        }
        let commands = self.commands.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "This gameplay runtime cannot be reattached",
            )
        })?;
        let (reply, response) = mpsc::sync_channel(1);
        commands
            .send(SessionCommand::StartGameplay {
                config: Box::new(config),
                relay_data_plane,
                reply,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Hook runtime has stopped"))?;
        response.recv_timeout(COMMAND_TIMEOUT).map_err(|error| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Gameplay attach did not finish: {error}"),
            )
        })?
    }

    pub(super) fn stop_gameplay(&self) -> io::Result<()> {
        let commands = self.commands.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "This gameplay runtime cannot be detached",
            )
        })?;
        let (reply, response) = mpsc::sync_channel(1);
        commands
            .send(SessionCommand::StopGameplay { reply })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Hook runtime has stopped"))?;
        response.recv_timeout(COMMAND_TIMEOUT).map_err(|error| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Gameplay detach did not finish: {error}"),
            )
        })?
    }

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
            commands: None,
            mode: SessionMode::Official,
            worker: None,
        }
    }

    #[cfg(test)]
    pub(super) fn with_test_persistent_runtime(mode: SessionMode) -> Self {
        let (commands, command_rx) = tokio_mpsc::unbounded_channel();
        drop(command_rx);
        let (_event_tx, events) = mpsc::channel();
        Self {
            cancellation: CancellationToken::new(),
            events,
            ipc_control: None,
            commands: Some(commands),
            mode,
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
