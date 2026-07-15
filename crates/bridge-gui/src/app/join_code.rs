use super::widgets::label_with_help;
use super::*;

const LAN_CREATE_DIALOG_MAX_WIDTH: f32 = 400.0;
const LAN_CREATE_DIALOG_HORIZONTAL_MARGIN: f32 = 32.0;

fn adapter_address_tooltip(adapter: &LanAdapter) -> String {
    adapter
        .addresses
        .iter()
        .map(|address| address.address.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn lan_create_dialog_width(viewport_width: f32) -> f32 {
    (viewport_width - LAN_CREATE_DIALOG_HORIZONTAL_MARGIN).clamp(1.0, LAN_CREATE_DIALOG_MAX_WIDTH)
}

fn should_show_lan_create_dialog(dialog_open: bool, route: RouteChoice) -> bool {
    dialog_open && route == RouteChoice::LanDirect
}

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
                if self.application_snapshot.lan_room.is_some() {
                    self.join_code_message = Some(t!("lan.stop_before_switch").into_owned());
                    return false;
                }
                self.route = RouteChoice::ExternalRelay;
                self.pending_lan_invitation = None;
                self.lan_probe_results.clear();
                self.selected_lan_probe = None;
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
            Ok(JoinCode::LanDirect(code)) => {
                if self.application_snapshot.lan_room.is_some() {
                    self.join_code_message = Some(t!("lan.stop_before_switch").into_owned());
                    return false;
                }
                self.lan_probe_results.clear();
                self.selected_lan_probe = None;
                self.pending_lan_invitation = None;
                if self.application.probe_lan_join(code) {
                    self.join_code_message = Some(t!("lan.probing").into_owned());
                    true
                } else {
                    self.show_busy_status();
                    false
                }
            }
            Err(error) => {
                self.join_code_message = Some(format!("{}: {error}", t!("join_code.invalid")));
                false
            }
        }
    }

    pub(super) fn join_lan_room(
        &mut self,
        invitation: LanJoinCode,
        endpoint: std::net::SocketAddr,
    ) {
        let (steam_id64, display_name) = self.current_identity();
        let Ok(steam_id64) = steam_id64.parse::<u64>() else {
            self.join_code_message = Some(t!("lan.identity_required").into_owned());
            return;
        };
        if !self
            .application
            .join_lan_room(steam_id64, display_name, invitation, endpoint)
        {
            self.show_busy_status();
        }
    }

    pub(super) fn lan_create_dialog(&mut self, context: &egui::Context) {
        if !should_show_lan_create_dialog(self.lan_create_dialog_open, self.route) {
            self.lan_create_dialog_open = false;
            return;
        }
        let dialog_width = lan_create_dialog_width(context.content_rect().width());
        let mut open = true;
        let mut cancel = false;
        let mut create = false;
        egui::Window::new(t!("lan.create_title"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .default_width(dialog_width)
            .min_width(dialog_width)
            .max_width(dialog_width)
            .open(&mut open)
            .resizable(false)
            .show(context, |ui| {
                label_with_help(ui, t!("lan.adapters"), t!("lan.disclosure"));
                ui.add_space(6.0);
                let mut selected_count = self
                    .lan_adapters
                    .iter()
                    .filter(|(_, selected)| *selected)
                    .count();
                let adapter_list_width = ui.available_width();
                egui::ScrollArea::both()
                    .id_salt("lan_adapter_list")
                    .max_width(adapter_list_width)
                    .max_height(260.0)
                    .min_scrolled_width(adapter_list_width)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (adapter, selected) in &mut self.lan_adapters {
                            let was_selected = *selected;
                            let can_select = was_selected
                                || selected_count < tractor_beam_core::MAX_SELECTED_LAN_ADAPTERS;
                            ui.horizontal(|ui| {
                                let response = ui
                                    .add_enabled(
                                        can_select,
                                        egui::Checkbox::new(selected, &adapter.name),
                                    )
                                    .on_hover_text(adapter_address_tooltip(adapter));
                                if response.changed() {
                                    if *selected {
                                        selected_count = selected_count.saturating_add(1);
                                    } else {
                                        selected_count = selected_count.saturating_sub(1);
                                    }
                                }
                            });
                        }
                    });
                ui.weak(format!(
                    "{}: {selected_count}/{}",
                    t!("lan.adapters_selected"),
                    tractor_beam_core::MAX_SELECTED_LAN_ADAPTERS
                ));
                if let Some(message) = &self.join_code_message {
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(message).color(ui.visuals().error_fg_color),
                        )
                        .wrap(),
                    );
                }
                ui.add_space(6.0);
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            selected_count > 0 && self.mutations_enabled(),
                            egui::Button::new(t!("lan.create_confirm")),
                        )
                        .clicked()
                    {
                        create = true;
                    }
                    if ui.button(t!("join_code.cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if create {
            let (steam_id64, display_name) = self.current_identity();
            match steam_id64.parse::<u64>() {
                Ok(steam_id64) => {
                    let adapters = self
                        .lan_adapters
                        .iter()
                        .filter(|(_, selected)| *selected)
                        .map(|(adapter, _)| adapter.clone())
                        .collect();
                    if !self
                        .application
                        .create_lan_room(steam_id64, display_name, adapters)
                    {
                        self.show_busy_status();
                    }
                }
                Err(_) => self.join_code_message = Some(t!("lan.identity_required").into_owned()),
            }
        }
        if cancel {
            open = false;
            self.join_code_message = None;
        }
        self.lan_create_dialog_open = open;
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
                if self.lan_probe_results.len() > 1 {
                    ui.label(t!("lan.choose_endpoint"));
                    for (index, result) in self.lan_probe_results.iter().enumerate() {
                        ui.radio_value(
                            &mut self.selected_lan_probe,
                            Some(index),
                            format!("{} (via {})", result.endpoint, result.local_address.ip()),
                        );
                    }
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
        if confirm {
            if let (Some(invitation), Some(index)) =
                (self.pending_lan_invitation.clone(), self.selected_lan_probe)
                && let Some(result) = self.lan_probe_results.get(index)
            {
                self.join_lan_room(invitation, result.endpoint);
            } else if self.import_join_code() {
                open = false;
            }
        }
        if cancel {
            open = false;
            self.join_code_input.clear();
            self.join_code_message = None;
            self.lan_probe_results.clear();
            self.selected_lan_probe = None;
            self.pending_lan_invitation = None;
        }
        self.join_code_dialog_open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_create_dialog_stays_inside_the_viewport() {
        assert_eq!(lan_create_dialog_width(576.0), 400.0);
        assert_eq!(lan_create_dialog_width(360.0), 328.0);
        assert_eq!(lan_create_dialog_width(24.0), 1.0);
    }

    #[test]
    fn lan_create_dialog_closes_after_switching_to_relay() {
        assert!(should_show_lan_create_dialog(true, RouteChoice::LanDirect));
        assert!(!should_show_lan_create_dialog(
            true,
            RouteChoice::ExternalRelay
        ));
    }
}
