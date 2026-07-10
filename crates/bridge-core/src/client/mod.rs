//! Local Bridge Client runtime without GUI ownership.

mod config;
mod diagnostics;
mod hook_config;
mod hook_ipc;
mod hook_lifecycle;
mod input_delay;
mod join_code;
mod logging;
mod packet_flow;
mod probe;
mod process_lifecycle;
mod relay_transport;
mod runtime;
mod session;
mod session_config;
mod session_health;
mod state;

pub use config::{
    CLIENT_CONFIG_FILE, ClientConfig, ClientConfigSelection, LoadedClientConfig, LocalDate,
    RelayPreset, app_data_config_path, bundle_config_path, load_client_config,
    resolve_room_template, save_client_config_selection,
};
pub use diagnostics::diagnostics_directory;
pub use input_delay::{InputDelayError, InputDelayOperation, InputDelayReport, InputDelayStatus};
pub use join_code::{JoinCode, JoinCodeError};
pub use logging::{
    ClientLogSink, ClientSessionLog, ClientSessionLogContext, emit_client_log_event,
};
pub use probe::{
    DEFAULT_RELAY_PROBE_PAYLOAD_BYTES, HookReceiveProbeReport, LightPingReport, LightPingTarget,
    READINESS_PROBE_CONNECTION_PROFILES, READINESS_PROBE_PAYLOAD_BYTES,
    READINESS_PROBE_SAMPLES_PER_CASE, ReadinessProbeCaseReport, ReadinessProbeReport,
    RelayProbeReport,
};
pub use runtime::{BridgeClient, ClientError, runtime_name};
pub use session_config::{
    ConfigError, ConnectionProfile, RelayEndpoint, SessionConfig, SessionHealthConfig, SessionMode,
    SteamIdentity, TransportChoice,
};
pub use session_health::{SessionHealthSnapshot, SessionHealthSummary, SessionQuality};
pub use state::{
    ClientIncidentKind, ClientIncidentSnapshot, Counters, HookIpcConnectionState, HookIpcState,
    HookStartupPhase, HookStartupState, LogEntry, LogLevel, RuntimeState, SessionStatus,
    SessionStopReason,
};

pub const PRODUCT_NAME: &str = "Tractor Beam";
