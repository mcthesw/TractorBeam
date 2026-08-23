use eframe::egui::{self, ComboBox, TextEdit};
use rust_i18n::t;
use tractor_beam_core::{HookStartupPhase, RelayCatalogChange, SessionStatus, TransportChoice};

use crate::app::{RelayDialogMode, RelayDialogState};

use super::*;

impl BridgeApp {
    pub(super) fn relay_connection_ui(&mut self, ui: &mut egui::Ui) {
        if self.relay_settings_original.is_none() {
            let relay = self.selected_relay_preset().map_or_else(
                || self.relay_config().relay.to_string(),
                |preset| self.relay_option_label(preset),
            );
            let (steam_id, display_name) = self.current_identity();
            let account = if display_name.is_empty() {
                steam_id
            } else {
                display_name
            };
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.strong(t!("room.current_connection"));
                    if ui.button(t!("room.change_settings")).clicked() {
                        self.begin_relay_settings_edit();
                    }
                });
                ui.label(format!(
                    "{relay} · {} · {account}",
                    connection_profile_label(self.current_connection_profile())
                ));
            });
            return;
        }

        ui.group(|ui| {
            self.relay_section(ui, false);
            ui.add_space(8.0);
            self.transport_section(ui);
            ui.add_space(8.0);
            self.steam_section(ui, false);
            if self.client_state().status == SessionStatus::Running {
                ui.add_space(6.0);
                ui.weak(t!("room.switch_running_hint"));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(t!("join_code.cancel")).clicked() {
                    self.cancel_relay_settings_edit();
                }
                let hook_ready = self.client_state().hook_startup.phase == HookStartupPhase::Ready;
                let running = self.client_state().status == SessionStatus::Running;
                if ui
                    .add_enabled(!running || hook_ready, egui::Button::new(t!("room.switch")))
                    .clicked()
                {
                    self.apply_relay_settings();
                }
            });
        });
    }

    pub(super) fn relay_section(&mut self, ui: &mut egui::Ui, persist_immediately: bool) {
        let relay_label = t!("relay.server");
        let retest_label = t!("probe.test_latency");
        label_with_help(ui, relay_label, t!("help.relay_server"));
        let mut selected_relay = self.selected_relay.clone();
        let selected_text = selected_relay
            .as_deref()
            .and_then(|id| self.relay_presets.iter().find(|relay| relay.id == id))
            .map_or_else(
                || {
                    if self.relay_host.trim().is_empty() {
                        t!("relay.none").into_owned()
                    } else {
                        format!("{} ({})", t!("relay.one_time"), self.relay_config().relay)
                    }
                },
                |relay| self.relay_option_label(relay),
            );
        ComboBox::from_id_salt("home_relay")
            .selected_text(selected_text)
            .width(400.0)
            .show_ui(ui, |ui| {
                for relay in &self.relay_presets {
                    let label = self.relay_option_label(relay);
                    ui.selectable_value(&mut selected_relay, Some(relay.id.clone()), label);
                }
            });
        if selected_relay != self.selected_relay {
            self.selected_relay = selected_relay;
            self.apply_selected_relay_defaults();
            if persist_immediately {
                self.persist_selection();
            }
        }
        if persist_immediately {
            ui.add_space(4.0);
            let can_manage = self.relay_catalog_management_enabled();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_manage, egui::Button::new(t!("relay.add")))
                    .clicked()
                {
                    self.relay_dialog = Some(RelayDialogState::add());
                }
                if ui
                    .add_enabled(
                        can_manage && self.selected_relay_preset().is_some(),
                        egui::Button::new(t!("relay.edit")),
                    )
                    .clicked()
                    && let Some(relay) = self.selected_relay_preset().cloned()
                {
                    self.relay_dialog = Some(RelayDialogState::edit(&relay));
                }
                if ui.button(&*retest_label).clicked() {
                    self.test_relay_latency();
                }
            });
        } else if ui.button(retest_label).clicked() {
            self.test_relay_latency();
        }
    }

    pub(crate) fn relay_dialog(&mut self, context: &egui::Context) {
        let Some(mut dialog) = self.relay_dialog.take() else {
            return;
        };
        if self.application_snapshot.room_active()
            || self.client_state().status != SessionStatus::Idle
        {
            return;
        }

        let title = match &dialog.mode {
            RelayDialogMode::Add => t!("relay.add_title"),
            RelayDialogMode::Edit { .. } => t!("relay.edit_title"),
        };
        let mut open = true;
        let mut cancel = false;
        let mut save = false;
        let mut delete = false;
        let enabled = self.mutations_enabled();
        egui::Window::new(title)
            .collapsible(false)
            .default_width(360.0)
            .movable(true)
            .open(&mut open)
            .resizable(false)
            .show(context, |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    ui.label(t!("relay.name"));
                    ui.add(TextEdit::singleline(&mut dialog.name).desired_width(f32::INFINITY));
                    ui.add_space(6.0);
                    ui.label(t!("relay.host"));
                    ui.add(TextEdit::singleline(&mut dialog.host).desired_width(f32::INFINITY));
                    ui.add_space(6.0);
                    ui.label(t!("relay.port"));
                    ui.add(egui::DragValue::new(&mut dialog.port).range(1..=u16::MAX));
                    ui.add_space(6.0);
                    ui.label(t!("relay.transports"));
                    let tcp_before = dialog.supports_tcp;
                    let udp_before = dialog.supports_udp;
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut dialog.supports_tcp, t!("transport.tcp"));
                        ui.checkbox(&mut dialog.supports_udp, t!("transport.udp"));
                    });
                    reconcile_default_transport(&mut dialog, tcp_before, udp_before);
                    ui.add_space(6.0);
                    ui.label(t!("relay.default_transport"));
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(dialog.supports_tcp, |ui| {
                            ui.radio_value(
                                &mut dialog.default_transport,
                                TransportChoice::Tcp,
                                t!("transport.tcp"),
                            );
                        });
                        ui.add_enabled_ui(dialog.supports_udp, |ui| {
                            ui.radio_value(
                                &mut dialog.default_transport,
                                TransportChoice::Udp,
                                t!("transport.udp"),
                            );
                        });
                    });
                });

                if let Some(error) = &dialog.error {
                    ui.add_space(6.0);
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if matches!(&dialog.mode, RelayDialogMode::Edit { .. })
                        && ui
                            .add_enabled(enabled, egui::Button::new(t!("relay.delete")))
                            .clicked()
                    {
                        delete = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(enabled, egui::Button::new(t!("relay.save")))
                            .clicked()
                        {
                            save = true;
                        }
                        if ui.button(t!("relay.cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
            });

        if cancel || !open {
            return;
        }
        if delete {
            let RelayDialogMode::Edit { id } = &dialog.mode else {
                unreachable!("delete is only shown while editing");
            };
            if !self
                .application
                .save_relay_catalog(RelayCatalogChange::Delete { id: id.clone() })
            {
                self.show_busy_status();
            }
        } else if save {
            dialog.error = validate_relay_dialog(&dialog);
            if dialog.error.is_none() {
                let relay = dialog.relay_input();
                let change = match &dialog.mode {
                    RelayDialogMode::Add => RelayCatalogChange::Add(relay),
                    RelayDialogMode::Edit { id } => RelayCatalogChange::Update {
                        id: id.clone(),
                        relay,
                    },
                };
                if !self.application.save_relay_catalog(change) {
                    self.show_busy_status();
                }
            }
        }
        self.relay_dialog = Some(dialog);
    }

    fn relay_catalog_management_enabled(&self) -> bool {
        self.mutations_enabled()
            && !self.application_snapshot.room_active()
            && self.client_state().status == SessionStatus::Idle
    }

    fn transport_section(&mut self, ui: &mut egui::Ui) {
        label_with_help(ui, t!("connection_profile"), t!("help.connection_profile"));
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.preset_supports_transport(TransportChoice::Tcp), |ui| {
                ui.radio_value(
                    &mut self.transport,
                    TransportChoice::Tcp,
                    t!("transport.tcp"),
                );
            });
            ui.add_enabled_ui(self.preset_supports_transport(TransportChoice::Udp), |ui| {
                ui.radio_value(
                    &mut self.transport,
                    TransportChoice::Udp,
                    t!("transport.udp"),
                );
            });
        });
    }

    pub(super) fn steam_section(&mut self, ui: &mut egui::Ui, persist_immediately: bool) {
        let accounts = self.client_state().detected_accounts.clone();
        let steam_label = t!("steam.account");
        let refresh_label = t!("steam.refresh_accounts");
        let manual_steam_label = t!("steam.manual_id");
        let display_name_label = t!("display_name");
        label_with_help(ui, steam_label, t!("help.steam_account"));
        if accounts.is_empty() {
            ui.label(t!("steam.no_accounts"));
        } else {
            let account_before = self.selected_account;
            ComboBox::from_id_salt("home_steam_account")
                .selected_text(selected_account_label(self.selected_account, &accounts))
                .width(400.0)
                .show_ui(ui, |ui| {
                    for (index, account) in accounts.iter().enumerate() {
                        ui.selectable_value(
                            &mut self.selected_account,
                            Some(index),
                            account_label(account),
                        );
                    }
                    ui.selectable_value(&mut self.selected_account, None, t!("manual"));
                });
            if self.selected_account != account_before && persist_immediately {
                self.persist_selection();
            }
        }
        ui.add_space(2.0);
        if ui.button(refresh_label).clicked() {
            self.refresh_accounts();
            if persist_immediately {
                self.persist_selection();
            }
        }
        if self.selected_account.is_none() {
            ui.add_space(4.0);
            ui.add(
                TextEdit::singleline(&mut self.manual_steam_id)
                    .hint_text(manual_steam_label)
                    .desired_width(400.0),
            );
            ui.add_space(2.0);
            ui.add(
                TextEdit::singleline(&mut self.manual_display_name)
                    .hint_text(display_name_label)
                    .desired_width(400.0),
            );
        }
    }
}

fn validate_relay_dialog(dialog: &RelayDialogState) -> Option<String> {
    if dialog.name.trim().is_empty() {
        return Some(t!("relay.error_name_required").into_owned());
    }
    if dialog.host.trim().is_empty() {
        return Some(t!("relay.error_host_required").into_owned());
    }
    if !dialog.supports_tcp && !dialog.supports_udp {
        return Some(t!("relay.error_transport_required").into_owned());
    }
    let default_supported = match dialog.default_transport {
        TransportChoice::Tcp => dialog.supports_tcp,
        TransportChoice::Udp => dialog.supports_udp,
    };
    (!default_supported).then(|| t!("relay.error_default_transport").into_owned())
}

fn reconcile_default_transport(dialog: &mut RelayDialogState, tcp_before: bool, udp_before: bool) {
    if tcp_before
        && !dialog.supports_tcp
        && dialog.default_transport == TransportChoice::Tcp
        && dialog.supports_udp
    {
        dialog.default_transport = TransportChoice::Udp;
    }
    if udp_before
        && !dialog.supports_udp
        && dialog.default_transport == TransportChoice::Udp
        && dialog.supports_tcp
    {
        dialog.default_transport = TransportChoice::Tcp;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_dialog_starts_with_both_transports_and_tcp_default() {
        let dialog = RelayDialogState::add();

        assert!(dialog.supports_tcp);
        assert!(dialog.supports_udp);
        assert_eq!(dialog.default_transport, TransportChoice::Tcp);
    }

    #[test]
    fn disabling_the_default_transport_switches_to_the_remaining_transport() {
        let mut dialog = RelayDialogState::add();
        dialog.supports_tcp = false;

        reconcile_default_transport(&mut dialog, true, true);

        assert_eq!(dialog.default_transport, TransportChoice::Udp);
    }

    #[test]
    fn enabling_tcp_does_not_override_an_explicit_udp_default() {
        let mut dialog = RelayDialogState::add();
        dialog.supports_tcp = false;
        dialog.default_transport = TransportChoice::Udp;
        dialog.supports_tcp = true;

        reconcile_default_transport(&mut dialog, false, true);

        assert_eq!(dialog.default_transport, TransportChoice::Udp);
    }

    #[test]
    fn dialog_requires_at_least_one_transport() {
        let mut dialog = RelayDialogState::add();
        dialog.name = "Relay".to_owned();
        dialog.host = "relay.example.test".to_owned();
        dialog.supports_tcp = false;
        dialog.supports_udp = false;

        assert!(validate_relay_dialog(&dialog).is_some());
    }
}
