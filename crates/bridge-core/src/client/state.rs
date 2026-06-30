use std::{
    fmt::{self, Display},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::mpsc::Sender;

use super::{
    SessionHealthSnapshot, SteamIdentity,
    probe::{HookReceiveProbeReport, ReadinessProbeReport},
};

pub(super) const MAX_IN_MEMORY_LOGS: usize = 2_000;
const MAX_CLIENT_INCIDENT_SNAPSHOTS: usize = 16;
const INCIDENT_REPEAT_WINDOW_SECONDS: u64 = 60;
const DATA_PLANE_STALL_MIN_ELAPSED_SECONDS: u64 = 15;
const DATA_PLANE_STALL_MIN_PACKETS: u64 = 20;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionStopReason {
    UserStopped,
    GameExited { process_name: String, pid: u32 },
    RuntimeEnded { message: String },
}

impl Display for SessionStopReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserStopped => formatter.write_str("user_stopped"),
            Self::GameExited { process_name, pid } => {
                write!(formatter, "game_exited process={process_name} pid={pid}")
            }
            Self::RuntimeEnded { message } => write!(formatter, "runtime_ended message={message}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HookStartupPhase {
    #[default]
    NotStarted,
    Configured,
    WaitingForIsaac,
    Injecting,
    WaitingForHookEndpoint,
    EndpointReady,
    Ready,
    Failed,
    Cancelled,
}

impl Display for HookStartupPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStarted => formatter.write_str("not_started"),
            Self::Configured => formatter.write_str("configured"),
            Self::WaitingForIsaac => formatter.write_str("waiting_for_isaac"),
            Self::Injecting => formatter.write_str("injecting"),
            Self::WaitingForHookEndpoint => formatter.write_str("waiting_for_hook_endpoint"),
            Self::EndpointReady => formatter.write_str("endpoint_ready"),
            Self::Ready => formatter.write_str("ready"),
            Self::Failed => formatter.write_str("failed"),
            Self::Cancelled => formatter.write_str("cancelled"),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HookStartupState {
    pub phase: HookStartupPhase,
    pub process_name: Option<String>,
    pub pid: Option<u32>,
    pub injector_path: Option<PathBuf>,
    pub hook_path: Option<PathBuf>,
    pub launch_parameters_path: Option<PathBuf>,
    pub endpoint: Option<String>,
    pub injected: bool,
    pub endpoint_ready: bool,
    pub access_denied: bool,
    pub message: Option<String>,
    pub updated_at: u64,
}

impl HookStartupState {
    #[must_use]
    pub fn is_started(&self) -> bool {
        self.phase != HookStartupPhase::NotStarted
    }
}

impl RuntimeState {
    #[must_use]
    pub fn hook_log_path_written(&self) -> Option<PathBuf> {
        self.hook_launch_parameters_path_written
            .as_ref()
            .map(|path| path.with_file_name(crate::diagnostics::BRIDGE_HOOK_LOG))
    }

    pub(super) fn record_session_health_incident(
        &mut self,
        health: &SessionHealthSnapshot,
    ) -> Option<ClientIncidentSnapshot> {
        let incident = ClientIncidentSnapshot::from_health(unix_seconds(), health)?;
        let recently_recorded = self.client_incidents.iter().rev().any(|previous| {
            previous.kind == incident.kind
                && incident.timestamp.saturating_sub(previous.timestamp)
                    < INCIDENT_REPEAT_WINDOW_SECONDS
        });
        if recently_recorded {
            return None;
        }
        self.client_incidents.push(incident.clone());
        let overflow = self
            .client_incidents
            .len()
            .saturating_sub(MAX_CLIENT_INCIDENT_SNAPSHOTS);
        if overflow > 0 {
            self.client_incidents.drain(..overflow);
        }
        Some(incident)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClientIncidentKind {
    DataPlaneStall,
    QueueDrop,
    RuntimeRttTimeout,
    SequenceGap,
}

impl Display for ClientIncidentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DataPlaneStall => formatter.write_str("data_plane_stall"),
            Self::QueueDrop => formatter.write_str("queue_drop"),
            Self::RuntimeRttTimeout => formatter.write_str("runtime_rtt_timeout"),
            Self::SequenceGap => formatter.write_str("sequence_gap"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientIncidentSnapshot {
    pub timestamp: u64,
    pub kind: ClientIncidentKind,
    pub summary: String,
    pub health: SessionHealthSnapshot,
}

impl ClientIncidentSnapshot {
    fn from_health(timestamp: u64, health: &SessionHealthSnapshot) -> Option<Self> {
        let kind = if health.queues.total_dropped() > 0 {
            ClientIncidentKind::QueueDrop
        } else if health.runtime_rtt.timed_out > 0 {
            ClientIncidentKind::RuntimeRttTimeout
        } else if health.source_sequence.gaps > 0 {
            ClientIncidentKind::SequenceGap
        } else if data_plane_stalled(health) {
            ClientIncidentKind::DataPlaneStall
        } else {
            return None;
        };
        Some(Self {
            timestamp,
            kind,
            summary: incident_summary(kind, health),
            health: health.clone(),
        })
    }
}

fn data_plane_stalled(health: &SessionHealthSnapshot) -> bool {
    health.elapsed_seconds >= DATA_PLANE_STALL_MIN_ELAPSED_SECONDS
        && ((health.hook_in_recv.packets >= DATA_PLANE_STALL_MIN_PACKETS
            && health.relay_recv.packets == 0)
            || (health.relay_recv.packets >= DATA_PLANE_STALL_MIN_PACKETS
                && health.hook_out_send_duration.count == 0))
}

fn incident_summary(kind: ClientIncidentKind, health: &SessionHealthSnapshot) -> String {
    let base = format!(
        "elapsed={}s hook_in={} relay_recv={} hook_out_sends={} rtt_sent={} rtt_recv={} rtt_timeout={} queue_drops={} seq_gaps={}",
        health.elapsed_seconds,
        health.hook_in_recv.packets,
        health.relay_recv.packets,
        health.hook_out_send_duration.count,
        health.runtime_rtt.sent,
        health.runtime_rtt.received,
        health.runtime_rtt.timed_out,
        health.queues.total_dropped(),
        health.source_sequence.gaps,
    );
    match kind {
        ClientIncidentKind::DataPlaneStall => {
            format!("{base}; possible missing target or local data-plane stall")
        }
        ClientIncidentKind::QueueDrop => format!("{base}; local queue dropped packets"),
        ClientIncidentKind::RuntimeRttTimeout => format!("{base}; runtime RTT timed out"),
        ClientIncidentKind::SequenceGap => format!("{base}; inbound sequence gaps observed"),
    }
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
    pub latest_hook_receive_probe_warning: Option<String>,
    pub latest_session_health: Option<SessionHealthSnapshot>,
    pub latest_session_health_summary: Option<SessionHealthSnapshot>,
    pub hook_launch_parameters_path_written: Option<PathBuf>,
    pub hook_launch_parameters_cleanup: Option<String>,
    pub hook_startup: HookStartupState,
    pub last_stop_reason: Option<SessionStopReason>,
    pub client_incidents: Vec<ClientIncidentSnapshot>,
}

#[derive(Debug)]
pub(super) enum RuntimeEvent {
    Log(LogLevel, String),
    CounterDelta(Counters),
    ReadinessProbeFinished(Result<Box<ReadinessProbeReport>, String>),
    HookReceiveProbeFinished(Result<HookReceiveProbeReport, String>),
    HookReceiveProbeWarning(String),
    HookStartup(Box<HookStartupState>),
    SessionHealthSnapshot(Box<SessionHealthSnapshot>),
    SessionHealthSummary(Box<SessionHealthSnapshot>),
    SessionEnded(SessionStopReason),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::session_health::{PacketStageSnapshot, QueueHealthSnapshot};

    #[test]
    fn startup_data_plane_stall_is_not_recorded() {
        let mut state = RuntimeState::default();
        let health = SessionHealthSnapshot {
            elapsed_seconds: DATA_PLANE_STALL_MIN_ELAPSED_SECONDS - 1,
            hook_in_recv: PacketStageSnapshot {
                packets: DATA_PLANE_STALL_MIN_PACKETS,
                ..PacketStageSnapshot::default()
            },
            ..SessionHealthSnapshot::default()
        };

        assert!(state.record_session_health_incident(&health).is_none());
        assert!(state.client_incidents.is_empty());
    }

    #[test]
    fn records_and_throttles_data_plane_stall_incidents() {
        let mut state = RuntimeState::default();
        let health = SessionHealthSnapshot {
            elapsed_seconds: DATA_PLANE_STALL_MIN_ELAPSED_SECONDS,
            hook_in_recv: PacketStageSnapshot {
                packets: DATA_PLANE_STALL_MIN_PACKETS,
                ..PacketStageSnapshot::default()
            },
            ..SessionHealthSnapshot::default()
        };

        let incident = state
            .record_session_health_incident(&health)
            .expect("data-plane stall should be recorded");

        assert_eq!(incident.kind, ClientIncidentKind::DataPlaneStall);
        assert!(incident.summary.contains("possible missing target"));
        assert!(state.record_session_health_incident(&health).is_none());
        assert_eq!(state.client_incidents.len(), 1);
    }

    #[test]
    fn queue_drop_incident_takes_priority() {
        let mut state = RuntimeState::default();
        let health = SessionHealthSnapshot {
            elapsed_seconds: DATA_PLANE_STALL_MIN_ELAPSED_SECONDS,
            hook_in_recv: PacketStageSnapshot {
                packets: DATA_PLANE_STALL_MIN_PACKETS,
                ..PacketStageSnapshot::default()
            },
            queues: QueueHealthSnapshot {
                outbound_dropped: 1,
                ..QueueHealthSnapshot::default()
            },
            ..SessionHealthSnapshot::default()
        };

        let incident = state
            .record_session_health_incident(&health)
            .expect("queue drop should be recorded");

        assert_eq!(incident.kind, ClientIncidentKind::QueueDrop);
    }
}
