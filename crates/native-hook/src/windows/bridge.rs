use std::{
    collections::VecDeque,
    ffi::OsString,
    fmt::Display,
    fs,
    io::{self, Read, Write},
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, SYSTEMTIME},
    System::{LibraryLoader::GetModuleFileNameW, SystemInformation::GetLocalTime},
};

use tractor_beam_hook_ipc::{GamePacket, SessionId};

use super::ipc_worker::{self, WorkerCounters};

const RUNTIME_FILE: &str = "hook-runtime.txt";
const DAILY_LOG_RETAIN_COUNT: usize = 10;

static STATE: Mutex<Option<BridgeState>> = Mutex::new(None);
static LOG_LOCK: Mutex<()> = Mutex::new(());
static LOG_DATE: Mutex<Option<String>> = Mutex::new(None);
static NEXT_SEQUENCE: AtomicU32 = AtomicU32::new(1);
static MODULE_HANDLE: AtomicUsize = AtomicUsize::new(0);

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
    ipc_endpoint: String,
    ipc_session: SessionId,
}

struct BridgeState {
    mode: BridgeMode,
    fallback_to_steam: bool,
    data_tx: SyncSender<GamePacket>,
    queue: Arc<Mutex<VecDeque<GamePacket>>>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    counters: Arc<WorkerCounters>,
}

pub fn set_module_handle(module: HINSTANCE) {
    MODULE_HANDLE.store(module as usize, Ordering::Relaxed);
}

pub fn initialize() {
    let Some(config_path) = default_runtime_path() else {
        log_error("bridge_module_directory_unavailable");
        return;
    };
    log_info(format!(
        "bridge_config_path_attempted path={}",
        config_path.display()
    ));
    let Ok(config) = read_config(&config_path) else {
        log_warn(format!(
            "bridge_config_missing path={}",
            config_path.display()
        ));
        return;
    };
    if config.mode == BridgeMode::Off {
        log_info("bridge_mode_off");
        return;
    }

    let queue = Arc::new(Mutex::new(VecDeque::new()));
    let running = Arc::new(AtomicBool::new(true));
    let counters = Arc::new(WorkerCounters::default());
    let (data_tx, data_rx) = sync_channel(tractor_beam_hook_ipc::HOOK_DATA_QUEUE_CAPACITY);
    let worker = ipc_worker::spawn(
        config.ipc_endpoint,
        config.ipc_session,
        data_rx,
        Arc::clone(&queue),
        Arc::clone(&running),
        Arc::clone(&counters),
    );

    let state = BridgeState {
        mode: config.mode,
        fallback_to_steam: config.fallback_to_steam,
        data_tx,
        queue,
        running,
        worker: Some(worker),
        counters,
    };
    *STATE.lock().expect("bridge state lock poisoned") = Some(state);
    log_info(format!(
        "bridge_initialized mode={:?} fallback_to_steam={} ipc_version={}.{} data_queue_capacity={} control_queue_capacity={}",
        config.mode,
        config.fallback_to_steam,
        tractor_beam_hook_ipc::PROTOCOL_MAJOR,
        tractor_beam_hook_ipc::PROTOCOL_MINOR,
        tractor_beam_hook_ipc::HOOK_DATA_QUEUE_CAPACITY,
        tractor_beam_hook_ipc::CONTROL_QUEUE_CAPACITY,
    ));
}

pub fn shutdown() {
    if let Some(mut state) = STATE.lock().expect("bridge state lock poisoned").take() {
        log_info("bridge_shutdown");
        state.running.store(false, Ordering::Relaxed);
        if let Some(worker) = state.worker.take() {
            let _ = worker.join();
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
        return false;
    }
    if len as usize > tractor_beam_hook_ipc::MAX_GAME_PAYLOAD_SIZE {
        return false;
    }
    let Some((data_tx, counters)) = STATE
        .lock()
        .expect("bridge state lock poisoned")
        .as_ref()
        .map(|state| (state.data_tx.clone(), Arc::clone(&state.counters)))
    else {
        return false;
    };
    let packet = GamePacket {
        peer,
        sequence: NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        channel,
        send_type,
        payload: unsafe { std::slice::from_raw_parts(data, len as usize) }.to_vec(),
    };
    match data_tx.try_send(packet) {
        Ok(()) => true,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
            saturating_increment(&counters.hook_data_dropped);
            false
        }
    }
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
    let Ok(_guard) = LOG_LOCK.lock() else {
        return;
    };
    let Some(directory) = hook_log_directory() else {
        return;
    };
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let date = local_date();
    let path = directory.join(format!("{date}.log"));
    let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let should_prune = LOG_DATE.lock().is_ok_and(|mut active_date| {
        if active_date.as_deref() == Some(date.as_str()) {
            false
        } else {
            *active_date = Some(date);
            true
        }
    });
    if should_prune {
        prune_daily_logs(&directory);
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let _ = writeln!(file, "{timestamp} {level} {message}");
}

fn read_config(path: &std::path::Path) -> io::Result<BridgeConfig> {
    let mut contents = String::new();
    fs::File::open(path)?.read_to_string(&mut contents)?;
    let mut mode = BridgeMode::Off;
    let mut fallback_to_steam = true;
    let mut ipc_endpoint = None;
    let mut ipc_session = None;

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
            "ipc_endpoint" => ipc_endpoint = Some(value.trim().to_owned()),
            "ipc_session" => ipc_session = value.trim().parse().ok(),
            _ => {}
        }
    }

    let ipc_endpoint = ipc_endpoint
        .filter(|endpoint| !endpoint.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing IPC endpoint"))?;
    let ipc_session = ipc_session
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid IPC session"))?;

    Ok(BridgeConfig {
        mode,
        fallback_to_steam,
        ipc_endpoint,
        ipc_session,
    })
}

fn default_runtime_path() -> Option<PathBuf> {
    hook_log_directory().map(|directory| directory.join(RUNTIME_FILE))
}

fn hook_log_directory() -> Option<PathBuf> {
    module_directory().map(|directory| directory.join("logs").join("hook"))
}

fn local_date() -> String {
    let mut time = SYSTEMTIME::default();
    unsafe { GetLocalTime(&raw mut time) };
    format!("{:04}-{:02}-{:02}", time.wYear, time.wMonth, time.wDay)
}

fn prune_daily_logs(directory: &std::path::Path) {
    let mut files = fs::read_dir(directory)
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_daily_log_name)
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    for path in files.into_iter().skip(DAILY_LOG_RETAIN_COUNT) {
        let _ = fs::remove_file(path);
    }
}

fn is_daily_log_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 14
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && &bytes[10..] == b".log"
}

fn module_directory() -> Option<PathBuf> {
    let module = MODULE_HANDLE.load(Ordering::Relaxed) as HINSTANCE;
    if module.is_null() {
        return None;
    }
    let mut buffer = vec![0_u16; 260];
    loop {
        let length =
            unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
        if length == 0 {
            return None;
        }
        let length = length as usize;
        if length < buffer.len().saturating_sub(1) {
            return PathBuf::from(OsString::from_wide(&buffer[..length]))
                .parent()
                .map(PathBuf::from);
        }
        buffer.resize(buffer.len().saturating_mul(2), 0);
    }
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

fn saturating_increment(counter: &std::sync::atomic::AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}
