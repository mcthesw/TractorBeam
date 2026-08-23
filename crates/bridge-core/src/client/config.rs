use std::{
    collections::HashSet,
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use atomic_write_file::AtomicWriteFile;
use serde::Deserialize;

use super::session_config::{RelayEndpoint, SessionHealthConfig, SessionMode, TransportChoice};

pub const CLIENT_CONFIG_FILE: &str = "config.toml";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadedClientConfig {
    pub config: ClientConfig,
    pub source: Option<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientConfig {
    pub default_transport: TransportChoice,
    pub default_mode: SessionMode,
    pub selected_relay: Option<String>,
    pub selected_steam_id64: Option<String>,
    pub relays: Vec<RelayPreset>,
    pub session_health: SessionHealthConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            default_transport: TransportChoice::default(),
            default_mode: SessionMode::Pure,
            selected_relay: None,
            selected_steam_id64: None,
            relays: Vec::new(),
            session_health: SessionHealthConfig::default(),
        }
    }
}

impl ClientConfig {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayProfileInput {
    pub name: String,
    pub endpoint: RelayEndpoint,
    pub supports_udp: bool,
    pub supports_tcp: bool,
    pub default_transport: TransportChoice,
}

impl RelayProfileInput {
    fn into_preset(self, id: String) -> Result<RelayPreset, ClientConfigError> {
        let relay = RelayPreset {
            id,
            name: self.name.trim().to_owned(),
            endpoint: RelayEndpoint::new(self.endpoint.host.trim(), self.endpoint.port),
            supports_udp: self.supports_udp,
            supports_tcp: self.supports_tcp,
            default_transport: Some(self.default_transport),
        };
        relay.validate()?;
        Ok(relay)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RelayCatalogChange {
    Add(RelayProfileInput),
    Update {
        id: String,
        relay: RelayProfileInput,
    },
    Delete {
        id: String,
    },
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
    #[error("relay does not exist: {0}")]
    RelayNotFound(String),
    #[error("relay record number is exhausted")]
    RelayIdExhausted,
    #[error("invalid session health config: {0}")]
    InvalidSessionHealth(String),
    #[error("Bundle config path is unavailable")]
    ConfigPathUnavailable,
    #[error("invalid editable config document: {0}")]
    InvalidDocument(String),
    #[error("could not {operation} config at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Deserialize)]
struct RawClientConfig {
    default_transport: Option<String>,
    default_mode: Option<String>,
    selected_relay: Option<String>,
    selected_steam_id64: Option<String>,
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
            default_transport: parse_transport(value.default_transport.as_deref())?
                .unwrap_or_default(),
            default_mode: parse_mode(value.default_mode.as_deref())?.unwrap_or(SessionMode::Pure),
            selected_relay: trimmed_non_empty(value.selected_relay),
            selected_steam_id64: trimmed_non_empty(value.selected_steam_id64),
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
    let Some(path) = bundle_config_path() else {
        return LoadedClientConfig {
            config: ClientConfig::default(),
            source: None,
            warnings: vec!["Bundle config path is unavailable".to_owned()],
        };
    };
    let mut warnings = Vec::new();
    if !path.exists() {
        return LoadedClientConfig {
            config: ClientConfig::default(),
            source: None,
            warnings,
        };
    }
    match load_config_file(&path) {
        Ok(config) => LoadedClientConfig {
            config,
            source: Some(path),
            warnings,
        },
        Err(error) => {
            warnings.push(format!("Invalid config at {}: {error}", path.display()));
            LoadedClientConfig {
                config: ClientConfig::default(),
                source: Some(path),
                warnings,
            }
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClientConfigSelection {
    pub selected_relay: Option<String>,
    pub selected_steam_id64: Option<String>,
}

pub fn save_client_config_selection(
    selection: &ClientConfigSelection,
) -> Result<PathBuf, ClientConfigError> {
    let path = bundle_config_path().ok_or(ClientConfigError::ConfigPathUnavailable)?;
    save_selection_to(&path, selection)?;
    Ok(path)
}

pub fn save_client_relay_catalog_to(
    path: &Path,
    change: &RelayCatalogChange,
) -> Result<LoadedClientConfig, ClientConfigError> {
    let existing = read_editable_config(path)?;
    let mut doc = parse_editable_config(&existing)?;
    let current = parse_client_config(&doc)?;

    match change {
        RelayCatalogChange::Add(input) => {
            let id = next_relay_id(&doc, &current)?;
            let relay = input.clone().into_preset(id.clone())?;
            relay_tables_mut(&mut doc).push(relay_table(&relay));
            set_optional_key(&mut doc, "selected_relay", Some(&id));
            let next = id
                .parse::<u64>()
                .expect("generated relay ids are numeric")
                .checked_add(1)
                .ok_or(ClientConfigError::RelayIdExhausted)?;
            doc["next_relay_id"] = toml_edit::value(
                i64::try_from(next).map_err(|_| ClientConfigError::RelayIdExhausted)?,
            );
        }
        RelayCatalogChange::Update { id, relay } => {
            let relay = relay.clone().into_preset(id.clone())?;
            let table = find_relay_table_mut(&mut doc, id)?;
            write_relay_fields(table, &relay);
        }
        RelayCatalogChange::Delete { id } => {
            let next_id = next_relay_id(&doc, &current)?;
            let tables = relay_tables_mut(&mut doc);
            let Some(index) = tables
                .iter()
                .position(|table| relay_table_id_matches(table, id))
            else {
                return Err(ClientConfigError::RelayNotFound(id.clone()));
            };
            tables.remove(index);
            if current.selected_relay.as_deref() == Some(id) {
                set_optional_key(&mut doc, "selected_relay", None);
            }
            doc["next_relay_id"] = toml_edit::value(
                next_id
                    .parse::<i64>()
                    .map_err(|_| ClientConfigError::RelayIdExhausted)?,
            );
        }
    }

    let config = parse_client_config(&doc)?;
    write_document(path, &doc)?;
    Ok(LoadedClientConfig {
        config,
        source: Some(path.to_path_buf()),
        warnings: Vec::new(),
    })
}

fn save_selection_to(
    path: &Path,
    selection: &ClientConfigSelection,
) -> Result<(), ClientConfigError> {
    let existing = read_editable_config(path)?;
    let mut doc = parse_editable_config(&existing)?;
    set_optional_key(
        &mut doc,
        "selected_relay",
        selection.selected_relay.as_deref(),
    );
    set_optional_key(
        &mut doc,
        "selected_steam_id64",
        selection.selected_steam_id64.as_deref(),
    );
    write_document(path, &doc)
}

fn read_editable_config(path: &Path) -> Result<String, ClientConfigError> {
    match fs::read_to_string(path) {
        Ok(existing) => Ok(existing),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(ClientConfigError::Io {
            operation: "read",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn parse_editable_config(contents: &str) -> Result<toml_edit::DocumentMut, ClientConfigError> {
    contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| ClientConfigError::InvalidDocument(error.to_string()))
}

fn parse_client_config(doc: &toml_edit::DocumentMut) -> Result<ClientConfig, ClientConfigError> {
    toml::from_str::<RawClientConfig>(&doc.to_string())?.try_into()
}

fn write_document(path: &Path, doc: &toml_edit::DocumentMut) -> Result<(), ClientConfigError> {
    let mut file = AtomicWriteFile::open(path).map_err(|source| ClientConfigError::Io {
        operation: "open for atomic write",
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(doc.to_string().as_bytes())
        .map_err(|source| ClientConfigError::Io {
            operation: "write",
            path: path.to_path_buf(),
            source,
        })?;
    file.commit().map_err(|source| ClientConfigError::Io {
        operation: "replace",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn next_relay_id(
    doc: &toml_edit::DocumentMut,
    config: &ClientConfig,
) -> Result<String, ClientConfigError> {
    let after_existing = config
        .relays
        .iter()
        .filter_map(|relay| relay.id.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(ClientConfigError::RelayIdExhausted)?;
    let stored = doc
        .get("next_relay_id")
        .and_then(toml_edit::Item::as_integer)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(1);
    let id = after_existing.max(stored);
    i64::try_from(id).map_err(|_| ClientConfigError::RelayIdExhausted)?;
    Ok(id.to_string())
}

fn relay_tables_mut(doc: &mut toml_edit::DocumentMut) -> &mut toml_edit::ArrayOfTables {
    if !doc.contains_key("relays") {
        doc["relays"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    doc["relays"]
        .as_array_of_tables_mut()
        .expect("validated config relays must be an array of tables")
}

fn find_relay_table_mut<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    id: &str,
) -> Result<&'a mut toml_edit::Table, ClientConfigError> {
    relay_tables_mut(doc)
        .iter_mut()
        .find(|table| relay_table_id_matches(table, id))
        .ok_or_else(|| ClientConfigError::RelayNotFound(id.to_owned()))
}

fn relay_table_id_matches(table: &toml_edit::Table, id: &str) -> bool {
    table
        .get("id")
        .and_then(toml_edit::Item::as_str)
        .is_some_and(|stored| stored.trim() == id.trim())
}

fn relay_table(relay: &RelayPreset) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    table["id"] = toml_edit::value(relay.id.clone());
    write_relay_fields(&mut table, relay);
    table
}

fn write_relay_fields(table: &mut toml_edit::Table, relay: &RelayPreset) {
    table["name"] = toml_edit::value(relay.name.clone());
    table["host"] = toml_edit::value(relay.endpoint.host.clone());
    table["port"] = toml_edit::value(i64::from(relay.endpoint.port));
    table["udp"] = toml_edit::value(relay.supports_udp);
    table["tcp"] = toml_edit::value(relay.supports_tcp);
    match relay.default_transport {
        Some(transport) => {
            table["default_transport"] = toml_edit::value(transport_name(transport));
        }
        None => {
            table.remove("default_transport");
        }
    }
}

fn transport_name(transport: TransportChoice) -> &'static str {
    match transport {
        TransportChoice::Udp => "udp",
        TransportChoice::Tcp => "tcp",
    }
}

#[cfg(test)]
fn save_client_config_selection_to(
    path: &Path,
    selection: &ClientConfigSelection,
) -> Result<(), ClientConfigError> {
    save_selection_to(path, selection)
}

fn set_optional_key(doc: &mut toml_edit::DocumentMut, key: &str, value: Option<&str>) {
    let trimmed = value.map(|v| v.trim()).filter(|v| !v.is_empty());
    match trimmed {
        Some(value) => {
            doc[key] = toml_edit::value(value.to_owned());
        }
        None => {
            doc.remove(key);
        }
    }
}

pub fn bundle_config_path() -> Option<PathBuf> {
    bundle_directory().map(|directory| directory.join(CLIENT_CONFIG_FILE))
}

pub fn bundle_directory() -> Option<PathBuf> {
    if let Some(path) = env::var_os("TRACTOR_BEAM_BUNDLE_DIR") {
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
#[path = "config_tests.rs"]
mod tests;
