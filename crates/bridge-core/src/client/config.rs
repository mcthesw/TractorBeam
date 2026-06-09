use std::{
    error::Error,
    fmt::{self, Display},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub room: String,
    pub mode: SessionMode,
    pub steam_id64: String,
    pub display_name: String,
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
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MissingRelayHost => "relay host is required",
            Self::InvalidRelayPort => "relay port is invalid",
            Self::MissingRoom => "room is required",
            Self::MissingSteamId => "SteamID64 is required",
            Self::InvalidSteamId => "SteamID64 must contain digits only",
        };
        formatter.write_str(message)
    }
}

impl Error for ConfigError {}
