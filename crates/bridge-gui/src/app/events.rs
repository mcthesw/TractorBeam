use super::*;

impl BridgeApp {
    pub(super) fn sync_application(&mut self) {
        self.application_snapshot = self.application.snapshot();
        self.initialize_form();
        for event in self.application.drain_events() {
            self.handle_application_event(event);
        }
    }

    fn handle_application_event(&mut self, event: ApplicationEvent) {
        match event {
            ApplicationEvent::StartFinished(result) => match result {
                Ok(()) => {
                    self.status_message = None;
                    self.start_error_dialog_open = false;
                }
                Err(error) => {
                    self.status_message = Some(StatusMessage::from_client_error(&error));
                    self.start_error_dialog_open = true;
                }
            },
            ApplicationEvent::StopFinished => self.status_message = None,
            ApplicationEvent::AccountsRefreshed => {
                self.selected_account =
                    initial_selected_account(&self.client_state().detected_accounts, None);
                self.persist_selection();
            }
            ApplicationEvent::ReadinessProbeStarted(result)
            | ApplicationEvent::HookReceiveProbeStarted(result)
            | ApplicationEvent::LightPingStarted(result) => match result {
                Ok(()) => self.status_message = None,
                Err(error) => {
                    self.status_message = Some(StatusMessage::from_client_error(&error));
                }
            },
            ApplicationEvent::InputDelayReadFinished(result) => match result {
                Ok(report) => {
                    self.input_delay_value = report.value.to_string();
                    self.input_delay_message =
                        Some(format!("{}: {}", t!("input_delay.read_ok"), report.value));
                    self.status_message = None;
                }
                Err(error) => self.set_input_delay_error(&error),
            },
            ApplicationEvent::InputDelayWriteFinished(result) => match result {
                Ok(report) => {
                    let mut message = format!("{}: {}", t!("input_delay.write_ok"), report.value);
                    if report.value < 0 {
                        message.push_str(t!("input_delay.negative_hint").as_ref());
                    }
                    self.input_delay_message = Some(message);
                    self.status_message = None;
                }
                Err(error) => self.set_input_delay_error(&error),
            },
            ApplicationEvent::LogDirectoryOpened(result) => match result {
                Ok(path) => {
                    self.status_message = None;
                    tracing::info!(directory = %path.display(), "Opened log directory");
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Could not open log directory");
                    self.status_message = Some(StatusMessage::LogOpenFailed);
                }
            },
            ApplicationEvent::DiagnosticsBundleExported(result) => match result {
                Ok(Some(path)) => {
                    tracing::info!(path = %path.display(), "Diagnostics Bundle export completed");
                    self.last_diagnostics_bundle = Some(path);
                    self.status_message = Some(StatusMessage::DiagnosticsExported);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "Could not export Diagnostics Bundle");
                    self.status_message = Some(StatusMessage::DiagnosticsExportFailed);
                }
            },
            ApplicationEvent::ClipboardReadFinished(result) => match result {
                Ok(text) if !text.trim().is_empty() => {
                    self.join_code_input = text;
                    let _ = self.import_join_code();
                }
                Ok(_) => {
                    self.join_code_input.clear();
                    self.join_code_message = None;
                    self.lan_probe_results.clear();
                    self.selected_lan_probe = None;
                    self.pending_lan_invitation = None;
                    self.join_code_dialog_open = true;
                }
                Err(error) => {
                    tracing::debug!(error = %error, "Clipboard text unavailable; opening manual Join Code input");
                    self.join_code_input.clear();
                    self.join_code_message = None;
                    self.lan_probe_results.clear();
                    self.selected_lan_probe = None;
                    self.pending_lan_invitation = None;
                    self.join_code_dialog_open = true;
                }
            },
            ApplicationEvent::LanAdaptersEnumerated(result) => match result {
                Ok(adapters) if !adapters.is_empty() => {
                    self.lan_adapters = default_lan_adapter_selection(adapters);
                    self.lan_create_dialog_open = true;
                    self.join_code_message = None;
                }
                Ok(_) => self.join_code_message = Some(t!("lan.no_adapters").into_owned()),
                Err(error) => {
                    self.join_code_message = Some(format!("{}: {error}", t!("lan.adapters_failed")))
                }
            },
            ApplicationEvent::LanProbeFinished(result) => match result {
                Ok((_, results))
                    if lan_probe_disposition(results.len())
                        == LanProbeDisposition::NoneReachable =>
                {
                    self.join_code_message = Some(t!("lan.probe_none").into_owned());
                }
                Ok((invitation, results))
                    if lan_probe_disposition(results.len()) == LanProbeDisposition::JoinOne =>
                {
                    let endpoint = results[0].endpoint;
                    self.pending_lan_invitation = Some(invitation.clone());
                    self.join_lan_room(invitation, endpoint);
                }
                Ok((invitation, results)) => {
                    self.pending_lan_invitation = Some(invitation);
                    self.lan_probe_results = results;
                    self.selected_lan_probe = Some(0);
                    self.join_code_dialog_open = true;
                }
                Err(error) => {
                    self.join_code_message = Some(format!("{}: {error}", t!("lan.probe_failed")))
                }
            },
            ApplicationEvent::LanRoomCreated(result) => match result {
                Ok(code) => {
                    self.route = RouteChoice::LanDirect;
                    self.join_code_message = Some(t!("lan.created").into_owned());
                    self.lan_create_dialog_open = false;
                    self.pending_lan_invitation =
                        JoinCode::decode(&code).ok().and_then(|code| match code {
                            JoinCode::LanDirect(code) => Some(code),
                            _ => None,
                        });
                }
                Err(error) => {
                    self.join_code_message = Some(format!("{}: {error}", t!("lan.create_failed")))
                }
            },
            ApplicationEvent::LanRoomJoined(result) => match result {
                Ok(()) => {
                    self.route = RouteChoice::LanDirect;
                    self.join_code_dialog_open = false;
                    self.join_code_message = Some(t!("lan.joined").into_owned());
                }
                Err(error) => {
                    self.join_code_message = Some(format!("{}: {error}", t!("lan.join_failed")))
                }
            },
            ApplicationEvent::SelectionSaveFailed(error) => {
                tracing::warn!(error = %error, "Could not save GUI selection");
                self.status_message = Some(StatusMessage::SelectionSaveFailed);
            }
            ApplicationEvent::CommandRejected => self.show_busy_status(),
            ApplicationEvent::ShutdownComplete => {}
        }
    }

    pub(super) fn handle_close(&mut self, context: &egui::Context) {
        let close_requested = context.input(|input| input.viewport().close_requested());
        if close_requested && !self.shutdown.close_allowed {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.shutdown.request(Instant::now()) {
                self.application.request_shutdown();
            }
        }

        if self.application_snapshot.shutdown_complete {
            self.shutdown.complete();
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.shutdown.timed_out(Instant::now()) {
            tracing::error!("Application shutdown exceeded the three-second deadline");
            std::process::exit(0);
        }
    }
}
