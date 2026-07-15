use std::{fs, io};

use super::{
    ConfigError, InputDelayError, LoadedClientConfig, RelayEndpoint, SessionConfig, SessionMode,
    SessionRouteConfig, hook_config, hook_ipc,
    logging::{
        ClientLogSink, ClientSessionLog, ClientSessionLogContext, ClientSessionLogRoute,
        TracingClientLogSink,
    },
    probe, session,
    state::{self, log_entry, trim_logs},
};

mod maintenance;
use crate::client::{LogLevel, RuntimeState};

#[derive(Debug)]
pub struct BridgeClient {
    pub(super) state: RuntimeState,
    pub(super) session: Option<session::SessionHandle>,
    pub(super) loaded_config: LoadedClientConfig,
    pub(super) log_sink: Box<dyn ClientLogSink>,
    active_session_log: Option<Box<dyn ClientSessionLog>>,
    readiness_probe: Option<probe::ProbeHandle>,
    hook_receive_probe: Option<probe::ProbeHandle>,
    light_ping_probe: Option<probe::LightPingHandle>,
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
            light_ping_probe: None,
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
        if let Some(handle) = &self.light_ping_probe {
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
                state::RuntimeEvent::HookStartup(startup) => {
                    let mut startup = *startup;
                    if startup.launch_parameters_path.is_none() {
                        startup.launch_parameters_path =
                            self.state.hook_launch_parameters_path_written.clone();
                    }
                    self.state.hook_startup = startup;
                }
                state::RuntimeEvent::HookIpc(ipc) => {
                    let ipc = *ipc;
                    if ipc.connection == state::HookIpcConnectionState::Connected
                        && self.state.hook_startup.injected
                    {
                        self.state.hook_startup.phase = state::HookStartupPhase::Ready;
                        self.state.hook_startup.endpoint_ready = true;
                        self.state.hook_startup.message =
                            Some("Native Hook local IPC is ready".to_owned());
                        self.state.hook_startup.updated_at = state::unix_seconds();
                    }
                    self.state.hook_ipc = ipc;
                }
                state::RuntimeEvent::SessionHealthSnapshot(snapshot) => {
                    if let Some(incident) = self.state.record_session_health_incident(&snapshot) {
                        self.log(
                            LogLevel::Warn,
                            format!("Client incident {}: {}", incident.kind, incident.summary),
                        );
                    }
                    self.state.latest_session_health = Some(*snapshot);
                    self.refresh_smoothness();
                }
                state::RuntimeEvent::SessionHealthSummary(snapshot) => {
                    let snapshot = *snapshot;
                    self.state.latest_session_health = Some(snapshot.clone());
                    self.state.latest_session_health_summary = Some(snapshot);
                    self.refresh_smoothness();
                }
                state::RuntimeEvent::SessionEnded(reason) => {
                    if self.state.last_stop_reason.is_none() {
                        self.state.last_stop_reason = Some(reason.clone());
                    }
                }
                state::RuntimeEvent::Stopped => {
                    self.state.status = state::SessionStatus::Idle;
                    self.state.active_session_mode = None;
                    self.active_session_log = None;
                    should_clear = true;
                }
                state::RuntimeEvent::LightPingFinished(report) => {
                    let report = *report;
                    self.log(
                        LogLevel::Info,
                        format!(
                            "Light ping {}: relay={} {} received={}/{} median={}ms",
                            report
                                .target
                                .relay_name
                                .as_deref()
                                .unwrap_or(&report.target.endpoint.to_string()),
                            report.target.endpoint,
                            report.latency_label(),
                            report.received,
                            report.sent,
                            report
                                .median_rtt_ms
                                .map_or("-".to_owned(), |ms| ms.to_string()),
                        ),
                    );
                    self.upsert_light_ping_report(report);
                }
                state::RuntimeEvent::RoomPeersUpdated(peers) => {
                    self.state.room_peers = peers;
                }
                state::RuntimeEvent::RoomPathQualityUpdated(quality) => {
                    self.state.room_path_quality = quality;
                    self.refresh_smoothness();
                }
                state::RuntimeEvent::RelayLinkChanged(link) => {
                    self.state.relay_link = link;
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
        let log_route = match &config.route {
            SessionRouteConfig::ExternalRelay(route) => ClientSessionLogRoute::ExternalRelay {
                relay_name: route.relay_name.clone(),
                relay: route.relay.clone(),
                transport: route.transport,
            },
            SessionRouteConfig::LanDirect(_) => ClientSessionLogRoute::LanDirect,
        };
        self.stop_session();
        self.state.last_stop_reason = None;
        self.state.latest_hook_receive_probe = None;
        self.state.latest_hook_receive_probe_error = None;
        self.state.latest_session_health = None;
        self.state.latest_session_health_summary = None;
        self.state.smoothness = super::SmoothnessSnapshot::default();
        self.state.latest_input_delay_status = None;
        self.state.active_session_mode = None;
        self.state.hook_launch_parameters_path_written = None;
        self.state.hook_launch_parameters_cleanup = None;
        self.state.hook_startup = state::HookStartupState::default();
        self.state.hook_ipc = state::HookIpcState::default();
        self.state.client_incidents.clear();
        self.state.room_peers.clear();
        self.state.room_path_quality.clear();
        self.state.relay_link = state::RelayLinkState::Inactive;
        self.active_session_log = self
            .log_sink
            .start_session(ClientSessionLogContext {
                route: log_route,
                mode: config.mode,
            })
            .ok();

        let native_hook = if config.mode != SessionMode::Official {
            let native_hook_paths = match tractor_beam_isaac_injector::resolve_native_hook_paths() {
                Ok(paths) => paths,
                Err(error) => {
                    let message = format!("Native Hook artifact resolution failed: {error}");
                    self.record_hook_startup_failure(None, message.clone());
                    self.active_session_log = None;
                    return Err(io::Error::other(message).into());
                }
            };
            let ipc = hook_ipc::HookIpcSession::generate();
            let write = match hook_config::write_hook_config(config, &native_hook_paths, &ipc) {
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
                endpoint: Some("local IPC".to_owned()),
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
            Some(session::SessionNativeHook::new(native_hook_paths, ipc))
        } else {
            self.state.hook_launch_parameters_path_written = None;
            None
        };
        let session = session::spawn_bridge_worker_background(config.clone(), native_hook);

        if let Err(error) = crate::steam::launch_isaac() {
            self.apply_stopped_session_events(session.stop());
            self.cleanup_hook_launch_parameters("Steam launch failed");
            self.active_session_log = None;
            return Err(error.into());
        }

        self.session = Some(session);
        self.state.status = state::SessionStatus::Running;
        self.state.active_session_mode = Some(config.mode);
        self.log(LogLevel::Info, format!("Starting {} session", config.mode));
        if config.mode != SessionMode::Official {
            match &config.route {
                SessionRouteConfig::ExternalRelay(route) => {
                    if let Some(name) = &route.relay_name {
                        self.log(LogLevel::Info, format!("Relay preset: {name}"));
                    }
                    self.log(LogLevel::Info, format!("Relay endpoint: {}", route.relay));
                    self.log(LogLevel::Info, format!("Transport: {}", route.transport));
                }
                SessionRouteConfig::LanDirect(_) => {
                    self.log(LogLevel::Info, "Session route: direct LAN");
                }
            }
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
            if self.state.last_stop_reason.is_none() {
                self.state.last_stop_reason = Some(state::SessionStopReason::UserStopped);
            }
            self.cleanup_hook_launch_parameters("user stopped session");
        }
        self.state.status = state::SessionStatus::Idle;
        self.state.active_session_mode = None;
        self.active_session_log = None;
        self.log(LogLevel::Info, "Session stopped");
    }

    pub fn shutdown(&mut self) {
        self.stop_session();
        if let Some(handle) = self.readiness_probe.take() {
            handle.finish();
        }
        if let Some(handle) = self.hook_receive_probe.take() {
            handle.finish();
        }
        if let Some(handle) = self.light_ping_probe.take() {
            handle.finish();
        }
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
        self.hook_receive_probe =
            Some(probe::spawn_hook_receive_probe(self.state.hook_ipc.clone()));
        self.state.hook_probe_running = true;
        self.state.latest_hook_receive_probe_error = None;
        self.log(LogLevel::Info, "Hook receive probe started");
        Ok(())
    }

    pub fn start_light_ping_probes(
        &mut self,
        targets: Vec<probe::LightPingTarget>,
    ) -> Result<(), ClientError> {
        if targets.is_empty() {
            return Ok(());
        }
        self.state.light_ping_reports.clear();
        let handle = probe::spawn_light_ping_probes(targets.clone())?;
        self.light_ping_probe = Some(handle);
        self.log(
            LogLevel::Info,
            format!("Light ping probes started for {} relay(s)", targets.len()),
        );
        Ok(())
    }

    fn upsert_light_ping_report(&mut self, report: probe::LightPingReport) {
        let key = report.target.endpoint.clone();
        if let Some(existing) = self
            .state
            .light_ping_reports
            .iter_mut()
            .find(|r| r.target.endpoint == key)
        {
            *existing = report;
        } else {
            self.state.light_ping_reports.push(report);
        }
    }
}

impl Default for BridgeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        self.session = None;
        self.remove_hook_launch_parameters_silent();
        self.readiness_probe = None;
        self.hook_receive_probe = None;
        self.light_ping_probe = None;
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
    #[error("{0}")]
    InputDelay(#[from] InputDelayError),
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
