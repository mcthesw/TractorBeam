//! Local Bridge Client runtime without GUI ownership.

mod config;
mod diagnostics;
mod hook_config;
mod hook_ipc;
mod hook_lifecycle;
mod input_delay;
mod join_code;
mod lan;
mod logging;
mod packet_flow;
mod probe;
mod process_lifecycle;
mod relay_transport;
mod room_path_quality;
mod runtime;
mod session;
mod session_config;
mod session_health;
mod smoothness;
mod state;
#[cfg(test)]
mod test_relay;

pub use config::{
    CLIENT_CONFIG_FILE, ClientConfig, ClientConfigSelection, LoadedClientConfig, RelayPreset,
    app_data_config_path, bundle_config_path, load_client_config, save_client_config_selection,
};
pub use input_delay::{
    InputDelayError, InputDelayEvidence, InputDelayEvidenceBlocker, InputDelayOperation,
    InputDelayReport, InputDelayStatus,
};
pub use join_code::{JoinCode, JoinCodeError, LanJoinCode, RelayJoinCode, SessionCredential};
pub use lan::{
    LanAdapterAddress, LanControlPlane, LanPeerConnectionState, LanPeerState, LanProbeResult,
    enumerate_lan_adapter_addresses,
};
pub use logging::{
    ClientLogSink, ClientSessionLog, ClientSessionLogContext, emit_client_log_event,
};
pub use probe::{
    DEFAULT_RELAY_PROBE_PAYLOAD_BYTES, HookReceiveProbeReport, LightPingReport, LightPingTarget,
    READINESS_PROBE_CONNECTION_PROFILES, READINESS_PROBE_PAYLOAD_BYTES,
    READINESS_PROBE_SAMPLES_PER_CASE, ReadinessProbeCaseReport, ReadinessProbeReport,
    RelayProbeReport,
};
pub use room_path_quality::{RoomPathQualitySnapshot, RoomPathQualityState};
pub use runtime::{BridgeClient, ClientError, runtime_name};
pub use session_config::{
    ConfigError, ConnectionProfile, ExternalRelayConfig, LanDirectConfig, RelayEndpoint,
    SessionConfig, SessionHealthConfig, SessionMode, SessionRouteConfig, SteamIdentity,
    TransportChoice,
};
pub use session_health::{
    QualityConfidence, SessionHealthSnapshot, SessionHealthSummary, SessionHealthWindow,
    SessionQuality, SessionQualityReason,
};
pub use smoothness::{SmoothnessReason, SmoothnessSnapshot};
pub use state::{
    ClientIncidentKind, ClientIncidentSnapshot, Counters, HookIpcConnectionState, HookIpcState,
    HookStartupPhase, HookStartupState, LogEntry, LogLevel, RelayLinkState, RuntimeState,
    SessionStatus, SessionStopReason,
};

pub const PRODUCT_NAME: &str = "Tractor Beam";
