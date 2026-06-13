use std::io;

use super::{
    ConfigError, LoadedClientConfig, RelayEndpoint, SessionConfig, SessionMode, hook_config,
    logging::{ClientLogSink, ClientSessionLog, ClientSessionLogContext, TracingClientLogSink},
    probe, session,
    state::{self, log_entry, trim_logs},
};
use crate::client::{LogLevel, RuntimeState};

#[derive(Debug)]
pub struct BridgeClient {
    pub(super) state: RuntimeState,
    session: Option<session::SessionHandle>,
    pub(super) loaded_config: LoadedClientConfig,
    pub(super) log_sink: Box<dyn ClientLogSink>,
    active_session_log: Option<Box<dyn ClientSessionLog>>,
    readiness_probe: Option<probe::ProbeHandle>,
    hook_receive_probe: Option<probe::ProbeHandle>,
}

impl BridgeClient {
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(LoadedClientConfig::default())
    }

    #[must_use]
    pub fn with_config(loaded_config: LoadedClientConfig) -> Self {
        Self::with_config_and_log_sink(loaded_config, Box::new(TracingClientLogSink))
    }

    #[must_use]
    pub fn with_config_and_log_sink(
        loaded_config: LoadedClientConfig,
        log_sink: Box<dyn ClientLogSink>,
    ) -> Self {
        let mut client = Self {
            state: RuntimeState::default(),
            session: None,
            loaded_config,
            log_sink,
            active_session_log: None,
            readiness_probe: None,
            hook_receive_probe: None,
        };
        client.refresh_steam_accounts();
        client.log(LogLevel::Info, "Bridge Client ready");
        if let Some(path) = &client.loaded_config.source {
            client.log(
                LogLevel::Info,
                format!("Loaded client config from {}", path.display()),
            );
        }
        for warning in client.loaded_config.warnings.clone() {
            client.log(LogLevel::Warn, warning);
        }
        if let Some(root) = client.log_sink.root() {
            client.log(
                LogLevel::Info,
                format!("Bridge Client logs: {}", root.display()),
            );
        }
        for warning in client.log_sink.warnings() {
            client.log(LogLevel::Warn, warning);
        }
        client
    }

    #[must_use]
    pub fn state(&self) -> &RuntimeState {
        &self.state
    }

    #[must_use]
    pub fn loaded_config(&self) -> &LoadedClientConfig {
        &self.loaded_config
    }

    pub fn poll_events(&mut self) -> bool {
        let mut processed = false;
        let mut should_clear = false;
        let mut readiness_finished = false;
        let mut hook_probe_finished = false;
        let mut events = Vec::new();
        if let Some(handle) = &self.session {
            while let Ok(event) = handle.events.try_recv() {
                events.push(event);
            }
        }
        if let Some(handle) = &self.readiness_probe {
            while let Ok(event) = handle.events.try_recv() {
                events.push(event);
            }
        }
        if let Some(handle) = &self.hook_receive_probe {
            while let Ok(event) = handle.events.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            processed = true;
            match event {
                state::RuntimeEvent::Log(level, message) => self.push_log(level, message),
                state::RuntimeEvent::CounterDelta(delta) => self.state.counters.add(delta),
                state::RuntimeEvent::ReadinessProbeFinished(result) => {
                    self.state.readiness_probe_running = false;
                    readiness_finished = true;
                    match result {
                        Ok(report) => {
                            self.state.latest_readiness_probe = Some(*report);
                        }
                        Err(message) => self.log(LogLevel::Error, message),
                    }
                }
                state::RuntimeEvent::HookReceiveProbeFinished(result) => {
                    self.state.hook_probe_running = false;
                    hook_probe_finished = true;
                    match result {
                        Ok(report) => {
                            self.state.latest_hook_receive_probe = Some(report);
                            self.state.latest_hook_receive_probe_error = None;
                        }
                        Err(message) => {
                            self.state.latest_hook_receive_probe = None;
                            self.state.latest_hook_receive_probe_error = Some(message.clone());
                            self.log(LogLevel::Error, message);
                        }
                    }
                }
                state::RuntimeEvent::SessionHealthSnapshot(snapshot) => {
                    self.state.latest_session_health = Some(*snapshot);
                }
                state::RuntimeEvent::SessionHealthSummary(snapshot) => {
                    let snapshot = *snapshot;
                    self.state.latest_session_health = Some(snapshot.clone());
                    self.state.latest_session_health_summary = Some(snapshot);
                }
                state::RuntimeEvent::Stopped => {
                    self.state.status = state::SessionStatus::Idle;
                    self.active_session_log = None;
                    should_clear = true;
                }
            }
        }
        if should_clear {
            self.session = None;
        }
        if readiness_finished && let Some(handle) = self.readiness_probe.take() {
            handle.finish();
        }
        if hook_probe_finished && let Some(handle) = self.hook_receive_probe.take() {
            handle.finish();
        }
        processed
    }

    pub fn refresh_steam_accounts(&mut self) {
        self.state.detected_accounts = crate::steam::detect_accounts()
            .into_iter()
            .map(Into::into)
            .collect();
        let count = self.state.detected_accounts.len();
        self.log(LogLevel::Info, format!("Detected {count} Steam account(s)"));
    }

    pub fn start_session(&mut self, config: &SessionConfig) -> Result<(), ClientError> {
        config.validate()?;
        self.stop_session();
        self.active_session_log = self
            .log_sink
            .start_session(ClientSessionLogContext {
                relay_name: config.relay_name.clone(),
                relay: config.relay.clone(),
                transport: config.transport,
                room: config.room.clone(),
                mode: config.mode,
            })
            .ok();

        let session = if config.mode != SessionMode::Official {
            hook_config::write_hook_config(config)?;
            Some(session::spawn_bridge_worker(config.clone())?)
        } else {
            None
        };

        if let Err(error) = crate::steam::launch_isaac() {
            if let Some(handle) = session {
                handle.stop();
            }
            self.active_session_log = None;
            return Err(error.into());
        }

        self.session = session;
        self.state.status = state::SessionStatus::Running;
        self.log(
            LogLevel::Info,
            format!(
                "Starting {mode} session in room {room}",
                mode = config.mode,
                room = config.room
            ),
        );
        if config.mode != SessionMode::Official {
            if let Some(name) = &config.relay_name {
                self.log(LogLevel::Info, format!("Relay preset: {name}"));
            }
            self.log(LogLevel::Info, format!("Relay endpoint: {}", config.relay));
            self.log(LogLevel::Info, format!("Transport: {}", config.transport));
        }
        self.log(
            LogLevel::Info,
            format!("Steam launch URI: {}", crate::steam::isaac_launch_uri()),
        );
        Ok(())
    }

    pub fn stop_session(&mut self) {
        if let Some(handle) = self.session.take() {
            handle.stop();
        }
        self.state.status = state::SessionStatus::Idle;
        self.active_session_log = None;
        self.log(LogLevel::Info, "Session stopped");
    }

    pub fn start_readiness_probe(&mut self, relay: RelayEndpoint) -> Result<(), ClientError> {
        if self.state.readiness_probe_running {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "readiness probe is already running",
            )
            .into());
        }
        let handle = probe::spawn_readiness_probe(relay.clone())?;
        self.readiness_probe = Some(handle);
        self.state.readiness_probe_running = true;
        self.log(
            LogLevel::Info,
            format!(
                "Readiness probe started: relay={relay} samples_per_case={} payload_bytes={:?} transports=[TCP, UDP]",
                probe::READINESS_PROBE_SAMPLES_PER_CASE,
                probe::READINESS_PROBE_PAYLOAD_BYTES
            ),
        );
        Ok(())
    }

    pub fn start_hook_receive_probe(&mut self) -> Result<(), ClientError> {
        if self.state.hook_probe_running {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "hook receive probe is already running",
            )
            .into());
        }
        self.hook_receive_probe = Some(probe::spawn_hook_receive_probe());
        self.state.hook_probe_running = true;
        self.state.latest_hook_receive_probe_error = None;
        self.log(LogLevel::Info, "Hook receive probe started");
        Ok(())
    }

    pub(super) fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        self.push_log(level, message);
    }

    fn push_log(&mut self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        let active_session = self.active_session_log.as_deref();
        let session_context = active_session.map(ClientSessionLog::context);
        self.log_sink.emit(session_context, level, &message);
        if let Some(session) = active_session {
            session.emit(level, &message);
        }
        let entry = log_entry(level, message);
        self.state.logs.push(entry);
        trim_logs(&mut self.state.logs);
    }
}

impl Default for BridgeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        if let Some(handle) = self.session.take() {
            handle.stop();
        }
        if let Some(handle) = self.readiness_probe.take() {
            handle.finish();
        }
        if let Some(handle) = self.hook_receive_probe.take() {
            handle.finish();
        }
    }
}

#[must_use]
pub fn runtime_name() -> &'static str {
    "bridge-core"
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("{0}")]
    Config(#[from] ConfigError),
    #[error("{0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use crate::client::{
        SessionHealthConfig, SessionHealthSnapshot, SessionQuality, TransportChoice,
    };

    use super::*;

    #[test]
    fn exposes_runtime_name() {
        assert_eq!(runtime_name(), "bridge-core");
    }

    #[test]
    fn validates_relay_endpoint() {
        assert!(
            RelayEndpoint::new("relay.example.com", 25_910)
                .validate()
                .is_ok()
        );
        assert_eq!(
            RelayEndpoint::new("", 25_910).validate(),
            Err(ConfigError::MissingRelayHost)
        );
    }

    #[test]
    fn validates_session_config() {
        let config = SessionConfig {
            relay: RelayEndpoint::new("relay.example.com", 25_910),
            relay_name: None,
            transport: TransportChoice::Udp,
            room: "room".to_owned(),
            mode: SessionMode::Pure,
            steam_id64: "76561198000000001".to_owned(),
            display_name: "Alice".to_owned(),
            session_health: SessionHealthConfig::default(),
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn redacts_exported_diagnostics_text() {
        let mut client = BridgeClient::new();
        client.log(LogLevel::Info, "Relay endpoint: 203.0.113.10:25910");
        client.log(LogLevel::Info, "Starting Pure session in room 123");
        client.log(LogLevel::Info, "SteamID64 76561198000000001");

        let text = client.redacted_diagnostics_text();

        assert!(!text.contains("203.0.113.10"));
        assert!(!text.contains("76561198000000001"));
        assert!(!text.contains("room 123"));
    }

    #[test]
    fn diagnostics_include_session_health_evidence() {
        let mut client = BridgeClient::new();
        client.state.latest_session_health = Some(SessionHealthSnapshot {
            quality: SessionQuality::Good,
            ..SessionHealthSnapshot::default()
        });

        let text = client.diagnostics_text();

        assert!(text.contains("session health:"));
        assert!(text.contains("quality=good"));
        assert!(text.contains("\"quality\": \"good\""));
    }
}
