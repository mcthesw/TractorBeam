//! Shared Client runtime, diagnostics, and platform helpers for Tractor Beam.

pub mod build_info;
pub mod client;
pub mod diagnostics;
pub mod steam;

pub use tractor_beam_relay_protocol as protocol;

pub use client::{
    BridgeClient, CLIENT_CONFIG_FILE, ClientConfig, ClientConfigSelection, ClientError,
    ClientIncidentKind, ClientIncidentSnapshot, ClientLogSink, ClientSessionLog,
    ClientSessionLogContext, ConfigError, ConnectionProfile, Counters,
    DEFAULT_RELAY_PROBE_PAYLOAD_BYTES, HookIpcConnectionState, HookIpcState,
    HookReceiveProbeReport, HookStartupPhase, HookStartupState, InputDelayError,
    InputDelayOperation, InputDelayReport, InputDelayStatus, JoinCode, JoinCodeError,
    LightPingReport, LightPingTarget, LoadedClientConfig, LogEntry, LogLevel, PRODUCT_NAME,
    READINESS_PROBE_CONNECTION_PROFILES, READINESS_PROBE_PAYLOAD_BYTES,
    READINESS_PROBE_SAMPLES_PER_CASE, ReadinessProbeCaseReport, ReadinessProbeReport,
    RelayEndpoint, RelayLinkState, RelayPreset, RelayProbeReport, RuntimeState, SessionConfig,
    SessionCredential, SessionHealthConfig, SessionHealthSnapshot, SessionHealthSummary,
    SessionMode, SessionQuality, SessionStatus, SessionStopReason, SteamIdentity, TransportChoice,
    app_data_config_path, bundle_config_path, emit_client_log_event, load_client_config,
    runtime_name, save_client_config_selection,
};
