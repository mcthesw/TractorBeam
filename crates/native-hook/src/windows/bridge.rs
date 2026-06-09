use std::{
    collections::VecDeque,
    env,
    fmt::Display,
    fs,
    io::{self, Read, Write},
    net::{SocketAddr, UdpSocket},
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CONFIG_FILE: &str = "isaac_bridge_config.txt";
const LOG_FILE: &str = "basement_bridge_hook.log";
const LOCAL_MAGIC: &[u8; 4] = b"IBR1";
const LOCAL_HEADER_LEN: usize = 32;
const MAX_PAYLOAD_SIZE: usize = 64 * 1024;
const MAX_QUEUED_PACKETS: usize = 4096;
const MAX_LOG_EVENTS: u32 = 20_000;
const TYPE_OUTGOING: u8 = 1;
const TYPE_INCOMING: u8 = 2;

static STATE: Mutex<Option<BridgeState>> = Mutex::new(None);
static LOG_LOCK: Mutex<()> = Mutex::new(());
static LOG_EVENTS: AtomicU32 = AtomicU32::new(0);
static NEXT_SEQUENCE: AtomicU32 = AtomicU32::new(1);
static LOCAL_OUT_EVENTS: AtomicU32 = AtomicU32::new(0);
static LOCAL_IN_EVENTS: AtomicU32 = AtomicU32::new(0);
static AVAILABLE_HITS: AtomicU32 = AtomicU32::new(0);
static READ_HITS: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookLogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Display for HookLogLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trace => formatter.write_str("trace"),
            Self::Debug => formatter.write_str("debug"),
            Self::Info => formatter.write_str("info"),
            Self::Warn => formatter.write_str("warn"),
            Self::Error => formatter.write_str("error"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeMode {
    Off,
    Replace,
}

#[derive(Clone, Debug)]
struct BridgeConfig {
    mode: BridgeMode,
    fallback_to_steam: bool,
    sidecar: SocketAddr,
    bind: SocketAddr,
}

#[derive(Clone, Debug)]
struct QueuedPacket {
    peer: u64,
    sequence: u32,
    channel: i32,
    send_type: i32,
    payload: Vec<u8>,
}

struct BridgeState {
    mode: BridgeMode,
    fallback_to_steam: bool,
    sidecar: SocketAddr,
    send_socket: UdpSocket,
    queue: Arc<Mutex<VecDeque<QueuedPacket>>>,
    running: Arc<AtomicBool>,
    receiver: Option<JoinHandle<()>>,
}

pub fn initialize() {
    let Ok(config) = read_config() else {
        log_warn("bridge_config_missing");
        return;
    };
    if config.mode == BridgeMode::Off {
        log_info("bridge_mode_off");
        return;
    }

    let Ok(send_socket) = UdpSocket::bind("127.0.0.1:0") else {
        log_error("bridge_send_socket_bind_failed");
        return;
    };
    let Ok(receive_socket) = UdpSocket::bind(config.bind) else {
        log_error(format!(
            "bridge_receive_socket_bind_failed bind={}",
            config.bind
        ));
        return;
    };
    let _ = receive_socket.set_read_timeout(Some(Duration::from_millis(20)));

    let queue = Arc::new(Mutex::new(VecDeque::new()));
    let running = Arc::new(AtomicBool::new(true));
    let receiver = spawn_receiver(receive_socket, Arc::clone(&queue), Arc::clone(&running));

    let state = BridgeState {
        mode: config.mode,
        fallback_to_steam: config.fallback_to_steam,
        sidecar: config.sidecar,
        send_socket,
        queue,
        running,
        receiver: Some(receiver),
    };
    *STATE.lock().expect("bridge state lock poisoned") = Some(state);
    log_info(format!(
        "bridge_initialized mode={:?} fallback_to_steam={} sidecar={} bind={}",
        config.mode, config.fallback_to_steam, config.sidecar, config.bind
    ));
}

pub fn shutdown() {
    if let Some(mut state) = STATE.lock().expect("bridge state lock poisoned").take() {
        log_info("bridge_shutdown");
        state.running.store(false, Ordering::Relaxed);
        if let Some(receiver) = state.receiver.take() {
            let _ = receiver.join();
        }
    }
}

pub fn mode() -> BridgeMode {
    STATE
        .lock()
        .expect("bridge state lock poisoned")
        .as_ref()
        .map_or(BridgeMode::Off, |state| state.mode)
}

pub fn should_fallback_to_steam() -> bool {
    STATE
        .lock()
        .expect("bridge state lock poisoned")
        .as_ref()
        .is_none_or(|state| state.fallback_to_steam)
}

pub fn send_packet(peer: u64, data: *const u8, len: u32, send_type: i32, channel: i32) -> bool {
    if data.is_null() {
        log_warn(format!(
            "local_out_rejected reason=null_data peer={peer} channel={channel} send_type={send_type} bytes={len}"
        ));
        return false;
    }
    if len as usize > MAX_PAYLOAD_SIZE {
        log_warn(format!(
            "local_out_rejected reason=payload_too_large peer={peer} channel={channel} send_type={send_type} bytes={len}"
        ));
        return false;
    }
    let guard = STATE.lock().expect("bridge state lock poisoned");
    let Some(state) = guard.as_ref() else {
        return false;
    };
    let payload = unsafe { std::slice::from_raw_parts(data, len as usize) };
    let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let frame = encode_local_packet(TYPE_OUTGOING, peer, sequence, channel, send_type, payload);
    let sent = state
        .send_socket
        .send_to(&frame, state.sidecar)
        .is_ok_and(|sent| sent == frame.len());
    let level = if sent {
        HookLogLevel::Debug
    } else {
        HookLogLevel::Warn
    };
    let event = LOCAL_OUT_EVENTS.fetch_add(1, Ordering::Relaxed) + 1;
    if !sent || should_sample_packet_event(event) {
        log(
            level,
            format!(
                "local_out event={event} peer={peer} sequence={sequence} channel={channel} send_type={send_type} bytes={len} sent={sent}"
            ),
        );
    }
    sent
}

pub fn has_packet(channel: i32, out_size: *mut u32) -> bool {
    let guard = STATE.lock().expect("bridge state lock poisoned");
    let Some(state) = guard.as_ref() else {
        return false;
    };
    let queue = state.queue.lock().expect("bridge queue lock poisoned");
    let Some(packet) = queue
        .iter()
        .find(|packet| channel_matches(channel, packet.channel))
    else {
        return false;
    };
    if !out_size.is_null() {
        unsafe {
            *out_size = packet.payload.len() as u32;
        }
    }
    let event = AVAILABLE_HITS.fetch_add(1, Ordering::Relaxed) + 1;
    if should_sample_packet_event(event) {
        log_debug(format!(
            "steam_available_bridge_hit event={event} requested_channel={channel} packet_channel={} peer={} sequence={} send_type={} bytes={} queue_len={}",
            packet.channel,
            packet.peer,
            packet.sequence,
            packet.send_type,
            packet.payload.len(),
            queue.len()
        ));
    }
    true
}

pub fn read_packet(
    channel: i32,
    destination: *mut u8,
    max_size: u32,
    out_size: *mut u32,
    out_peer: *mut u64,
) -> bool {
    if destination.is_null() {
        return false;
    }
    let guard = STATE.lock().expect("bridge state lock poisoned");
    let Some(state) = guard.as_ref() else {
        return false;
    };
    let mut queue = state.queue.lock().expect("bridge queue lock poisoned");
    let Some(index) = queue
        .iter()
        .position(|packet| channel_matches(channel, packet.channel))
    else {
        return false;
    };
    let packet = queue.remove(index).expect("queue index exists");
    let copy_len = packet.payload.len().min(max_size as usize);
    unsafe {
        std::ptr::copy_nonoverlapping(packet.payload.as_ptr(), destination, copy_len);
        if !out_size.is_null() {
            *out_size = copy_len as u32;
        }
        if !out_peer.is_null() {
            *out_peer = packet.peer;
        }
    }
    if copy_len < packet.payload.len() {
        log_warn(format!(
            "steam_read_bridge_truncated requested_channel={channel} packet_channel={} peer={} sequence={} send_type={} packet_bytes={} copied_bytes={} queue_len={}",
            packet.channel,
            packet.peer,
            packet.sequence,
            packet.send_type,
            packet.payload.len(),
            copy_len,
            queue.len()
        ));
    } else {
        let event = READ_HITS.fetch_add(1, Ordering::Relaxed) + 1;
        if should_sample_packet_event(event) {
            log_debug(format!(
                "steam_read_bridge_hit event={event} requested_channel={channel} packet_channel={} peer={} sequence={} send_type={} bytes={} queue_len={}",
                packet.channel,
                packet.peer,
                packet.sequence,
                packet.send_type,
                packet.payload.len(),
                queue.len()
            ));
        }
    }
    true
}

pub fn log_trace(message: impl Display) {
    log(HookLogLevel::Trace, message);
}

pub fn log_debug(message: impl Display) {
    log(HookLogLevel::Debug, message);
}

pub fn log_info(message: impl Display) {
    log(HookLogLevel::Info, message);
}

pub fn log_warn(message: impl Display) {
    log(HookLogLevel::Warn, message);
}

pub fn log_error(message: impl Display) {
    log(HookLogLevel::Error, message);
}

pub fn log(level: HookLogLevel, message: impl Display) {
    let event_index = LOG_EVENTS.fetch_add(1, Ordering::Relaxed);
    if event_index >= MAX_LOG_EVENTS {
        return;
    }
    let Ok(_guard) = LOG_LOCK.lock() else {
        return;
    };
    let path = default_config_path().with_file_name(LOG_FILE);
    if let Some(directory) = path.parent() {
        let _ = fs::create_dir_all(directory);
    }
    let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let _ = writeln!(file, "{timestamp} {level} {message}");
}

fn spawn_receiver(
    socket: UdpSocket,
    queue: Arc<Mutex<VecDeque<QueuedPacket>>>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = vec![0_u8; LOCAL_HEADER_LEN + MAX_PAYLOAD_SIZE];
        while running.load(Ordering::Relaxed) {
            match socket.recv_from(&mut buffer) {
                Ok((size, _)) => {
                    if let Some(packet) = decode_incoming(&buffer[..size]) {
                        let mut queue = queue.lock().expect("bridge queue lock poisoned");
                        if queue.len() >= MAX_QUEUED_PACKETS {
                            queue.pop_front();
                            log_warn(format!(
                                "local_in_queue_dropped max_queued={MAX_QUEUED_PACKETS}"
                            ));
                        }
                        let event = LOCAL_IN_EVENTS.fetch_add(1, Ordering::Relaxed) + 1;
                        if should_sample_packet_event(event) {
                            log_debug(format!(
                                "local_in event={event} peer={} sequence={} channel={} send_type={} bytes={} queue_len={}",
                                packet.peer,
                                packet.sequence,
                                packet.channel,
                                packet.send_type,
                                packet.payload.len(),
                                queue.len() + 1
                            ));
                        }
                        queue.push_back(packet);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => {
                    log_error(format!("local_in_socket_error error={error}"));
                    break;
                }
            }
        }
    })
}

fn read_config() -> io::Result<BridgeConfig> {
    let mut contents = String::new();
    fs::File::open(default_config_path())?.read_to_string(&mut contents)?;
    let mut mode = BridgeMode::Off;
    let mut fallback_to_steam = true;
    let mut sidecar = SocketAddr::from_str("127.0.0.1:25900").expect("static endpoint");
    let mut bind = SocketAddr::from_str("127.0.0.1:25901").expect("static endpoint");

    for line in contents.lines().map(str::trim) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "mode" => {
                mode = match value.trim().to_ascii_lowercase().as_str() {
                    "replace" | "mirror" => BridgeMode::Replace,
                    _ => BridgeMode::Off,
                };
            }
            "fallback_to_steam" => fallback_to_steam = parse_bool(value, true),
            "sidecar" => {
                if let Ok(parsed) = value.trim().parse() {
                    sidecar = parsed;
                }
            }
            "bind" => {
                if let Ok(parsed) = value.trim().parse() {
                    bind = parsed;
                }
            }
            _ => {}
        }
    }

    Ok(BridgeConfig {
        mode,
        fallback_to_steam,
        sidecar,
        bind,
    })
}

fn default_config_path() -> PathBuf {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Documents")
        .join("My Games")
        .join("Binding of Isaac Repentance+")
        .join("online_logs")
        .join(CONFIG_FILE)
}

fn parse_bool(value: &str, fallback: bool) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => fallback,
    }
}

fn channel_matches(requested: i32, packet: i32) -> bool {
    requested == packet
}

fn should_sample_packet_event(event: u32) -> bool {
    event <= 64 || event.is_multiple_of(1_000)
}

fn encode_local_packet(
    packet_type: u8,
    peer: u64,
    sequence: u32,
    channel: i32,
    send_type: i32,
    payload: &[u8],
) -> Vec<u8> {
    let mut frame = Vec::with_capacity(LOCAL_HEADER_LEN + payload.len());
    frame.extend_from_slice(LOCAL_MAGIC);
    frame.push(1);
    frame.push(packet_type);
    frame.extend_from_slice(&(LOCAL_HEADER_LEN as u16).to_le_bytes());
    frame.extend_from_slice(&peer.to_le_bytes());
    frame.extend_from_slice(&sequence.to_le_bytes());
    frame.extend_from_slice(&channel.to_le_bytes());
    frame.extend_from_slice(&send_type.to_le_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn decode_incoming(bytes: &[u8]) -> Option<QueuedPacket> {
    if bytes.len() < LOCAL_HEADER_LEN
        || &bytes[0..4] != LOCAL_MAGIC
        || bytes[4] != 1
        || bytes[5] != TYPE_INCOMING
    {
        return None;
    }
    let header_len = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as usize;
    let peer = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let sequence = u32::from_le_bytes(bytes[16..20].try_into().ok()?);
    let channel = i32::from_le_bytes(bytes[20..24].try_into().ok()?);
    let send_type = i32::from_le_bytes(bytes[24..28].try_into().ok()?);
    let payload_len = u32::from_le_bytes(bytes[28..32].try_into().ok()?) as usize;
    if header_len < LOCAL_HEADER_LEN
        || payload_len > MAX_PAYLOAD_SIZE
        || bytes.len() < header_len + payload_len
    {
        return None;
    }
    Some(QueuedPacket {
        peer,
        sequence,
        channel,
        send_type,
        payload: bytes[header_len..header_len + payload_len].to_vec(),
    })
}
