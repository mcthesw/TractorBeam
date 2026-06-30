use std::{fs, io};

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
        client.log(
            LogLevel::Info,
            format!(
                "Bridge Client ready ({})",
                crate::build_info::version_label()
            ),
        );
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
                            self.state.latest_hook_receive_probe_warning = None;
                        }
                        Err(message) => {
                            self.state.latest_hook_receive_probe = None;
                            self.state.latest_hook_receive_probe_error = Some(message.clone());
                            self.state.latest_hook_receive_probe_warning = None;
                            self.log(LogLevel::Error, message);
                        }
                    }
                }
                state::RuntimeEvent::HookReceiveProbeWarning(message) => {
                    self.state.latest_hook_receive_probe_warning = Some(message);
                    self.state.latest_hook_receive_probe_error = None;
                }
                state::RuntimeEvent::HookStartup(startup) => {
                    let mut startup = *startup;
                    if startup.launch_parameters_path.is_none() {
                        startup.launch_parameters_path =
                            self.state.hook_launch_parameters_path_written.clone();
                    }
                    self.state.hook_startup = startup;
                }
                state::RuntimeEvent::SessionHealthSnapshot(snapshot) => {
                    if let Some(incident) = self.state.record_session_health_incident(&snapshot) {
                        self.log(
                            LogLevel::Warn,
                            format!("Client incident {}: {}", incident.kind, incident.summary),
                        );
                    }
                    self.state.latest_session_health = Some(*snapshot);
                }
                state::RuntimeEvent::SessionHealthSummary(snapshot) => {
                    let snapshot = *snapshot;
                    self.state.latest_session_health = Some(snapshot.clone());
                    self.state.latest_session_health_summary = Some(snapshot);
                }
                state::RuntimeEvent::SessionEnded(reason) => {
                    self.state.last_stop_reason = Some(reason.clone());
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
            self.cleanup_hook_launch_parameters("session ended");
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
        self.state.last_stop_reason = None;
        self.state.latest_hook_receive_probe = None;
        self.state.latest_hook_receive_probe_error = None;
        self.state.latest_hook_receive_probe_warning = None;
        self.state.latest_session_health = None;
        self.state.latest_session_health_summary = None;
        self.state.hook_launch_parameters_path_written = None;
        self.state.hook_launch_parameters_cleanup = None;
        self.state.hook_startup = state::HookStartupState::default();
        self.state.client_incidents.clear();
        self.active_session_log = self
            .log_sink
            .start_session(ClientSessionLogContext {
                relay_name: config.relay_name.clone(),
                relay: config.relay.clone(),
                transport: config.transport,
                room: config.room.clone(),
                mode: config.mode,
                #[cfg(feature = "internal-test")]
                test_run_id: config.test_run_id.clone(),
            })
            .ok();

        let session = if config.mode != SessionMode::Official {
            let native_hook_paths = match basement_isaac_injector::resolve_native_hook_paths() {
                Ok(paths) => paths,
                Err(error) => {
                    let message = format!("Native Hook artifact resolution failed: {error}");
                    self.record_hook_startup_failure(None, message.clone());
                    self.active_session_log = None;
                    return Err(io::Error::other(message).into());
                }
            };
            let write = match hook_config::write_hook_config(config, &native_hook_paths) {
                Ok(write) => write,
                Err(error) => {
                    let message = format!("Native Hook launch parameter write failed: {error}");
                    self.record_hook_startup_failure(Some(&native_hook_paths), message.clone());
                    self.active_session_log = None;
                    return Err(io::Error::new(error.kind(), message).into());
                }
            };
            self.state.hook_launch_parameters_path_written = Some(write.path.clone());
            self.state.hook_startup = state::HookStartupState {
                phase: state::HookStartupPhase::Configured,
                injector_path: Some(native_hook_paths.injector.clone()),
                hook_path: Some(native_hook_paths.hook.clone()),
                launch_parameters_path: Some(write.path.clone()),
                endpoint: Some(format!("{}/UDP", hook_config::HOOK_OUT)),
                message: Some(format!(
                    "Hook launch parameters written to {}",
                    write.path.display()
                )),
                updated_at: state::unix_seconds(),
                ..state::HookStartupState::default()
            };
            self.log(
                LogLevel::Info,
                format!(
                    "Native Hook launch parameters written to {}",
                    write.path.display()
                ),
            );
            match session::spawn_bridge_worker(config.clone(), native_hook_paths.clone()) {
                Ok(handle) => Some(handle),
                Err(error) => {
                    let message = format!("Bridge worker startup failed: {error}");
                    self.record_hook_startup_failure(Some(&native_hook_paths), message);
                    self.cleanup_hook_launch_parameters("bridge worker startup failed");
                    self.active_session_log = None;
                    return Err(error.into());
                }
            }
        } else {
            self.state.hook_launch_parameters_path_written = None;
            None
        };

        if let Err(error) = crate::steam::launch_isaac() {
            if let Some(handle) = session {
                self.apply_stopped_session_events(handle.stop());
            }
            self.cleanup_hook_launch_parameters("Steam launch failed");
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
        #[cfg(feature = "internal-test")]
        if let Some(test_run_id) = &config.test_run_id {
            self.log(
                LogLevel::Info,
                format!("Internal test run id: {test_run_id}"),
            );
        }
        self.log(
            LogLevel::Info,
            format!("Steam launch URI: {}", crate::steam::isaac_launch_uri()),
        );
        Ok(())
    }

    pub fn stop_session(&mut self) {
        if let Some(handle) = self.session.take() {
            self.apply_stopped_session_events(handle.stop());
            self.state.last_stop_reason = Some(state::SessionStopReason::UserStopped);
            self.cleanup_hook_launch_parameters("user stopped session");
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
                "Readiness probe started: relay={relay} samples_per_case={} payload_bytes={:?} connection_profiles=[{}]",
                probe::READINESS_PROBE_SAMPLES_PER_CASE,
                probe::READINESS_PROBE_PAYLOAD_BYTES,
                probe::READINESS_PROBE_CONNECTION_PROFILES
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
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
        self.hook_receive_probe = Some(probe::spawn_hook_receive_probe(
            self.state.hook_log_path_written(),
        ));
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

    fn apply_stopped_session_events(&mut self, events: Vec<state::RuntimeEvent>) {
        for event in events {
            match event {
                state::RuntimeEvent::Log(level, message) => self.push_log(level, message),
                state::RuntimeEvent::CounterDelta(delta) => self.state.counters.add(delta),
                state::RuntimeEvent::SessionHealthSnapshot(snapshot) => {
                    if let Some(incident) = self.state.record_session_health_incident(&snapshot) {
                        self.log(
                            LogLevel::Warn,
                            format!("Client incident {}: {}", incident.kind, incident.summary),
                        );
                    }
                    self.state.latest_session_health = Some(*snapshot);
                }
                state::RuntimeEvent::SessionHealthSummary(snapshot) => {
                    let snapshot = *snapshot;
                    self.state.latest_session_health = Some(snapshot.clone());
                    self.state.latest_session_health_summary = Some(snapshot);
                }
                state::RuntimeEvent::HookReceiveProbeWarning(message) => {
                    self.state.latest_hook_receive_probe_warning = Some(message);
                    self.state.latest_hook_receive_probe_error = None;
                }
                state::RuntimeEvent::HookStartup(startup) => {
                    let mut startup = *startup;
                    if startup.launch_parameters_path.is_none() {
                        startup.launch_parameters_path =
                            self.state.hook_launch_parameters_path_written.clone();
                    }
                    self.state.hook_startup = startup;
                }
                state::RuntimeEvent::SessionEnded(reason) => {
                    self.state.last_stop_reason = Some(reason);
                }
                state::RuntimeEvent::Stopped
                | state::RuntimeEvent::ReadinessProbeFinished(_)
                | state::RuntimeEvent::HookReceiveProbeFinished(_) => {}
            }
        }
    }

    fn record_hook_startup_failure(
        &mut self,
        paths: Option<&basement_isaac_injector::NativeHookPaths>,
        message: impl Into<String>,
    ) {
        let message = message.into();
        let mut startup = state::HookStartupState {
            phase: state::HookStartupPhase::Failed,
            launch_parameters_path: self.state.hook_launch_parameters_path_written.clone(),
            message: Some(message.clone()),
            updated_at: state::unix_seconds(),
            ..state::HookStartupState::default()
        };
        if let Some(paths) = paths {
            startup.injector_path = Some(paths.injector.clone());
            startup.hook_path = Some(paths.hook.clone());
            startup.endpoint = Some(format!("{}/UDP", hook_config::HOOK_OUT));
        }
        self.state.hook_startup = startup;
        self.log(LogLevel::Error, message);
    }

    fn cleanup_hook_launch_parameters(&mut self, reason: &str) {
        if hook_launch_parameters_cleanup_finished(&self.state.hook_launch_parameters_cleanup) {
            return;
        }
        let Some(path) = self.state.hook_launch_parameters_path_written.clone() else {
            return;
        };
        let cleanup = match fs::remove_file(&path) {
            Ok(()) => {
                let message = format!("removed path={} reason={reason}", path.display());
                self.log(
                    LogLevel::Info,
                    format!("Native Hook launch parameters {message}"),
                );
                message
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let message = format!("already_missing path={} reason={reason}", path.display());
                self.log(
                    LogLevel::Info,
                    format!("Native Hook launch parameters {message}"),
                );
                message
            }
            Err(error) => {
                let message = format!(
                    "remove_failed path={} reason={reason} error={error}",
                    path.display()
                );
                self.log(
                    LogLevel::Warn,
                    format!("Native Hook launch parameters {message}"),
                );
                message
            }
        };
        self.state.hook_launch_parameters_cleanup = Some(cleanup);
    }

    fn remove_hook_launch_parameters_silent(&self) {
        if let Some(path) = &self.state.hook_launch_parameters_path_written {
            let _ = fs::remove_file(path);
        }
    }
}

fn hook_launch_parameters_cleanup_finished(cleanup: &Option<String>) -> bool {
    cleanup.as_deref().is_some_and(|cleanup| {
        cleanup.starts_with("removed ") || cleanup.starts_with("already_missing ")
    })
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
        self.remove_hook_launch_parameters_silent();
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
    use std::{
        env, fs,
        path::PathBuf,
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

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
            #[cfg(feature = "internal-test")]
            test_run_id: None,
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

    #[test]
    fn diagnostics_include_native_hook_startup_evidence() {
        let mut client = BridgeClient::new();
        client.state.hook_launch_parameters_path_written =
            Some(PathBuf::from("bundle/native-hook/isaac_bridge_config.txt"));
        client.state.hook_launch_parameters_cleanup = Some(
            "removed path=bundle/native-hook/isaac_bridge_config.txt reason=user stopped session"
                .to_owned(),
        );
        client.state.hook_startup = state::HookStartupState {
            phase: state::HookStartupPhase::WaitingForHookEndpoint,
            process_name: Some("isaac-ng.exe".to_owned()),
            pid: Some(42),
            injector_path: Some(PathBuf::from("bundle/basement-isaac-injector.exe")),
            hook_path: Some(PathBuf::from("bundle/native-hook/basement_native_hook.dll")),
            launch_parameters_path: Some(PathBuf::from(
                "bundle/native-hook/isaac_bridge_config.txt",
            )),
            endpoint: Some("127.0.0.1:25901/UDP".to_owned()),
            injected: true,
            endpoint_ready: false,
            access_denied: false,
            message: Some("waiting for endpoint".to_owned()),
            updated_at: 123,
        };

        let text = client.diagnostics_text();

        assert!(text.contains("native hook startup:"));
        assert!(text.contains("phase: waiting_for_hook_endpoint"));
        assert!(text.contains("process_name: isaac-ng.exe"));
        assert!(text.contains("injector_path: bundle/basement-isaac-injector.exe"));
        assert!(text.contains("hook_path: bundle/native-hook/basement_native_hook.dll"));
        assert!(
            text.contains("launch_parameters_path: bundle/native-hook/isaac_bridge_config.txt")
        );
        assert!(text.contains("launch_parameters_cleanup: removed path="));
    }

    #[test]
    fn cleanup_hook_launch_parameters_keeps_first_successful_result() {
        let directory = unique_test_dir("hook-launch-cleanup");
        let path = directory.join("isaac_bridge_config.txt");
        fs::write(&path, "sidecar=127.0.0.1:25900\n").expect("write launch parameters");
        let mut client = BridgeClient::new();
        client.state.hook_launch_parameters_path_written = Some(path.clone());

        client.cleanup_hook_launch_parameters("user stopped session");

        assert!(!path.exists());
        let cleanup = client
            .state
            .hook_launch_parameters_cleanup
            .clone()
            .expect("cleanup result should be recorded");
        assert!(cleanup.starts_with("removed "));
        assert!(cleanup.contains("reason=user stopped session"));

        client.cleanup_hook_launch_parameters("session ended");

        assert_eq!(
            client.state.hook_launch_parameters_cleanup.as_deref(),
            Some(cleanup.as_str())
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn cleanup_hook_launch_parameters_records_already_missing() {
        let directory = unique_test_dir("hook-launch-cleanup-missing");
        let path = directory.join("isaac_bridge_config.txt");
        let mut client = BridgeClient::new();
        client.state.hook_launch_parameters_path_written = Some(path);

        client.cleanup_hook_launch_parameters("session ended");

        let cleanup = client
            .state
            .hook_launch_parameters_cleanup
            .as_deref()
            .expect("cleanup result should be recorded");
        assert!(cleanup.starts_with("already_missing "));
        assert!(cleanup.contains("reason=session ended"));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn hook_receive_warning_clears_previous_probe_error() {
        let mut client = BridgeClient::new();
        client.state.latest_hook_receive_probe_error = Some("previous error".to_owned());

        client.apply_stopped_session_events(vec![state::RuntimeEvent::HookReceiveProbeWarning(
            "sampling warning".to_owned(),
        )]);

        assert_eq!(
            client.state.latest_hook_receive_probe_warning.as_deref(),
            Some("sampling warning")
        );
        assert!(client.state.latest_hook_receive_probe_error.is_none());
    }

    #[test]
    fn startup_failure_record_keeps_artifact_and_launch_parameter_paths() {
        let mut client = BridgeClient::new();
        let paths = basement_isaac_injector::NativeHookPaths {
            injector: PathBuf::from("bundle/basement-isaac-injector.exe"),
            hook: PathBuf::from("bundle/native-hook/basement_native_hook.dll"),
        };
        client.state.hook_launch_parameters_path_written =
            Some(PathBuf::from("bundle/native-hook/isaac_bridge_config.txt"));

        client.record_hook_startup_failure(Some(&paths), "Bridge worker startup failed");

        assert_eq!(
            client.state.hook_startup.phase,
            state::HookStartupPhase::Failed
        );
        assert_eq!(
            client.state.hook_startup.injector_path.as_ref(),
            Some(&paths.injector)
        );
        assert_eq!(
            client.state.hook_startup.hook_path.as_ref(),
            Some(&paths.hook)
        );
        assert_eq!(
            client.state.hook_startup.launch_parameters_path.as_deref(),
            Some(PathBuf::from("bundle/native-hook/isaac_bridge_config.txt").as_path())
        );
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let path =
            env::temp_dir().join(format!("basement-bridge-{name}-{}-{nonce}", process::id()));
        fs::create_dir_all(&path).expect("create test directory");
        path
    }
}
