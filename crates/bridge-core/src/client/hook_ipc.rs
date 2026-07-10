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

fn server_handshake(
    stream: &mut LocalSocketStream,
    session_id: SessionId,
) -> io::Result<(
    tractor_beam_hook_ipc::NegotiatedProtocol,
    FrameDecoder,
    Vec<HookToClient>,
)> {
    stream.set_nonblocking(true)?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut decoder = FrameDecoder::new();
    let negotiated = 'handshake: loop {
        if Instant::now() >= deadline {
            return Err(protocol_io("local IPC handshake timed out"));
        }
        match read_messages::<HookToClient>(stream, &mut decoder) {
            Ok(messages) => match messages.as_slice() {
                [HookToClient::Handshake(handshake)] => {
                    break 'handshake (*handshake)
                        .validate(PeerRole::NativeHook, session_id)
                        .map_err(protocol_io)?;
                }
                [] => {}
                _ => return Err(protocol_io("expected one Native Hook handshake")),
            },
            Err(error) if is_transient(&error) => thread::sleep(IO_POLL_INTERVAL),
            Err(error) => return Err(error),
        }
    };
    write_message(
        stream,
        &ClientToHook::Handshake(Handshake::new(PeerRole::BridgeClient, session_id)),
    )?;
    loop {
        if Instant::now() >= deadline {
            return Err(protocol_io("local IPC ready acknowledgement timed out"));
        }
        match read_messages::<HookToClient>(stream, &mut decoder) {
            Ok(messages) => {
                let mut messages = messages.into_iter();
                if let Some(message) = messages.next() {
                    if message == HookToClient::Ready {
                        return Ok((negotiated, decoder, messages.collect()));
                    }
                    return Err(protocol_io("expected Native Hook ready acknowledgement"));
                }
            }
            Err(error) if is_transient(&error) => thread::sleep(IO_POLL_INTERVAL),
            Err(error) => return Err(error),
        }
    }
}

enum ConnectionEnd {
    Shutdown,
    Disconnected,
}

fn run_connection(
    stream: &mut LocalSocketStream,
    context: &ConnectionContext<'_>,
    reconnects: u32,
    mut decoder: FrameDecoder,
    mut pending_messages: Vec<HookToClient>,
) -> io::Result<ConnectionEnd> {
    let mut pending = HashMap::<u32, SyncSender<Result<i32, ErrorCode>>>::new();
    let mut next_ping_at = Instant::now() + LIVENESS_PING_INTERVAL;
    let mut pending_ping = None::<(u32, Instant)>;
    let mut next_ping_id = 1_u32;
    loop {
        if context.cancellation.is_cancelled() {
            let _ = write_message(stream, &ClientToHook::Shutdown);
            reject_pending(&mut pending);
            return Ok(ConnectionEnd::Shutdown);
        }

        while let Ok(call) = context.control_rx.try_recv() {
            write_message(
                stream,
                &ClientToHook::InputDelay {
                    id: call.id,
                    command: call.command,
                },
            )?;
            pending.insert(call.id, call.response);
        }

        let now = Instant::now();
        if pending_ping.is_some_and(|(_, deadline)| now >= deadline) {
            reject_pending(&mut pending);
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Native Hook local IPC liveness check timed out",
            ));
        }
        if pending_ping.is_none() && now >= next_ping_at {
            let id = next_ping_id;
            next_ping_id = next_ping_id.wrapping_add(1);
            write_message(stream, &ClientToHook::Ping { id })?;
            pending_ping = Some((id, now + LIVENESS_PONG_TIMEOUT));
            next_ping_at = now + LIVENESS_PING_INTERVAL;
        }

        let messages = if pending_messages.is_empty() {
            read_messages::<HookToClient>(stream, &mut decoder)
        } else {
            Ok(mem::take(&mut pending_messages))
        };
        match messages {
            Ok(messages) => {
                for message in messages {
                    match message {
                        HookToClient::Handshake(_) | HookToClient::Ready => {
                            reject_pending(&mut pending);
                            return Err(protocol_io("unexpected handshake message after ready"));
                        }
                        HookToClient::Game(packet) => {
                            if context.from_hook_tx.try_send(packet).is_err() {
                                saturating_increment(context.client_dropped);
                            }
                        }
                        HookToClient::InputDelayResult { id, result } => {
                            if let Some(response) = pending.remove(&id) {
                                let _ = response.send(result);
                            } else {
                                reject_pending(&mut pending);
                                return Err(protocol_io("unexpected Input Delay response id"));
                            }
                        }
                        HookToClient::Pong { id } => match pending_ping {
                            Some((expected, _)) if expected == id => pending_ping = None,
                            _ => {
                                reject_pending(&mut pending);
                                return Err(protocol_io("unexpected local IPC liveness response"));
                            }
                        },
                        HookToClient::Health(health) => publish_status(
                            context.event_tx,
                            HookIpcState {
                                connection: HookIpcConnectionState::Connected,
                                negotiated_major: Some(tractor_beam_hook_ipc::PROTOCOL_MAJOR),
                                negotiated_minor: Some(tractor_beam_hook_ipc::PROTOCOL_MINOR),
                                reconnects: reconnects.max(health.reconnects),
                                hook_data_dropped: health.hook_data_dropped,
                                client_data_dropped: context
                                    .client_dropped
                                    .load(Ordering::Relaxed)
                                    .max(health.client_data_dropped),
                                malformed_frames: health.malformed_frames,
                                updated_at: unix_seconds(),
                                ..HookIpcState::default()
                            },
                        ),
                        HookToClient::Goodbye => {
                            reject_pending(&mut pending);
                            return Ok(ConnectionEnd::Disconnected);
                        }
                    }
                }
            }
            Err(error) if is_transient(&error) => {}
            Err(error) if is_disconnect(&error) => {
                reject_pending(&mut pending);
                return Ok(ConnectionEnd::Disconnected);
            }
            Err(error) => {
                reject_pending(&mut pending);
                return Err(error);
            }
        }

        for _ in 0..MAX_DATA_BURST {
            match context.to_hook_rx.try_recv() {
                Ok(packet) => write_message(stream, &ClientToHook::Game(packet))?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    reject_pending(&mut pending);
                    return Ok(ConnectionEnd::Shutdown);
                }
            }
        }
    }
}

fn reject_pending(pending: &mut HashMap<u32, SyncSender<Result<i32, ErrorCode>>>) {
    for (_, response) in pending.drain() {
        let _ = response.send(Err(ErrorCode::NotConnected));
    }
}

fn reject_pending_controls(control_rx: &Receiver<InputDelayCall>) {
    while let Ok(call) = control_rx.try_recv() {
        let _ = call.response.send(Err(ErrorCode::NotConnected));
    }
}

fn drain_data(data_rx: &Receiver<GamePacket>, dropped: &AtomicU64) {
    while data_rx.try_recv().is_ok() {
        saturating_increment(dropped);
    }
}

fn publish_failure(event_tx: &RuntimeEventSender, message: &str) {
    publish_status(
        event_tx,
        HookIpcState {
            connection: HookIpcConnectionState::Failed,
            last_error: Some(message.to_owned()),
            updated_at: unix_seconds(),
            ..HookIpcState::default()
        },
    );
    send_event(
        event_tx,
        log_event(
            LogLevel::Error,
            format!("Native Hook local IPC failed: {message}"),
        ),
    );
}

fn publish_status(event_tx: &RuntimeEventSender, state: HookIpcState) {
    send_event(event_tx, RuntimeEvent::HookIpc(Box::new(state)));
}

fn status(connection: HookIpcConnectionState) -> HookIpcState {
    HookIpcState {
        connection,
        updated_at: unix_seconds(),
        ..HookIpcState::default()
    }
}

fn write_message(stream: &mut LocalSocketStream, message: &ClientToHook) -> io::Result<()> {
    let encoded = tractor_beam_hook_ipc::encode(message).map_err(protocol_io)?;
    write_all_bounded(stream, &encoded)
}

fn write_all_bounded(stream: &mut LocalSocketStream, bytes: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut written = 0;
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "local IPC stream stopped accepting bytes",
                ));
            }
            Ok(size) => written += size,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if is_transient(&error) && Instant::now() < deadline => {
                thread::sleep(IO_POLL_INTERVAL);
            }
            Err(error) if is_transient(&error) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "local IPC write timed out",
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_messages<T: tractor_beam_hook_ipc::WireMessage>(
    stream: &mut LocalSocketStream,
    decoder: &mut FrameDecoder,
) -> io::Result<Vec<T>> {
    let mut buffer = [0_u8; 4_096];
    match stream.read(&mut buffer) {
        #[cfg(windows)]
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "local IPC named pipe has no bytes available",
        )),
        #[cfg(not(windows))]
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "local IPC peer disconnected",
        )),
        Ok(size) => decoder.push(&buffer[..size]).map_err(protocol_io),
        Err(error) => Err(error),
    }
}

fn protocol_io(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn is_protocol_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidData
}

fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn is_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

fn saturating_increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

#[cfg(test)]
#[path = "hook_ipc_tests.rs"]
mod tests;
