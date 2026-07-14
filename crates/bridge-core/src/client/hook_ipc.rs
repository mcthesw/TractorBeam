use std::{
    collections::HashMap,
    future::Future,
    io::{self, Read, Write},
    mem,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use interprocess::local_socket::{
    GenericNamespaced, ListenerNonblockingMode, ListenerOptions, prelude::*,
};
use rand::RngExt as _;
use tokio::sync::mpsc::{Receiver as TokioReceiver, Sender as TokioSender};
use tokio_util::sync::CancellationToken;
use tractor_beam_hook_ipc::{
    ClientToHook, ErrorCode, FrameDecoder, GamePacket, Handshake, HookToClient, InputDelayCommand,
    PeerRole, SessionId,
};

use super::state::{
    HookIpcConnectionState, HookIpcState, LogLevel, RuntimeEvent, RuntimeEventSender, log_event,
    send_event, unix_seconds,
};

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const INITIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(240);
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const IO_POLL_INTERVAL: Duration = Duration::from_millis(10);
const WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const LIVENESS_PING_INTERVAL: Duration = Duration::from_millis(250);
const LIVENESS_PONG_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_DATA_BURST: usize = 64;
const INPUT_DELAY_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Clone, Copy)]
struct ListenerSettings {
    accept_poll_interval: Duration,
    initial_connect_timeout: Duration,
    reconnect_timeout: Duration,
}

impl Default for ListenerSettings {
    fn default() -> Self {
        Self {
            accept_poll_interval: ACCEPT_POLL_INTERVAL,
            initial_connect_timeout: INITIAL_CONNECT_TIMEOUT,
            reconnect_timeout: RECONNECT_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HookIpcSession {
    pub(super) endpoint: String,
    pub(super) session_id: SessionId,
}

impl HookIpcSession {
    pub(super) fn generate() -> Self {
        let mut bytes = [0_u8; 16];
        rand::rng().fill(&mut bytes);
        let session_id = SessionId::new(bytes);
        Self {
            endpoint: tractor_beam_hook_ipc::endpoint_name(session_id),
            session_id,
        }
    }

    #[cfg(test)]
    pub(super) fn test() -> Self {
        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&std::process::id().to_le_bytes().repeat(2));
        bytes[8..].copy_from_slice(
            &std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos() as u64)
                .to_le_bytes(),
        );
        let session_id = SessionId::new(bytes);
        Self {
            endpoint: tractor_beam_hook_ipc::endpoint_name(session_id),
            session_id,
        }
    }
}

pub(super) struct InputDelayCall {
    pub(super) id: u32,
    pub(super) command: InputDelayCommand,
    pub(super) response: SyncSender<Result<i32, ErrorCode>>,
}

#[derive(Clone)]
pub(super) struct ClientIpcSender {
    data_tx: SyncSender<GamePacket>,
    dropped: Arc<AtomicU64>,
}

struct ListenerContext {
    session_id: SessionId,
    from_hook_tx: TokioSender<GamePacket>,
    to_hook_rx: Receiver<GamePacket>,
    control_rx: Receiver<InputDelayCall>,
    client_dropped: Arc<AtomicU64>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    settings: ListenerSettings,
}

struct ConnectionContext<'a> {
    from_hook_tx: &'a TokioSender<GamePacket>,
    to_hook_rx: &'a Receiver<GamePacket>,
    control_rx: &'a Receiver<InputDelayCall>,
    client_dropped: &'a AtomicU64,
    event_tx: &'a RuntimeEventSender,
    cancellation: &'a CancellationToken,
}

impl ClientIpcSender {
    pub(super) fn try_send(&self, packet: GamePacket) -> bool {
        match self.data_tx.try_send(packet) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                saturating_increment(&self.dropped);
                false
            }
        }
    }
}

pub(super) fn control_channel() -> (SyncSender<InputDelayCall>, Receiver<InputDelayCall>) {
    mpsc::sync_channel(tractor_beam_hook_ipc::CONTROL_QUEUE_CAPACITY)
}

pub(super) fn request_input_delay(
    control_tx: &SyncSender<InputDelayCall>,
    id: u32,
    command: InputDelayCommand,
) -> io::Result<Result<i32, ErrorCode>> {
    let (response_tx, response_rx) = mpsc::sync_channel(1);
    control_tx
        .try_send(InputDelayCall {
            id,
            command,
            response: response_tx,
        })
        .map_err(|error| match error {
            TrySendError::Full(_) => {
                io::Error::new(io::ErrorKind::WouldBlock, "local IPC control queue is full")
            }
            TrySendError::Disconnected(_) => io::Error::new(
                io::ErrorKind::BrokenPipe,
                "local IPC control worker is unavailable",
            ),
        })?;
    response_rx
        .recv_timeout(INPUT_DELAY_TIMEOUT)
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("local IPC Input Delay response timed out: {error}"),
            )
        })
}

pub(super) fn start(
    session: HookIpcSession,
    control_rx: Receiver<InputDelayCall>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) -> io::Result<(
    TokioReceiver<GamePacket>,
    ClientIpcSender,
    impl Future<Output = io::Result<()>> + Send + 'static,
)> {
    start_with_settings(
        session,
        control_rx,
        event_tx,
        cancellation,
        ListenerSettings::default(),
    )
}

fn start_with_settings(
    session: HookIpcSession,
    control_rx: Receiver<InputDelayCall>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    settings: ListenerSettings,
) -> io::Result<(
    TokioReceiver<GamePacket>,
    ClientIpcSender,
    impl Future<Output = io::Result<()>> + Send + 'static,
)> {
    let name = session
        .endpoint
        .to_ns_name::<GenericNamespaced>()
        .map_err(io::Error::other)?;
    let listener = ListenerOptions::new()
        .name(name)
        .nonblocking(ListenerNonblockingMode::Accept)
        .create_sync()?;
    let (from_hook_tx, from_hook_rx) =
        tokio::sync::mpsc::channel(tractor_beam_hook_ipc::HOOK_DATA_QUEUE_CAPACITY);
    let (to_hook_tx, to_hook_rx) =
        mpsc::sync_channel(tractor_beam_hook_ipc::CLIENT_DATA_QUEUE_CAPACITY);
    let client_dropped = Arc::new(AtomicU64::new(0));
    let sender = ClientIpcSender {
        data_tx: to_hook_tx,
        dropped: Arc::clone(&client_dropped),
    };
    publish_status(&event_tx, status(HookIpcConnectionState::Listening));

    let worker = async move {
        tokio::task::spawn_blocking(move || {
            run_listener(
                listener,
                ListenerContext {
                    session_id: session.session_id,
                    from_hook_tx,
                    to_hook_rx,
                    control_rx,
                    client_dropped,
                    event_tx,
                    cancellation,
                    settings,
                },
            )
        })
        .await
        .map_err(|error| io::Error::other(format!("local IPC worker panicked: {error}")))?
    };
    Ok((from_hook_rx, sender, worker))
}

fn run_listener(listener: LocalSocketListener, context: ListenerContext) -> io::Result<()> {
    let ListenerContext {
        session_id,
        from_hook_tx,
        to_hook_rx,
        control_rx,
        client_dropped,
        event_tx,
        cancellation,
        settings,
    } = context;
    let started = Instant::now();
    let mut disconnected_at = started;
    let mut connected_once = false;
    let mut reconnects = 0_u32;
    loop {
        if cancellation.is_cancelled() {
            reject_pending_controls(&control_rx);
            return Ok(());
        }
        let expired = if connected_once {
            disconnected_at.elapsed() >= settings.reconnect_timeout
        } else {
            started.elapsed() >= settings.initial_connect_timeout
        };
        if expired {
            let message = if connected_once {
                "Native Hook local IPC reconnect timed out"
            } else {
                "Native Hook local IPC connection timed out"
            };
            publish_failure(&event_tx, message);
            reject_pending_controls(&control_rx);
            return Err(io::Error::new(io::ErrorKind::TimedOut, message));
        }

        match listener.accept() {
            Ok(mut stream) => {
                if connected_once {
                    reconnects = reconnects.saturating_add(1);
                }
                drain_data(&to_hook_rx, &client_dropped);
                let (negotiated, decoder, pending_messages) =
                    match server_handshake(&mut stream, session_id) {
                        Ok(handshake) => handshake,
                        Err(error) => {
                            publish_failure(&event_tx, &error.to_string());
                            reject_pending_controls(&control_rx);
                            return Err(error);
                        }
                    };
                connected_once = true;
                publish_status(
                    &event_tx,
                    HookIpcState {
                        connection: HookIpcConnectionState::Connected,
                        negotiated_major: Some(negotiated.major),
                        negotiated_minor: Some(negotiated.minor),
                        reconnects,
                        client_data_dropped: client_dropped.load(Ordering::Relaxed),
                        updated_at: unix_seconds(),
                        ..HookIpcState::default()
                    },
                );
                send_event(
                    &event_tx,
                    log_event(
                        LogLevel::Info,
                        format!(
                            "Native Hook local IPC connected version={}.{} reconnects={reconnects}",
                            negotiated.major, negotiated.minor
                        ),
                    ),
                );
                let connection = ConnectionContext {
                    from_hook_tx: &from_hook_tx,
                    to_hook_rx: &to_hook_rx,
                    control_rx: &control_rx,
                    client_dropped: &client_dropped,
                    event_tx: &event_tx,
                    cancellation: &cancellation,
                };
                match run_connection(
                    &mut stream,
                    &connection,
                    reconnects,
                    decoder,
                    pending_messages,
                ) {
                    Ok(ConnectionEnd::Shutdown) => return Ok(()),
                    Ok(ConnectionEnd::Disconnected) => {
                        disconnected_at = Instant::now();
                        drain_data(&to_hook_rx, &client_dropped);
                        reject_pending_controls(&control_rx);
                        publish_status(
                            &event_tx,
                            HookIpcState {
                                connection: HookIpcConnectionState::Reconnecting,
                                reconnects,
                                client_data_dropped: client_dropped.load(Ordering::Relaxed),
                                updated_at: unix_seconds(),
                                ..HookIpcState::default()
                            },
                        );
                    }
                    Err(error) if is_protocol_error(&error) => {
                        publish_failure(&event_tx, &error.to_string());
                        reject_pending_controls(&control_rx);
                        return Err(error);
                    }
                    Err(_) => {
                        disconnected_at = Instant::now();
                        drain_data(&to_hook_rx, &client_dropped);
                        reject_pending_controls(&control_rx);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                reject_pending_controls(&control_rx);
                thread::sleep(settings.accept_poll_interval);
            }
            Err(error) => return Err(error),
        }
    }
}

mod connection;

use connection::*;

#[cfg(test)]
#[path = "hook_ipc_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "hook_ipc_test_support.rs"]
mod test_support;
