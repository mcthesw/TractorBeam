use super::*;

impl BridgeApp {
    pub(super) fn selected_relay_preset(&self) -> Option<&RelayPreset> {
        self.selected_relay
            .and_then(|index| self.relay_presets.get(index))
    }

    pub(super) fn apply_selected_relay_defaults(&mut self) {
        let Some(relay) = self.selected_relay_preset().cloned() else {
            return;
        };
        self.transport = relay.preferred_transport(self.transport);
        self.relay_host = relay.endpoint.host;
        self.relay_port = relay.endpoint.port;
    }

    pub(super) fn preset_supports_transport(&self, transport: TransportChoice) -> bool {
        self.selected_relay_preset()
            .is_none_or(|relay| relay.supports(transport))
    }

    pub(super) fn copy_join_code(&self) -> Result<String, tractor_beam_core::JoinCodeError> {
        let relay_id = self.selected_relay_preset().map(|relay| relay.id.clone());
        JoinCode::ExternalRelay(RelayJoinCode {
            relay_id,
            relay_host: self.relay_host.trim().to_owned(),
            relay_port: self.relay_port,
            session_credential: self.session_credential,
        })
        .encode()
    }

    pub(super) fn import_join_code(&mut self) -> bool {
        let input = if self.join_code_input.trim().is_empty() {
            self.join_code_message = Some(t!("join_code.required").into_owned());
            return false;
        } else {
            self.join_code_input.trim().to_owned()
        };
        match JoinCode::decode(&input) {
            Ok(JoinCode::ExternalRelay(code)) => {
                if let Some(ref relay_id) = code.relay_id {
                    if let Some(index) = self
                        .relay_presets
                        .iter()
                        .position(|relay| &relay.id == relay_id)
                    {
                        self.selected_relay = Some(index);
                        self.apply_selected_relay_defaults();
                    } else {
                        self.selected_relay = None;
                        self.relay_host = code.relay_host.clone();
                        self.relay_port = code.relay_port;
                    }
                } else {
                    self.selected_relay = None;
                    self.relay_host = code.relay_host.clone();
                    self.relay_port = code.relay_port;
                }
                self.session_credential = code.session_credential;
                self.join_code_message = Some(t!("join_code.imported").into_owned());
                self.status_message = None;
                self.persist_selection();
                true
            }
            Ok(JoinCode::LanDirect(_)) => {
                self.join_code_message = Some(t!("join_code.lan_not_available").into_owned());
                false
            }
            Err(error) => {
                self.join_code_message = Some(format!("{}: {error}", t!("join_code.invalid")));
                false
            }
        }
    }

    pub(super) fn reveal_troubleshooting_package(&mut self) {
        let Some(path) = self.last_troubleshooting_package.as_ref() else {
            return;
        };
        let target = path.parent().unwrap_or(path);
        if let Err(error) = open::that_detached(target) {
            tracing::warn!(error = %error, path = %path.display(), "Could not reveal Troubleshooting Package");
            self.status_message = Some(StatusMessage::LogOpenFailed);
        }
    }

    pub(super) fn join_code_dialog(&mut self, context: &egui::Context) {
        if !self.join_code_dialog_open {
            return;
        }
        let mut open = true;
        let mut cancel = false;
        let mut confirm = false;
        egui::Window::new(t!("join_code.dialog_title"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .default_width(520.0)
            .open(&mut open)
            .resizable(false)
            .show(context, |ui| {
                ui.label(t!("join_code.paste_hint"));
                let response = ui.add(
                    egui::TextEdit::multiline(&mut self.join_code_input)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                response.request_focus();
                if let Some(message) = &self.join_code_message {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
                ui.horizontal(|ui| {
                    if ui.button(t!("join_code.confirm")).clicked() {
                        confirm = true;
                    }
                    if ui.button(t!("join_code.cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if confirm && self.import_join_code() {
            open = false;
        }
        if cancel {
            open = false;
            self.join_code_input.clear();
            self.join_code_message = None;
        }
        self.join_code_dialog_open = open;
    }
}
