use std::borrow::Cow;

use eframe::egui::{self, ComboBox, TextEdit};
use rust_i18n::t;
use tractor_beam_core::{HookStartupPhase, SessionStatus, TransportChoice};

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
        let manual_label = t!("relay.manual");
        let retest_label = t!("probe.test_latency");
        let host_label = t!("relay.host");
        label_with_help(ui, relay_label, t!("help.relay_server"));
        let mut selected_relay = self.selected_relay;
        let selected_text = selected_relay
            .and_then(|index| self.relay_presets.get(index))
            .map_or_else(
                || manual_label.clone(),
                |relay| Cow::Owned(self.relay_option_label(relay)),
            );
        ComboBox::from_id_salt("home_relay")
            .selected_text(selected_text)
            .width(400.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected_relay, None, manual_label);
                for (index, relay) in self.relay_presets.iter().enumerate() {
                    let label = self.relay_option_label(relay);
                    ui.selectable_value(&mut selected_relay, Some(index), label);
                }
            });
        if selected_relay != self.selected_relay {
            self.selected_relay = selected_relay;
            self.apply_selected_relay_defaults();
            if persist_immediately {
                self.persist_selection();
            }
        }
        if self.selected_relay.is_none() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(
                    TextEdit::singleline(&mut self.relay_host)
                        .hint_text(host_label)
                        .desired_width(310.0),
                );
                ui.add(egui::DragValue::new(&mut self.relay_port).range(1..=u16::MAX));
            });
        }
        if ui.button(retest_label).clicked() {
            self.test_relay_latency();
        }
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
