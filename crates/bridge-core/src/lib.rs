//! Shared Client runtime, diagnostics, and platform helpers for Tractor Beam.

pub mod build_info;
pub mod client;
pub mod diagnostics;
pub mod steam;

pub use tractor_beam_direct_protocol as direct_protocol;
pub use tractor_beam_relay_protocol as protocol;

pub use client::{
    BridgeClient, CLIENT_CONFIG_FILE, ClientConfig, ClientConfigSelection, ClientError,
    ClientIncidentKind, ClientIncidentSnapshot, ClientLogSink, ClientSessionLog,
    ClientSessionLogContext, ConfigError, ConnectionProfile, Counters,
    DEFAULT_RELAY_PROBE_PAYLOAD_BYTES, ExternalRelayConfig, HookIpcConnectionState, HookIpcState,
    HookReceiveProbeReport, HookStartupPhase, HookStartupState, InputDelayError,
    InputDelayEvidence, InputDelayEvidenceBlocker, InputDelayOperation, InputDelayReport,
    InputDelayStatus, JoinCode, JoinCodeError, LanAdapterAddress, LanControlPlane, LanDirectConfig,
    LanJoinCode, LanProbeResult, LightPingReport, LightPingTarget, LoadedClientConfig, LogEntry,
    LogLevel, PRODUCT_NAME, QualityConfidence, READINESS_PROBE_CONNECTION_PROFILES,
    READINESS_PROBE_PAYLOAD_BYTES, READINESS_PROBE_SAMPLES_PER_CASE, ReadinessProbeCaseReport,
    ReadinessProbeReport, RelayEndpoint, RelayJoinCode, RelayLinkState, RelayPreset,
    RelayProbeReport, RoomPathQualitySnapshot, RoomPathQualityState, RuntimeState, SessionConfig,
    SessionCredential, SessionHealthConfig, SessionHealthSnapshot, SessionHealthSummary,
    SessionHealthWindow, SessionMode, SessionQuality, SessionQualityReason, SessionRouteConfig,
    SessionStatus, SessionStopReason, SmoothnessReason, SmoothnessSnapshot, SteamIdentity,
    TransportChoice, app_data_config_path, bundle_config_path, emit_client_log_event,
    enumerate_lan_adapter_addresses, load_client_config, runtime_name,
    save_client_config_selection,
};
