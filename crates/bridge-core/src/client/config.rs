use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use chrono::{Datelike, Local};
use directories::ProjectDirs;
use serde::Deserialize;

use super::{
    PRODUCT_NAME,
    session_config::{RelayEndpoint, SessionHealthConfig, SessionMode, TransportChoice},
};

pub const CLIENT_CONFIG_FILE: &str = "config.toml";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadedClientConfig {
    pub config: ClientConfig,
    pub source: Option<PathBuf>,
    pub resolved_default_room: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientConfig {
    pub default_room: Option<String>,
    pub default_room_template: Option<String>,
    pub default_transport: TransportChoice,
    pub default_mode: SessionMode,
    pub selected_relay: Option<String>,
    pub relays: Vec<RelayPreset>,
    pub session_health: SessionHealthConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            default_room: None,
            default_room_template: None,
            default_transport: TransportChoice::Udp,
            default_mode: SessionMode::Pure,
            selected_relay: None,
            relays: Vec::new(),
            session_health: SessionHealthConfig::default(),
        }
    }
}

impl ClientConfig {
    pub fn resolved_room_for_startup(&self) -> Result<Option<String>, ClientConfigError> {
        if let Some(template) = self.default_room_template.as_deref() {
            return resolve_room_template(template, LocalDate::today()).map(Some);
        }
        Ok(self.default_room.clone())
    }

    pub fn selected_relay_index(&self) -> Option<usize> {
        let selected = self.selected_relay.as_deref()?;
        self.relays.iter().position(|relay| relay.id == selected)
    }

    fn validate(&self) -> Result<(), ClientConfigError> {
        let mut ids = HashSet::new();
        for relay in &self.relays {
            relay.validate()?;
            if !ids.insert(relay.id.as_str()) {
                return Err(ClientConfigError::DuplicateRelayId(relay.id.clone()));
            }
        }
        if let Some(selected) = self.selected_relay.as_deref()
            && !self.relays.iter().any(|relay| relay.id == selected)
        {
            return Err(ClientConfigError::UnknownSelectedRelay(selected.to_owned()));
        }
        self.resolved_room_for_startup()?;
        self.session_health
            .validate()
            .map_err(|message| ClientConfigError::InvalidSessionHealth(message.to_owned()))?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayPreset {
    pub id: String,
    pub name: String,
    pub endpoint: RelayEndpoint,
    pub supports_udp: bool,
    pub supports_tcp: bool,
    pub default_transport: Option<TransportChoice>,
}

impl RelayPreset {
    #[must_use]
    pub fn supports(&self, transport: TransportChoice) -> bool {
        match transport {
            TransportChoice::Udp => self.supports_udp,
            TransportChoice::Tcp => self.supports_tcp,
        }
    }

    #[must_use]
    pub fn preferred_transport(&self, fallback: TransportChoice) -> TransportChoice {
        if let Some(transport) = self.default_transport
            && self.supports(transport)
        {
            return transport;
        }
        if self.supports(fallback) {
            return fallback;
        }
        if self.supports_tcp {
            TransportChoice::Tcp
        } else {
            TransportChoice::Udp
        }
    }

    #[must_use]
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.endpoint)
    }

    fn validate(&self) -> Result<(), ClientConfigError> {
        if self.id.trim().is_empty() {
            return Err(ClientConfigError::InvalidRelay(
                "relay id is required".to_owned(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(ClientConfigError::InvalidRelay(format!(
                "relay {} name is required",
                self.id
            )));
        }
        self.endpoint
            .validate()
            .map_err(|error| ClientConfigError::InvalidRelay(format!("{}: {error}", self.id)))?;
        if !self.supports_udp && !self.supports_tcp {
            return Err(ClientConfigError::InvalidRelay(format!(
                "{} must support UDP or TCP",
                self.id
            )));
        }
        if let Some(transport) = self.default_transport
            && !self.supports(transport)
        {
            return Err(ClientConfigError::InvalidRelay(format!(
                "{} default transport is not supported",
                self.id
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl LocalDate {
    #[must_use]
    pub fn today() -> Self {
        let today = Local::now().date_naive();
        Self {
            year: today.year(),
            month: today.month(),
            day: today.day(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientConfigError {
    #[error("invalid TOML: {0}")]
    InvalidToml(#[from] toml::de::Error),
    #[error("invalid default transport: {0}")]
    InvalidTransport(String),
    #[error("invalid default mode: {0}")]
    InvalidMode(String),
    #[error("invalid relay preset: {0}")]
    InvalidRelay(String),
    #[error("duplicate relay id: {0}")]
    DuplicateRelayId(String),
    #[error("selected relay does not exist: {0}")]
    UnknownSelectedRelay(String),
    #[error("unsupported room template token: {0}")]
    UnsupportedRoomTemplate(String),
    #[error("invalid session health config: {0}")]
    InvalidSessionHealth(String),
}

#[derive(Debug, Deserialize)]
struct RawClientConfig {
    default_room: Option<String>,
    default_room_template: Option<String>,
    default_transport: Option<String>,
    default_mode: Option<String>,
    selected_relay: Option<String>,
    session_health: Option<RawSessionHealthConfig>,
    #[serde(default)]
    relays: Vec<RawRelayPreset>,
}

#[derive(Debug, Default, Deserialize)]
struct RawSessionHealthConfig {
    enabled: Option<bool>,
    runtime_rtt_enabled: Option<bool>,
    snapshot_interval_seconds: Option<u64>,
    runtime_rtt_interval_seconds: Option<u64>,
    runtime_rtt_timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawRelayPreset {
    id: String,
    name: String,
    host: String,
    port: u16,
    #[serde(default = "default_true")]
    udp: bool,
    #[serde(default = "default_true")]
    tcp: bool,
    default_transport: Option<String>,
}

impl TryFrom<RawClientConfig> for ClientConfig {
    type Error = ClientConfigError;

    fn try_from(value: RawClientConfig) -> Result<Self, Self::Error> {
        let config = Self {
            default_room: trimmed_non_empty(value.default_room),
            default_room_template: trimmed_non_empty(value.default_room_template),
            default_transport: parse_transport(value.default_transport.as_deref())?
                .unwrap_or_default(),
            default_mode: parse_mode(value.default_mode.as_deref())?.unwrap_or(SessionMode::Pure),
            selected_relay: trimmed_non_empty(value.selected_relay),
            relays: value
                .relays
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
            session_health: value.session_health.unwrap_or_default().into(),
        };
        config.validate()?;
        Ok(config)
    }
}

impl From<RawSessionHealthConfig> for SessionHealthConfig {
    fn from(value: RawSessionHealthConfig) -> Self {
        let defaults = Self::default();
        Self {
            enabled: value.enabled.unwrap_or(defaults.enabled),
            runtime_rtt_enabled: value
                .runtime_rtt_enabled
                .unwrap_or(defaults.runtime_rtt_enabled),
            snapshot_interval_seconds: value
                .snapshot_interval_seconds
                .unwrap_or(defaults.snapshot_interval_seconds),
            runtime_rtt_interval_seconds: value
                .runtime_rtt_interval_seconds
                .unwrap_or(defaults.runtime_rtt_interval_seconds),
            runtime_rtt_timeout_seconds: value
                .runtime_rtt_timeout_seconds
                .unwrap_or(defaults.runtime_rtt_timeout_seconds),
        }
    }
}

impl TryFrom<RawRelayPreset> for RelayPreset {
    type Error = ClientConfigError;

    fn try_from(value: RawRelayPreset) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.trim().to_owned(),
            name: value.name.trim().to_owned(),
            endpoint: RelayEndpoint::new(value.host.trim(), value.port),
            supports_udp: value.udp,
            supports_tcp: value.tcp,
            default_transport: parse_transport(value.default_transport.as_deref())?,
        })
    }
}

pub fn load_client_config() -> LoadedClientConfig {
    let bundle_path = bundle_config_path();
    let app_path = app_data_config_path();
    let mut warnings = Vec::new();

    for path in [bundle_path, app_path] {
        let Some(path) = path else {
            continue;
        };
        if !path.exists() {
            continue;
        }
        return match load_config_file(&path) {
            Ok(config) => {
                let resolved_default_room = match config.resolved_room_for_startup() {
                    Ok(room) => room,
                    Err(error) => {
                        warnings.push(format!(
                            "Invalid Room template in {}: {error}",
                            path.display()
                        ));
                        None
                    }
                };
                LoadedClientConfig {
                    config,
                    source: Some(path),
                    resolved_default_room,
                    warnings,
                }
            }
            Err(error) => {
                warnings.push(format!("Invalid config at {}: {error}", path.display()));
                LoadedClientConfig {
                    config: ClientConfig::default(),
                    source: Some(path),
                    resolved_default_room: None,
                    warnings,
                }
            }
        };
    }

    LoadedClientConfig {
        config: ClientConfig::default(),
        source: None,
        resolved_default_room: None,
        warnings,
    }
}

pub fn bundle_config_path() -> Option<PathBuf> {
    bundle_directory().map(|directory| directory.join(CLIENT_CONFIG_FILE))
}

pub fn app_data_config_path() -> Option<PathBuf> {
    ProjectDirs::from("io.github", "mcthesw", PRODUCT_NAME)
        .map(|project| project.data_local_dir().join(CLIENT_CONFIG_FILE))
}

pub fn bundle_directory() -> Option<PathBuf> {
    if let Some(path) = env::var_os("BASEMENT_BRIDGE_BUNDLE_DIR") {
        return Some(PathBuf::from(path));
    }
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

fn load_config_file(path: &Path) -> Result<ClientConfig, ClientConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|error| ClientConfigError::InvalidRelay(error.to_string()))?;
    toml::from_str::<RawClientConfig>(&contents)?.try_into()
}

pub fn resolve_room_template(template: &str, date: LocalDate) -> Result<String, ClientConfigError> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{date:") {
        output.push_str(&rest[..start]);
        let token_start = start + "{date:".len();
        let Some(end) = rest[token_start..].find('}') else {
            return Err(ClientConfigError::UnsupportedRoomTemplate(
                rest[start..].to_owned(),
            ));
        };
        let format = &rest[token_start..token_start + end];
        output.push_str(&format_date(format, date)?);
        rest = &rest[token_start + end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

fn format_date(format: &str, date: LocalDate) -> Result<String, ClientConfigError> {
    let mut output = String::new();
    let mut chars = format.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }
        let Some(token) = chars.next() else {
            return Err(ClientConfigError::UnsupportedRoomTemplate(
                format.to_owned(),
            ));
        };
        match token {
            'Y' => output.push_str(&format!("{:04}", date.year)),
            'm' => output.push_str(&format!("{:02}", date.month)),
            'd' => output.push_str(&format!("{:02}", date.day)),
            '%' => output.push('%'),
            other => {
                return Err(ClientConfigError::UnsupportedRoomTemplate(format!(
                    "%{other}"
                )));
            }
        }
    }
    Ok(output)
}

fn parse_transport(value: Option<&str>) -> Result<Option<TransportChoice>, ClientConfigError> {
    value
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "udp" => Ok(TransportChoice::Udp),
            "tcp" => Ok(TransportChoice::Tcp),
            other => Err(ClientConfigError::InvalidTransport(other.to_owned())),
        })
        .transpose()
}

fn parse_mode(value: Option<&str>) -> Result<Option<SessionMode>, ClientConfigError> {
    value
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "official" => Ok(SessionMode::Official),
            "fallback" => Ok(SessionMode::Fallback),
            "pure" => Ok(SessionMode::Pure),
            other => Err(ClientConfigError::InvalidMode(other.to_owned())),
        })
        .transpose()
}

fn trimmed_non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_supported_date_room_template() {
        let date = LocalDate {
            year: 2026,
            month: 6,
            day: 10,
        };

        let room = resolve_room_template("bb-{date:%Y%m%d}", date).unwrap();

        assert_eq!(room, "bb-20260610");
    }

    #[test]
    fn parses_relay_presets_and_defaults() {
        let raw = r#"
default_room_template = "bb-{date:%Y%m%d}"
default_transport = "tcp"
default_mode = "pure"
selected_relay = "current"

[session_health]
enabled = true
runtime_rtt_enabled = false
snapshot_interval_seconds = 10

[[relays]]
id = "current"
name = "Current test relay"
host = "relay.example.test"
port = 25910
udp = true
tcp = true
default_transport = "tcp"
"#;

        let config: ClientConfig = toml::from_str::<RawClientConfig>(raw)
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(config.default_transport, TransportChoice::Tcp);
        assert_eq!(config.default_mode, SessionMode::Pure);
        assert!(config.session_health.enabled);
        assert!(!config.session_health.runtime_rtt_enabled);
        assert_eq!(config.session_health.snapshot_interval_seconds, 10);
        assert_eq!(config.selected_relay_index(), Some(0));
        assert_eq!(
            config.relays[0].preferred_transport(TransportChoice::Udp),
            TransportChoice::Tcp
        );
    }

    #[test]
    fn rejects_invalid_session_health_interval() {
        let raw = r#"
[session_health]
enabled = true
snapshot_interval_seconds = 0
"#;

        let error =
            ClientConfig::try_from(toml::from_str::<RawClientConfig>(raw).unwrap()).unwrap_err();

        assert!(matches!(error, ClientConfigError::InvalidSessionHealth(_)));
    }
}
