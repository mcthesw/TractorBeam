//! Local Bridge Client runtime without GUI ownership.

mod config;
mod diagnostics;
mod hook_config;
mod hook_lifecycle;
#[cfg(feature = "internal-test")]
mod internal_test;
mod logging;
mod packet_flow;
mod probe;
mod relay_transport;
mod runtime;
mod session;
mod session_config;
mod session_health;
mod state;

#[cfg(feature = "internal-test")]
pub use config::InternalTestConfig;
pub use config::{
    CLIENT_CONFIG_FILE, ClientConfig, LoadedClientConfig, LocalDate, RelayPreset,
    app_data_config_path, bundle_config_path, load_client_config, resolve_room_template,
};
pub use diagnostics::diagnostics_directory;
#[cfg(feature = "internal-test")]
pub use internal_test::{
    InternalTestReport, InternalTestReportError, InternalTestReportRequest,
    InternalTestReportSession, InternalTestShareCode, InternalTestUploadReceipt,
    new_internal_test_id,
};
pub use logging::{
    ClientLogSink, ClientSessionLog, ClientSessionLogContext, emit_client_log_event,
};
pub use probe::{
    DEFAULT_RELAY_PROBE_PAYLOAD_BYTES, HookReceiveProbeReport, READINESS_PROBE_PAYLOAD_BYTES,
    READINESS_PROBE_SAMPLES_PER_CASE, ReadinessProbeCaseReport, ReadinessProbeReport,
    RelayProbeReport,
};
pub use runtime::{BridgeClient, ClientError, runtime_name};
pub use session_config::{
    ConfigError, RelayEndpoint, SessionConfig, SessionHealthConfig, SessionMode, SteamIdentity,
    TransportChoice,
};
pub use session_health::{SessionHealthSnapshot, SessionHealthSummary, SessionQuality};
pub use state::{
    ClientIncidentKind, ClientIncidentSnapshot, Counters, LogEntry, LogLevel, RuntimeState,
    SessionStatus, SessionStopReason,
};

pub const PRODUCT_NAME: &str = "Basement Bridge";
