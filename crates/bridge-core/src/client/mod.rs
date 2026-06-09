//! Local Bridge Client runtime without GUI ownership.

mod config;
mod diagnostics;
mod hook_config;
mod probe;
mod session;
mod state;

use std::io;

pub use config::{ConfigError, RelayEndpoint, SessionConfig, SessionMode, SteamIdentity};
pub use diagnostics::diagnostics_directory;
pub use probe::{DEFAULT_RELAY_PROBE_PAYLOAD_BYTES, HookReceiveProbeReport, RelayProbeReport};
pub use state::{Counters, LogEntry, LogLevel, RuntimeState, SessionStatus};

use state::log_entry;

pub const PRODUCT_NAME: &str = "Basement Bridge";

#[derive(Debug, Default)]
pub struct BridgeClient {
    state: RuntimeState,
    session: Option<session::SessionHandle>,
}

impl BridgeClient {
    #[must_use]
    pub fn new() -> Self {
        let mut client = Self::default();
        client.refresh_steam_accounts();
        client.log(LogLevel::Info, "Bridge Client ready");
        client
    }

    #[must_use]
    pub fn state(&self) -> &RuntimeState {
        &self.state
    }

    pub fn poll_events(&mut self) {
        let mut should_clear = false;
        if let Some(handle) = &self.session {
            while let Ok(event) = handle.events.try_recv() {
                match event {
                    state::RuntimeEvent::Log(level, message) => {
                        self.state.logs.push(log_entry(level, message));
                    }
                    state::RuntimeEvent::CounterDelta(delta) => self.state.counters.add(delta),
                    state::RuntimeEvent::Stopped => {
                        self.state.status = SessionStatus::Idle;
                        should_clear = true;
                    }
                }
            }
        }
        if should_clear {
            self.session = None;
        }
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

        let mut session = if config.mode != SessionMode::Official {
            hook_config::write_hook_config(config)?;
            Some(session::spawn_bridge_worker(config.clone()))
        } else {
            None
        };

        if let Err(error) = crate::steam::launch_isaac() {
            if let Some(handle) = session.take() {
                handle.stop();
            }
            return Err(error.into());
        }

        if let Some(handle) = &mut session {
            handle.spawn_injector_worker();
        }
        self.session = session;
        self.state.status = SessionStatus::Running;
        self.log(
            LogLevel::Info,
            format!(
                "Starting {mode} session in room {room}",
                mode = config.mode,
                room = config.room
            ),
        );
        if config.mode != SessionMode::Official {
            self.log(LogLevel::Info, format!("Relay endpoint: {}", config.relay));
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
        self.state.status = SessionStatus::Idle;
        self.log(LogLevel::Info, "Session stopped");
    }

    fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        self.state.logs.push(log_entry(level, message));
    }
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        if let Some(handle) = self.session.take() {
            handle.stop();
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
            room: "room".to_owned(),
            mode: SessionMode::Pure,
            steam_id64: "76561198000000001".to_owned(),
            display_name: "Alice".to_owned(),
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn redacts_exported_diagnostics_text() {
        let mut client = BridgeClient::new();
        client.state.logs.push(log_entry(
            LogLevel::Info,
            "Relay endpoint: 203.0.113.10:25910",
        ));
        client.state.logs.push(log_entry(
            LogLevel::Info,
            "Starting Pure session in room 123",
        ));
        client
            .state
            .logs
            .push(log_entry(LogLevel::Info, "SteamID64 76561198000000001"));

        let text = client.redacted_diagnostics_text();

        assert!(!text.contains("203.0.113.10"));
        assert!(!text.contains("76561198000000001"));
        assert!(!text.contains("room 123"));
    }
}
