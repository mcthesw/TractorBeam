use std::{
    error::Error,
    fmt::{self, Display},
};

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum SessionMode {
    Official,
    Fallback,
    Pure,
}

impl Display for SessionMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Official => formatter.write_str("Official"),
            Self::Fallback => formatter.write_str("Fallback"),
            Self::Pure => formatter.write_str("Pure"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub enum TransportChoice {
    Udp,
    #[default]
    Tcp,
}

impl Display for TransportChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Udp => formatter.write_str("UDP"),
            Self::Tcp => formatter.write_str("TCP"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionProfile {
    Udp,
    #[default]
    Tcp,
}

impl ConnectionProfile {
    pub const ALL: [Self; 2] = [Self::Tcp, Self::Udp];

    #[must_use]
    pub const fn transport(self) -> TransportChoice {
        match self {
            Self::Tcp => TransportChoice::Tcp,
            Self::Udp => TransportChoice::Udp,
        }
    }
}

impl Display for ConnectionProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Udp => formatter.write_str("UDP"),
            Self::Tcp => formatter.write_str("TCP"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionHealthConfig {
    pub enabled: bool,
    pub runtime_rtt_enabled: bool,
    pub snapshot_interval_seconds: u64,
    pub runtime_rtt_interval_seconds: u64,
    pub runtime_rtt_timeout_seconds: u64,
}

impl SessionHealthConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.snapshot_interval_seconds == 0 {
            return Err("snapshot_interval_seconds must be greater than 0");
        }
        if self.runtime_rtt_interval_seconds == 0 {
            return Err("runtime_rtt_interval_seconds must be greater than 0");
        }
        if self.runtime_rtt_timeout_seconds == 0 {
            return Err("runtime_rtt_timeout_seconds must be greater than 0");
        }
        Ok(())
    }
}

impl Default for SessionHealthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            runtime_rtt_enabled: true,
            snapshot_interval_seconds: 5,
            runtime_rtt_interval_seconds: 1,
            runtime_rtt_timeout_seconds: 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RelayEndpoint {
    pub host: String,
    pub port: u16,
}

impl RelayEndpoint {
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.host.trim().is_empty() {
            return Err(ConfigError::MissingRelayHost);
        }
        if self.port == 0 {
            return Err(ConfigError::InvalidRelayPort);
        }
        Ok(())
    }
}

impl Display for RelayEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.host, self.port)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteamIdentity {
    pub steam_id64: String,
    pub display_name: String,
    pub most_recent: bool,
}

impl From<crate::steam::SteamAccount> for SteamIdentity {
    fn from(value: crate::steam::SteamAccount) -> Self {
        let display_name = value.display_name().to_owned();
        Self {
            steam_id64: value.steam_id64,
            display_name,
            most_recent: value.most_recent,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionConfig {
    pub relay: RelayEndpoint,
    pub relay_name: Option<String>,
    pub transport: TransportChoice,
    pub room: String,
    pub mode: SessionMode,
    pub steam_id64: String,
    pub display_name: String,
    pub session_health: SessionHealthConfig,
    #[cfg(feature = "internal-test")]
    pub test_run_id: Option<String>,
}

impl SessionConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.mode != SessionMode::Official {
            self.relay.validate()?;
        }
        if self.room.trim().is_empty() {
            return Err(ConfigError::MissingRoom);
        }
        if self.steam_id64.trim().is_empty() {
            return Err(ConfigError::MissingSteamId);
        }
        if !self.steam_id64.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ConfigError::InvalidSteamId);
        }
        self.session_health
            .validate()
            .map_err(|_| ConfigError::InvalidSessionHealth)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    MissingRelayHost,
    InvalidRelayPort,
    MissingRoom,
    MissingSteamId,
    InvalidSteamId,
    InvalidSessionHealth,
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MissingRelayHost => "relay host is required",
            Self::InvalidRelayPort => "relay port is invalid",
            Self::MissingRoom => "room is required",
            Self::MissingSteamId => "SteamID64 is required",
            Self::InvalidSteamId => "SteamID64 must contain digits only",
            Self::InvalidSessionHealth => "session health config is invalid",
        };
        formatter.write_str(message)
    }
}

impl Error for ConfigError {}
