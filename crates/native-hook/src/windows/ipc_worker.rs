use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{Receiver, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use interprocess::TryClone as _;
use interprocess::local_socket::{GenericNamespaced, prelude::*};
use tractor_beam_hook_ipc::{
    ClientToHook, ErrorCode, FrameDecoder, GamePacket, Handshake, HookToClient, InputDelayCommand,
    IpcHealth, PeerRole, ProtocolError, SessionId,
};

use super::{bridge, input_delay::InputDelayMemoryError};

const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const INITIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(35);
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const IO_POLL_INTERVAL: Duration = Duration::from_millis(10);
const WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const HEALTH_INTERVAL: Duration = Duration::from_secs(1);
const MAX_DATA_BURST: usize = 64;

#[derive(Debug, Default)]
pub(super) struct WorkerCounters {
    pub(super) hook_data_dropped: AtomicU64,
    pub(super) client_data_dropped: AtomicU64,
    pub(super) malformed_frames: AtomicU64,
    pub(super) reconnects: AtomicU32,
}

pub(super) fn spawn(
    endpoint: String,
    session_id: SessionId,
    data_rx: Receiver<GamePacket>,
    inbound: Arc<Mutex<VecDeque<GamePacket>>>,
    running: Arc<AtomicBool>,
    counters: Arc<WorkerCounters>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        if let Err(error) = run(
            &endpoint, session_id, &data_rx, &inbound, &running, &counters,
        ) {
            bridge::log_error(format!("ipc_worker_terminal error={error}"));
        }
    })
}

fn run(
    endpoint: &str,
    session_id: SessionId,
    data_rx: &Receiver<GamePacket>,
    inbound: &Arc<Mutex<VecDeque<GamePacket>>>,
    running: &Arc<AtomicBool>,
    counters: &Arc<WorkerCounters>,
) -> io::Result<()> {
    let started = Instant::now();
    let mut disconnected_at = started;
    let mut connected_once = false;
    while running.load(Ordering::Relaxed) {
        if connect_window_expired(started, disconnected_at, connected_once) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                if connected_once {
                    "local IPC reconnect timed out"
                } else {
                    "initial local IPC connection timed out"
                },
            ));
        }
        match connect(endpoint, session_id) {
            Ok(mut stream) => {
                if connected_once {
                    saturating_increment_u32(&counters.reconnects);
                }
                discard_stale_data(data_rx, counters);
                bridge::log_info(format!(
                    "ipc_connected version={}.{} reconnects={}",
                    tractor_beam_hook_ipc::PROTOCOL_MAJOR,
                    tractor_beam_hook_ipc::PROTOCOL_MINOR,
                    counters.reconnects.load(Ordering::Relaxed)
                ));
                connected_once = true;
                match run_connection(&mut stream, data_rx, inbound, running, counters) {
                    Ok(ConnectionEnd::Shutdown) => return Ok(()),
                    Ok(ConnectionEnd::Disconnected) => {
                        disconnected_at = Instant::now();
                        bridge::log_warn("ipc_disconnected reconnecting=true");
                    }
                    Err(ConnectionError::Protocol(error)) => {
                        saturating_increment_u64(&counters.malformed_frames);
                        return Err(io::Error::new(io::ErrorKind::InvalidData, error));
                    }
                    Err(ConnectionError::Io(error)) => {
                        disconnected_at = Instant::now();
                        bridge::log_warn(format!("ipc_transport_error error={error}"));
                    }
                }
            }
            Err(error) if is_protocol_error(&error) => return Err(error),
            Err(_) => thread::sleep(CONNECT_RETRY_INTERVAL),
        }
    }
    Ok(())
}

fn connect_window_expired(
    started: Instant,
    disconnected_at: Instant,
    connected_once: bool,
) -> bool {
    if connected_once {
        disconnected_at.elapsed() >= RECONNECT_TIMEOUT
    } else {
        started.elapsed() >= INITIAL_CONNECT_TIMEOUT
    }
}

fn connect(endpoint: &str, session_id: SessionId) -> io::Result<LocalSocketStream> {
    let name = endpoint
        .to_ns_name::<GenericNamespaced>()
        .map_err(io::Error::other)?;
    let mut stream = LocalSocketStream::connect(name)?;
    stream.set_nonblocking(true)?;
    write_message(
        &mut stream,
        &HookToClient::Handshake(Handshake::new(PeerRole::NativeHook, session_id)),
    )?;

    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut decoder = FrameDecoder::new();
    loop {
        if Instant::now() >= deadline {
            return Err(protocol_io("local IPC handshake timed out"));
        }
        match read_messages::<ClientToHook>(&mut stream, &mut decoder) {
            Ok(messages) => {
                for message in messages {
                    match message {
                        ClientToHook::Handshake(handshake) => {
                            handshake
                                .validate(PeerRole::BridgeClient, session_id)
                                .map_err(protocol_io)?;
                            write_message(&mut stream, &HookToClient::Ready)?;
                            return Ok(stream);
                        }
                        _ => return Err(protocol_io("expected Bridge Client handshake")),
                    }
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

enum ConnectionError {
    Io(io::Error),
    Protocol(String),
}

fn run_connection(
    stream: &mut LocalSocketStream,
    data_rx: &Receiver<GamePacket>,
    inbound: &Arc<Mutex<VecDeque<GamePacket>>>,
    running: &AtomicBool,
    counters: &Arc<WorkerCounters>,
) -> Result<ConnectionEnd, ConnectionError> {
    let mut read_stream = stream.try_clone().map_err(ConnectionError::Io)?;
    read_stream
        .set_nonblocking(false)
        .map_err(ConnectionError::Io)?;
    let (reader_tx, reader_rx) = std::sync::mpsc::channel::<io::Result<ClientToHook>>();
    let inbound_reader = Arc::clone(inbound);
    let counters_reader = Arc::clone(counters);
    thread::spawn(move || {
        let mut decoder = FrameDecoder::new();
        loop {
            match read_messages::<ClientToHook>(&mut read_stream, &mut decoder) {
                Ok(messages) => {
                    for message in messages {
                        if let ClientToHook::Game(packet) = message {
                            enqueue_inbound(packet, &inbound_reader, &counters_reader);
                        } else {
                            let terminal = message == ClientToHook::Shutdown;
                            if reader_tx.send(Ok(message)).is_err() || terminal {
                                return;
                            }
                        }
                    }
                }
                Err(error) if is_transient(&error) => {}
                Err(error) => {
                    let _ = reader_tx.send(Err(error));
                    return;
                }
            }
        }
    });

    let mut pending_write = None::<PendingWrite>;
    let mut control_outbound = VecDeque::<HookToClient>::new();
    let mut next_health = Instant::now() + HEALTH_INTERVAL;
    while running.load(Ordering::Relaxed) {
        for message in reader_rx.try_iter() {
            match message {
                Ok(message) => match message {
                    ClientToHook::Handshake(_) => {
                        return Err(ConnectionError::Protocol(
                            ProtocolError::UnexpectedMessage("duplicate handshake").to_string(),
                        ));
                    }
                    ClientToHook::Game(packet) => enqueue_inbound(packet, inbound, counters),
                    ClientToHook::InputDelay { id, command } => {
                        control_outbound.push_back(HookToClient::InputDelayResult {
                            id,
                            result: handle_input_delay(command),
                        });
                    }
                    ClientToHook::Ping { id } => {
                        control_outbound.push_back(HookToClient::Pong { id });
                    }
                    ClientToHook::Shutdown => return Ok(ConnectionEnd::Shutdown),
                },
                Err(error) if is_disconnect(&error) => return Ok(ConnectionEnd::Disconnected),
                Err(error) if is_protocol_error(&error) => {
                    return Err(ConnectionError::Protocol(error.to_string()));
                }
                Err(error) => return Err(ConnectionError::Io(error)),
            }
        }

        if let Some(write) = &mut pending_write {
            if write.try_flush(stream).map_err(ConnectionError::Io)? {
                pending_write = None;
            } else {
                thread::sleep(IO_POLL_INTERVAL);
                continue;
            }
        }

        while let Some(message) = control_outbound.pop_front() {
            pending_write = PendingWrite::start(stream, &message).map_err(ConnectionError::Io)?;
            if pending_write.is_some() {
                break;
            }
        }

        if pending_write.is_some() {
            continue;
        }

        if Instant::now() >= next_health {
            pending_write = PendingWrite::start(stream, &HookToClient::Health(health(counters)))
                .map_err(ConnectionError::Io)?;
            next_health = Instant::now() + HEALTH_INTERVAL;
        }

        if pending_write.is_some() {
            continue;
        }

        for _ in 0..MAX_DATA_BURST {
            match data_rx.try_recv() {
                Ok(packet) => {
                    pending_write = PendingWrite::start(stream, &HookToClient::Game(packet))
                        .map_err(ConnectionError::Io)?;
                    if pending_write.is_some() {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(ConnectionEnd::Shutdown),
            }
        }
    }
    let _ = write_message(stream, &HookToClient::Goodbye);
    Ok(ConnectionEnd::Shutdown)
}

struct PendingWrite {
    bytes: Vec<u8>,
    written: usize,
    stalled_since: Instant,
}

impl PendingWrite {
    fn start(stream: &mut impl Write, message: &HookToClient) -> io::Result<Option<PendingWrite>> {
        let bytes = tractor_beam_hook_ipc::encode(message).map_err(protocol_io)?;
        let mut write = PendingWrite {
            bytes,
            written: 0,
            stalled_since: Instant::now(),
        };
        if write.try_flush(stream)? {
            Ok(None)
        } else {
            Ok(Some(write))
        }
    }

    fn try_flush(&mut self, stream: &mut impl Write) -> io::Result<bool> {
        match stream.write(&self.bytes[self.written..]) {
            Ok(0) if self.stalled_since.elapsed() < WRITE_TIMEOUT => Ok(false),
            Ok(0) => Err(tractor_beam_hook_ipc::sync_io::write_timeout()),
            Ok(size) => {
                self.written = self.written.saturating_add(size);
                self.stalled_since = Instant::now();
                Ok(self.written >= self.bytes.len())
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(false),
            Err(error) if is_transient(&error) && self.stalled_since.elapsed() < WRITE_TIMEOUT => {
                Ok(false)
            }
            Err(error) if is_transient(&error) => {
                Err(tractor_beam_hook_ipc::sync_io::write_timeout())
            }
            Err(error) => Err(error),
        }
    }
}

fn enqueue_inbound(
    packet: GamePacket,
    inbound: &Arc<Mutex<VecDeque<GamePacket>>>,
    counters: &WorkerCounters,
) {
    let mut queue = inbound.lock().expect("bridge queue lock poisoned");
    if queue.len() >= tractor_beam_hook_ipc::CLIENT_DATA_QUEUE_CAPACITY {
        saturating_increment_u64(&counters.client_data_dropped);
        return;
    }
    queue.push_back(packet);
}

fn handle_input_delay(command: InputDelayCommand) -> Result<i32, ErrorCode> {
    match command {
        InputDelayCommand::Read => {
            super::input_delay::read_current().map_err(|error| map_input_delay_error(error, false))
        }
        InputDelayCommand::Write(value) => super::input_delay::write_value(value)
            .map_err(|error| map_input_delay_error(error, true)),
    }
}

fn map_input_delay_error(error: InputDelayMemoryError, writing: bool) -> ErrorCode {
    match error {
        InputDelayMemoryError::TargetNotFound => ErrorCode::TargetNotFound,
        InputDelayMemoryError::MemoryAccessFailed if writing => ErrorCode::WriteFailed,
        InputDelayMemoryError::MemoryAccessFailed => ErrorCode::ReadFailed,
        InputDelayMemoryError::Internal => ErrorCode::InternalError,
    }
}

fn discard_stale_data(data_rx: &Receiver<GamePacket>, counters: &WorkerCounters) {
    while data_rx.try_recv().is_ok() {
        saturating_increment_u64(&counters.hook_data_dropped);
    }
}

fn health(counters: &WorkerCounters) -> IpcHealth {
    IpcHealth {
        hook_data_dropped: counters.hook_data_dropped.load(Ordering::Relaxed),
        client_data_dropped: counters.client_data_dropped.load(Ordering::Relaxed),
        malformed_frames: counters.malformed_frames.load(Ordering::Relaxed),
        reconnects: counters.reconnects.load(Ordering::Relaxed),
    }
}

fn write_message(stream: &mut LocalSocketStream, message: &HookToClient) -> io::Result<()> {
    tractor_beam_hook_ipc::sync_io::write_message(stream, message, WRITE_TIMEOUT, IO_POLL_INTERVAL)
}

fn read_messages<T: tractor_beam_hook_ipc::WireMessage>(
    stream: &mut LocalSocketStream,
    decoder: &mut FrameDecoder,
) -> io::Result<Vec<T>> {
    tractor_beam_hook_ipc::sync_io::read_messages(stream, decoder)
}

fn protocol_io(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn is_protocol_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidData
}

fn is_transient(error: &io::Error) -> bool {
    tractor_beam_hook_ipc::sync_io::is_transient(error)
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

fn saturating_increment_u64(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

fn saturating_increment_u32(counter: &AtomicU32) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ZeroThenWrite {
        returned_zero: bool,
        bytes: Vec<u8>,
    }

    impl Write for ZeroThenWrite {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if !self.returned_zero {
                self.returned_zero = true;
                return Ok(0);
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pending_write_resumes_after_zero_progress() {
        let mut writer = ZeroThenWrite {
            returned_zero: false,
            bytes: Vec::new(),
        };
        let mut pending = PendingWrite {
            bytes: b"game-packet".to_vec(),
            written: 0,
            stalled_since: Instant::now(),
        };

        assert!(!pending.try_flush(&mut writer).unwrap());
        assert!(pending.try_flush(&mut writer).unwrap());

        assert_eq!(writer.bytes, b"game-packet");
    }
}
