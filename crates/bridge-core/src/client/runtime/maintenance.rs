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
                    self.apply_hook_startup_state(*startup);
                }
                state::RuntimeEvent::HookIpc(ipc) => self.apply_hook_ipc_state(*ipc),
                state::RuntimeEvent::HookTargetObserved(target) => {
                    self.observe_game_target(target);
                }
                state::RuntimeEvent::SessionEnded(reason)
                    if self.state.last_stop_reason.is_none() =>
                {
                    self.state.last_stop_reason = Some(reason)
                }
                state::RuntimeEvent::SessionEnded(_)
                | state::RuntimeEvent::GameplayStopped
                | state::RuntimeEvent::Stopped
                | state::RuntimeEvent::ReadinessProbeFinished(_)
                | state::RuntimeEvent::HookReceiveProbeFinished(_)
                | state::RuntimeEvent::LightPingFinished(_)
                | state::RuntimeEvent::RoomPeersUpdated(_)
                | state::RuntimeEvent::RoomPathQualityUpdated(_)
                | state::RuntimeEvent::RelayLinkChanged(_) => {}
            }
        }
        self.finish_game_exit();
    }

    pub(super) fn apply_hook_startup_state(&mut self, mut startup: state::HookStartupState) {
        if startup.launch_parameters_path.is_none() {
            startup.launch_parameters_path = self.state.hook_launch_parameters_path_written.clone();
        }
        self.state.hook_startup = startup;
        self.reconcile_hook_startup();
    }

    pub(super) fn apply_hook_ipc_state(&mut self, ipc: state::HookIpcState) {
        self.state.hook_ipc = ipc;
        self.reconcile_hook_startup();
        self.refresh_steam_identity_mismatch();
    }

    fn reconcile_hook_startup(&mut self) {
        if !self.state.hook_startup.injected
            || matches!(
                self.state.hook_startup.phase,
                state::HookStartupPhase::Failed | state::HookStartupPhase::Cancelled
            )
        {
            return;
        }

        let (phase, endpoint_ready, message) = match (
            self.state.hook_ipc.connection,
            self.state.hook_ipc.installation,
        ) {
            (_, state::HookInstallState::Failed) => (
                state::HookStartupPhase::Failed,
                false,
                self.state.hook_ipc.last_error.as_ref().map_or_else(
                    || "Native Hook startup failed; fully exit Isaac and try again".to_owned(),
                    Clone::clone,
                ),
            ),
            (state::HookIpcConnectionState::Failed, _) => (
                state::HookStartupPhase::Failed,
                false,
                self.state.hook_ipc.last_error.as_ref().map_or_else(
                    || "Native Hook local IPC failed".to_owned(),
                    |error| format!("Native Hook local IPC failed: {error}"),
                ),
            ),
            (state::HookIpcConnectionState::Connected, state::HookInstallState::Ready) => (
                state::HookStartupPhase::Ready,
                true,
                "Native Hook is ready".to_owned(),
            ),
            (state::HookIpcConnectionState::Connected, _) => (
                state::HookStartupPhase::EndpointReady,
                true,
                "Native Hook connected; installing Steam networking hooks".to_owned(),
            ),
            _ => (
                state::HookStartupPhase::WaitingForHookEndpoint,
                false,
                "Injection succeeded; waiting for Native Hook".to_owned(),
            ),
        };
        self.state.hook_startup.phase = phase;
        self.state.hook_startup.endpoint_ready = endpoint_ready;
        self.state.hook_startup.message = Some(message);
        self.state.hook_startup.updated_at = state::unix_seconds();
    }

    pub(super) fn finish_game_exit(&mut self) {
        if !matches!(
            self.state.last_stop_reason,
            Some(state::SessionStopReason::GameExited { .. })
        ) {
            return;
        }
        self.state.status = state::SessionStatus::Idle;
        self.state.active_session_mode = None;
        self.state.hook_runtime_active = false;
        self.state.hook_startup = state::HookStartupState::default();
        self.state.hook_ipc = state::HookIpcState::default();
        self.state.steam_identity_mismatch = None;
        self.resume_gameplay_after_rejoin = false;
        self.clear_gameplay_targets();
        self.active_log_context = None;
    }

    pub(super) fn refresh_steam_identity_mismatch(&mut self) {
        let mismatch = self
            .state
            .relay_room_steam_id64
            .zip(self.state.hook_ipc.game_steam_id64)
            .and_then(|(room_steam_id64, game_steam_id64)| {
                (room_steam_id64 != game_steam_id64).then_some(state::SteamIdentityMismatch {
                    room_steam_id64,
                    game_steam_id64,
                })
            });
        if let Some(mismatch) = mismatch
            && Some(mismatch) != self.state.steam_identity_mismatch
        {
            self.log(
                LogLevel::Warn,
                format!(
                    "Steam identity mismatch: Isaac uses {} but the Relay room uses {}",
                    mismatch.game_steam_id64, mismatch.room_steam_id64
                ),
            );
        }
        self.state.steam_identity_mismatch = mismatch;
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
        #[cfg(target_os = "linux")]
        self.remove_proton_sidecar(reason);
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

    #[cfg(target_os = "linux")]
    fn remove_proton_sidecar(&mut self, reason: &str) {
        let Some(sidecar) = self.proton_sidecar.clone() else {
            return;
        };
        match tractor_beam_isaac_injector::remove_proton_winmm_sidecar(&sidecar) {
            Ok(()) => {
                self.proton_sidecar = None;
                self.log(
                    LogLevel::Info,
                    format!(
                        "Proton Native Hook sidecar removed path={} reason={reason}",
                        sidecar.dll.display()
                    ),
                );
            }
            Err(error) => self.log(
                LogLevel::Warn,
                format!(
                    "Proton Native Hook sidecar remove_failed path={} reason={reason} error={error}",
                    sidecar.dll.display()
                ),
            ),
        }
    }

    pub(super) fn refresh_smoothness(&mut self) {
        self.state.smoothness = super::super::smoothness::assess_smoothness(
            self.state.latest_session_health.as_ref(),
            &self.state.room_path_quality,
            state::unix_seconds(),
        );
    }

    pub(super) fn prepare_gameplay_start(
        &mut self,
        route: ClientSessionLogRoute,
        mode: SessionMode,
    ) {
        self.state.last_stop_reason = None;
        self.state.latest_hook_receive_probe = None;
        self.state.latest_hook_receive_probe_error = None;
        self.state.latest_session_health = None;
        self.state.latest_session_health_summary = None;
        self.state.smoothness = super::super::SmoothnessSnapshot::default();
        self.state.latest_input_delay_status = None;
        self.state.active_session_mode = None;
        self.state.client_incidents.clear();
        self.observed_game_targets.clear();
        self.state.missing_game_targets.clear();
        if self.relay_room.is_none() {
            self.relay_peers_known = false;
            self.state.room_peers.clear();
            self.state.room_path_quality.clear();
            self.state.relay_link = state::RelayLinkState::Inactive;
        }
        self.active_log_context = Some(ClientSessionLogContext { route, mode });
    }

    pub(super) fn observe_game_target(&mut self, target: u64) {
        if self.state.status != state::SessionStatus::Running {
            return;
        }
        if !self.observed_game_targets.contains(&target) {
            self.observed_game_targets.push(target);
            self.refresh_missing_game_targets();
        }
    }

    pub(super) fn refresh_missing_game_targets(&mut self) {
        if self.state.status != state::SessionStatus::Running || !self.relay_peers_known {
            return;
        }
        let missing = self
            .observed_game_targets
            .iter()
            .copied()
            .filter(|target| {
                !self
                    .state
                    .room_peers
                    .iter()
                    .any(|peer| peer.steam_id64 == *target)
            })
            .collect::<Vec<_>>();
        let newly_missing = missing
            .iter()
            .filter(|target| !self.state.missing_game_targets.contains(target))
            .copied()
            .collect::<Vec<_>>();
        self.state.missing_game_targets = missing;
        if !newly_missing.is_empty() {
            self.log(
                LogLevel::Warn,
                format!(
                    "Game target SteamID is not in the current Relay room: {}",
                    newly_missing
                        .iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            );
        }
    }

    pub(super) fn clear_gameplay_targets(&mut self) {
        self.observed_game_targets.clear();
        self.state.missing_game_targets.clear();
    }

    pub(super) fn log_session_route(&mut self, config: &SessionConfig) {
        if config.mode == SessionMode::Official {
            return;
        }
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

    pub(super) fn stop_session_runtime(&mut self, reason: &str) {
        if let Some(handle) = self.session.take() {
            self.apply_stopped_session_events(handle.stop());
            self.cleanup_hook_launch_parameters(reason);
        }
        self.state.status = state::SessionStatus::Idle;
        self.state.active_session_mode = None;
        self.state.hook_runtime_active = false;
        self.active_log_context = None;
    }

    pub(super) fn remove_hook_launch_parameters_silent(&self) {
        if let Some(path) = &self.state.hook_launch_parameters_path_written {
            let _ = fs::remove_file(path);
        }
        #[cfg(target_os = "linux")]
        if let Some(sidecar) = &self.proton_sidecar {
            let _ = tractor_beam_isaac_injector::remove_proton_winmm_sidecar(sidecar);
        }
    }
}

fn cleanup_finished(cleanup: &Option<String>) -> bool {
    cleanup.as_deref().is_some_and(|cleanup| {
        cleanup.starts_with("removed ") || cleanup.starts_with("already_missing ")
    })
}
