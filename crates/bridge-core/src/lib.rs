//! Shared runtime, protocol, diagnostics, and platform helpers for Basement Bridge.

pub mod client;
pub mod diagnostics;
pub mod protocol;
pub mod steam;

pub use client::{
    BridgeClient, ClientError, ConfigError, Counters, DEFAULT_RELAY_PROBE_PAYLOAD_BYTES,
    HookReceiveProbeReport, LogEntry, LogLevel, PRODUCT_NAME, RelayEndpoint, RelayProbeReport,
    RuntimeState, SessionConfig, SessionMode, SessionStatus, SteamIdentity, diagnostics_directory,
    runtime_name,
};
