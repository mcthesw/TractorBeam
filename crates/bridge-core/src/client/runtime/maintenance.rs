use super::*;

impl BridgeClient {
    pub(crate) fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        self.push_log(level, message);
    }

    pub fn clear_logs(&mut self) {
        self.state.logs.clear();
    }

    pub(super) fn push_log(&mut self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        self.log_sink
            .emit(self.active_log_context.as_ref(), level, &message);
        self.state.logs.push(log_entry(level, message));
        trim_logs(&mut self.state.logs);
    }

    pub(super) fn apply_stopped_session_events(&mut self, events: Vec<state::RuntimeEvent>) {
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
                    self.refresh_smoothness();
                }
                state::RuntimeEvent::SessionHealthSummary(snapshot) => {
                    let snapshot = *snapshot;
                    self.state.latest_session_health = Some(snapshot.clone());
                    self.state.latest_session_health_summary = Some(snapshot);
                    self.refresh_smoothness();
                }
                state::RuntimeEvent::HookStartup(startup) => {
                    let mut startup = *startup;
                    if startup.launch_parameters_path.is_none() {
                        startup.launch_parameters_path =
                            self.state.hook_launch_parameters_path_written.clone();
                    }
                    self.state.hook_startup = startup;
                }
                state::RuntimeEvent::HookIpc(ipc) => self.state.hook_ipc = *ipc,
                state::RuntimeEvent::SessionEnded(reason)
                    if self.state.last_stop_reason.is_none() =>
                {
                    self.state.last_stop_reason = Some(reason)
                }
                state::RuntimeEvent::SessionEnded(_)
                | state::RuntimeEvent::Stopped
                | state::RuntimeEvent::ReadinessProbeFinished(_)
                | state::RuntimeEvent::HookReceiveProbeFinished(_)
                | state::RuntimeEvent::LightPingFinished(_)
                | state::RuntimeEvent::RoomPeersUpdated(_)
                | state::RuntimeEvent::RoomPathQualityUpdated(_)
                | state::RuntimeEvent::RelayLinkChanged(_) => {}
            }
        }
    }

    pub(super) fn record_hook_startup_failure(
        &mut self,
        paths: Option<&tractor_beam_isaac_injector::NativeHookPaths>,
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
            startup.endpoint = Some("local IPC".to_owned());
        }
        self.state.hook_startup = startup;
        self.log(LogLevel::Error, message);
    }

    pub(super) fn cleanup_hook_launch_parameters(&mut self, reason: &str) {
        if cleanup_finished(&self.state.hook_launch_parameters_cleanup) {
            return;
        }
        let Some(path) = self.state.hook_launch_parameters_path_written.clone() else {
            return;
        };
        let cleanup = match fs::remove_file(&path) {
            Ok(()) => format!("removed path={} reason={reason}", path.display()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                format!("already_missing path={} reason={reason}", path.display())
            }
            Err(error) => format!(
                "remove_failed path={} reason={reason} error={error}",
                path.display()
            ),
        };
        let level = if cleanup.starts_with("remove_failed") {
            LogLevel::Warn
        } else {
            LogLevel::Info
        };
        self.log(level, format!("Native Hook launch parameters {cleanup}"));
        self.state.hook_launch_parameters_cleanup = Some(cleanup);
    }

    pub(super) fn refresh_smoothness(&mut self) {
        self.state.smoothness = super::super::smoothness::assess_smoothness(
            self.state.latest_session_health.as_ref(),
            &self.state.room_path_quality,
            state::unix_seconds(),
        );
    }

    pub(super) fn remove_hook_launch_parameters_silent(&self) {
        if let Some(path) = &self.state.hook_launch_parameters_path_written {
            let _ = fs::remove_file(path);
        }
    }
}

fn cleanup_finished(cleanup: &Option<String>) -> bool {
    cleanup.as_deref().is_some_and(|cleanup| {
        cleanup.starts_with("removed ") || cleanup.starts_with("already_missing ")
    })
}
