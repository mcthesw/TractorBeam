use std::{
    fmt::{self, Display},
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::mpsc::Sender;

use super::{
    SteamIdentity,
    probe::{HookReceiveProbeReport, ReadinessProbeReport},
};

pub(super) const MAX_IN_MEMORY_LOGS: usize = 2_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counters {
    pub hook_to_relay: u64,
    pub relay_to_hook: u64,
    pub sent_bytes: u64,
    pub received_bytes: u64,
    pub errors: u64,
}

impl Counters {
    pub(super) fn add(&mut self, other: Self) {
        self.hook_to_relay = self.hook_to_relay.saturating_add(other.hook_to_relay);
        self.relay_to_hook = self.relay_to_hook.saturating_add(other.relay_to_hook);
        self.sent_bytes = self.sent_bytes.saturating_add(other.sent_bytes);
        self.received_bytes = self.received_bytes.saturating_add(other.received_bytes);
        self.errors = self.errors.saturating_add(other.errors);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SessionStatus {
    #[default]
    Idle,
    Running,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Display for LogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trace => formatter.write_str("trace"),
            Self::Debug => formatter.write_str("debug"),
            Self::Info => formatter.write_str("info"),
            Self::Warn => formatter.write_str("warn"),
            Self::Error => formatter.write_str("error"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogEntry {
    pub timestamp: u64,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeState {
    pub status: SessionStatus,
    pub counters: Counters,
    pub detected_accounts: Vec<SteamIdentity>,
    pub logs: Vec<LogEntry>,
    pub readiness_probe_running: bool,
    pub hook_probe_running: bool,
    pub latest_readiness_probe: Option<ReadinessProbeReport>,
    pub latest_hook_receive_probe: Option<HookReceiveProbeReport>,
    pub latest_hook_receive_probe_error: Option<String>,
}

#[derive(Debug)]
pub(super) enum RuntimeEvent {
    Log(LogLevel, String),
    CounterDelta(Counters),
    ReadinessProbeFinished(Result<Box<ReadinessProbeReport>, String>),
    HookReceiveProbeFinished(Result<HookReceiveProbeReport, String>),
    Stopped,
}

pub(super) type RuntimeEventSender = Sender<RuntimeEvent>;

pub(super) fn log_event(level: LogLevel, message: impl Into<String>) -> RuntimeEvent {
    RuntimeEvent::Log(level, message.into())
}

pub(super) fn log_entry(level: LogLevel, message: impl Into<String>) -> LogEntry {
    LogEntry {
        timestamp: unix_seconds(),
        level,
        message: message.into(),
    }
}

pub(super) fn trim_logs(logs: &mut Vec<LogEntry>) {
    let overflow = logs.len().saturating_sub(MAX_IN_MEMORY_LOGS);
    if overflow > 0 {
        logs.drain(..overflow);
    }
}

pub(super) fn send_event(sender: &RuntimeEventSender, event: RuntimeEvent) {
    let _ = sender.try_send(event);
}

pub(super) fn error_counter() -> Counters {
    Counters {
        errors: 1,
        ..Counters::default()
    }
}

pub(super) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
