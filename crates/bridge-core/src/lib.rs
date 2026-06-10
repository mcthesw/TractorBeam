//! Shared runtime, protocol, diagnostics, and platform helpers for Basement Bridge.

pub mod client;
pub mod diagnostics;
pub mod protocol;
pub mod steam;

pub use client::{
    BridgeClient, CLIENT_CONFIG_FILE, ClientConfig, ClientError, ClientLogSink, ClientSessionLog,
    ClientSessionLogContext, ConfigError, Counters, DEFAULT_READINESS_PROBE_DURATION,
    DEFAULT_RELAY_PROBE_PAYLOAD_BYTES, HookReceiveProbeReport, LoadedClientConfig, LocalDate,
    LogEntry, LogLevel, PRODUCT_NAME, ReadinessProbeOutcome, ReadinessProbeReport, RelayEndpoint,
    RelayPreset, RelayProbeReport, RuntimeState, SessionConfig, SessionMode, SessionStatus,
    SteamIdentity, TransportChoice, app_data_config_path, bundle_config_path,
    diagnostics_directory, emit_client_log_event, load_client_config, resolve_room_template,
    runtime_name,
};
