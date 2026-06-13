//! Shared runtime, protocol, diagnostics, and platform helpers for Basement Bridge.

pub mod client;
pub mod diagnostics;
pub mod protocol;
pub mod steam;

pub use client::{
    BridgeClient, CLIENT_CONFIG_FILE, ClientConfig, ClientError, ClientLogSink, ClientSessionLog,
    ClientSessionLogContext, ConfigError, Counters, DEFAULT_RELAY_PROBE_PAYLOAD_BYTES,
    HookReceiveProbeReport, LoadedClientConfig, LocalDate, LogEntry, LogLevel, PRODUCT_NAME,
    READINESS_PROBE_PAYLOAD_BYTES, READINESS_PROBE_SAMPLES_PER_CASE, ReadinessProbeCaseReport,
    ReadinessProbeReport, RelayEndpoint, RelayPreset, RelayProbeReport, RuntimeState,
    SessionConfig, SessionHealthSnapshot, SessionHealthSummary, SessionMode, SessionQuality,
    SessionStatus, SteamIdentity, TransportChoice, app_data_config_path, bundle_config_path,
    diagnostics_directory, emit_client_log_event, load_client_config, resolve_room_template,
    runtime_name,
};
