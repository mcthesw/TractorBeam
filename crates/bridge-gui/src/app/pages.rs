use basement_bridge_core::{LogLevel, SessionMode, SessionStatus, TransportChoice};
use eframe::egui::{self, ComboBox, TextEdit};

use crate::i18n::{Language, Text, text};

use super::{
    BridgeApp, Page,
    widgets::{account_label, detail_counters, selected_account_label},
};

impl BridgeApp {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let home = self.t(Text::Home);
        let diagnostics = self.t(Text::Diagnostics);
        let debug = self.t(Text::Debug);
        let selected_language = self.language.label();

        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.page, Page::Home, home);
                ui.selectable_value(&mut self.page, Page::Diagnostics, diagnostics);
                ui.selectable_value(&mut self.page, Page::Debug, debug);
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("🌐");
                ComboBox::from_id_salt("language")
                    .selected_text(selected_language)
                    .width(112.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.language,
                            Language::Chinese,
                            Language::Chinese.label(),
                        );
                        ui.selectable_value(
                            &mut self.language,
                            Language::English,
                            Language::English.label(),
                        );
                    });
            });
        });
    }

    pub(super) fn home_page(&mut self, ui: &mut egui::Ui) {
        self.connection_form(ui);
        ui.add_space(12.0);
        self.steam_identity_ui(ui);
        ui.add_space(12.0);
        self.action_row(ui);
    }

    pub(super) fn diagnostics_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t(Text::Diagnostics));
        ui.add_space(8.0);
        if ui.button(self.t(Text::OpenLogDirectory)).clicked() {
            self.open_log_directory();
        }
        if let Some(path) = &self.last_log_directory {
            ui.add_space(4.0);
            ui.label(self.t(Text::LogDirectory));
            ui.monospace(path);
        }
        ui.add_space(12.0);
        detail_counters(ui, self.language, self.client.state());
        ui.add_space(12.0);
        ui.heading(self.t(Text::Logs));
        ui.add_space(4.0);
        let logs = &self.client.state().logs;
        egui::ScrollArea::vertical()
            .id_salt("diagnostics_logs")
            .max_height(420.0)
            .auto_shrink([false, false])
            .show_rows(ui, 20.0, logs.len(), |ui, range| {
                for entry in &logs[range] {
                    ui.horizontal(|ui| {
                        ui.monospace(format!("[{}]", entry.timestamp));
                        ui.colored_label(log_level_color(ui, entry.level), entry.level.to_string());
                        ui.label(&entry.message);
                    });
                }
            });
    }

    pub(super) fn debug_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t(Text::Debug));
        ui.add_space(8.0);

        self.readiness_probe_ui(ui);
        ui.add_space(12.0);

        if ui.button(self.t(Text::RunHookReceiveProbe)).clicked() {
            self.run_hook_receive_probe();
        }
        if self.client.state().hook_probe_running {
            ui.add_space(4.0);
            ui.label(self.t(Text::ProbeRunning));
        }
        if let Some(result) = &self.client.state().latest_hook_receive_probe {
            ui.add_space(4.0);
            ui.label(self.t(Text::LastHookReceiveProbe));
            ui.monospace(result.to_string());
        }
    }

    fn connection_form(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t(Text::Home));
        ui.add_space(8.0);

        ui.label(self.t(Text::RelayServer));
        let mut selected_relay = self.selected_relay;
        ComboBox::from_id_salt("relay_preset")
            .selected_text(self.relay_selection_label())
            .width(360.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected_relay, None, self.t(Text::ManualRelay));
                for (index, relay) in self.relay_presets.iter().enumerate() {
                    ui.selectable_value(&mut selected_relay, Some(index), relay.label());
                }
            });
        if selected_relay != self.selected_relay {
            self.selected_relay = selected_relay;
            self.apply_selected_relay_defaults();
        }

        let manual_relay = self.selected_relay.is_none();
        ui.add_space(8.0);
        ui.label(self.t(Text::RelayHost));
        ui.add_enabled(
            manual_relay,
            TextEdit::singleline(&mut self.relay_host).desired_width(f32::INFINITY),
        );

        ui.add_space(8.0);
        ui.label(self.t(Text::RelayPort));
        ui.add_enabled(
            manual_relay,
            egui::DragValue::new(&mut self.relay_port).range(1..=u16::MAX),
        );

        ui.add_space(8.0);
        let udp = self.t(Text::Udp);
        let tcp = self.t(Text::Tcp);
        ui.label(self.t(Text::Transport));
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.preset_supports_transport(TransportChoice::Udp), |ui| {
                ui.radio_value(&mut self.transport, TransportChoice::Udp, udp);
            });
            ui.add_enabled_ui(self.preset_supports_transport(TransportChoice::Tcp), |ui| {
                ui.radio_value(&mut self.transport, TransportChoice::Tcp, tcp);
            });
        });
        if !self.preset_supports_transport(self.transport) {
            self.apply_selected_relay_defaults();
        }

        ui.add_space(8.0);
        ui.label(self.t(Text::Room));
        ui.add(TextEdit::singleline(&mut self.room).desired_width(f32::INFINITY));
        ui.add_space(8.0);

        let official = self.t(Text::Official);
        let fallback = self.t(Text::Fallback);
        let pure = self.t(Text::Pure);
        ui.label(self.t(Text::Mode));
        ui.vertical(|ui| {
            ui.radio_value(&mut self.mode, SessionMode::Official, official);
            ui.radio_value(&mut self.mode, SessionMode::Fallback, fallback);
            ui.radio_value(&mut self.mode, SessionMode::Pure, pure);
        });
    }

    fn steam_identity_ui(&mut self, ui: &mut egui::Ui) {
        let accounts = self.client.state().detected_accounts.clone();
        ui.heading(self.t(Text::SteamAccount));
        ui.add_space(8.0);
        if accounts.is_empty() {
            ui.label(self.t(Text::NoSteamAccounts));
        } else {
            ComboBox::from_id_salt("steam_account")
                .selected_text(selected_account_label(
                    self.selected_account,
                    &accounts,
                    self.language,
                ))
                .width(360.0)
                .show_ui(ui, |ui| {
                    for (index, account) in accounts.iter().enumerate() {
                        ui.selectable_value(
                            &mut self.selected_account,
                            Some(index),
                            account_label(account),
                        );
                    }
                    ui.selectable_value(
                        &mut self.selected_account,
                        None,
                        text(self.language, Text::Manual),
                    );
                });
        }
        ui.add_space(4.0);
        if ui.button(self.t(Text::RefreshAccounts)).clicked() {
            self.refresh_accounts();
        }

        if self.selected_account.is_none() {
            ui.add_space(8.0);
            ui.label(self.t(Text::ManualSteamId));
            ui.add(TextEdit::singleline(&mut self.manual_steam_id).desired_width(f32::INFINITY));
            ui.add_space(8.0);
            ui.label(self.t(Text::DisplayName));
            ui.add(
                TextEdit::singleline(&mut self.manual_display_name).desired_width(f32::INFINITY),
            );
        }
    }

    fn action_row(&mut self, ui: &mut egui::Ui) {
        let running = self.client.state().status == SessionStatus::Running;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!running, egui::Button::new(self.t(Text::Start)))
                .clicked()
            {
                self.start();
            }
            if ui
                .add_enabled(running, egui::Button::new(self.t(Text::Stop)))
                .clicked()
            {
                self.client.stop_session();
            }
        });
    }

    fn readiness_probe_ui(&mut self, ui: &mut egui::Ui) {
        let running = self.client.state().readiness_probe_running;
        if ui
            .add_enabled(!running, egui::Button::new(self.t(Text::RunReadinessProbe)))
            .clicked()
        {
            self.start_readiness_probe();
        }
        if running {
            ui.add_space(4.0);
            ui.label(self.t(Text::ProbeRunning));
        }
        if let Some(report) = &self.client.state().latest_readiness_probe {
            ui.add_space(4.0);
            let color = match report.outcome {
                basement_bridge_core::ReadinessProbeOutcome::Pass => {
                    ui.visuals().strong_text_color()
                }
                basement_bridge_core::ReadinessProbeOutcome::Warn => {
                    egui::Color32::from_rgb(185, 124, 0)
                }
                basement_bridge_core::ReadinessProbeOutcome::Fail => ui.visuals().error_fg_color,
            };
            ui.colored_label(color, report.short_summary());
        }
    }
}

fn log_level_color(ui: &egui::Ui, level: LogLevel) -> egui::Color32 {
    match level {
        LogLevel::Trace => ui.visuals().weak_text_color(),
        LogLevel::Debug => egui::Color32::from_rgb(95, 125, 155),
        LogLevel::Info => ui.visuals().text_color(),
        LogLevel::Warn => egui::Color32::from_rgb(185, 124, 0),
        LogLevel::Error => ui.visuals().error_fg_color,
    }
}
