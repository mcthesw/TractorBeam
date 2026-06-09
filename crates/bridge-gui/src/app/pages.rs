use basement_bridge_core::{SessionMode, SessionStatus};
use eframe::egui::{self, ComboBox, TextEdit};

use crate::i18n::{Language, Text, text};

use super::{
    BridgeApp, Page,
    widgets::{account_label, detail_counters, selected_account_label, summary_counters},
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
        ui.add_space(16.0);
        summary_counters(ui, self.language, self.client.state());
    }

    pub(super) fn diagnostics_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t(Text::Diagnostics));
        ui.add_space(8.0);
        if ui.button(self.t(Text::ExportDiagnostics)).clicked() {
            self.export_diagnostics();
        }
        if let Some(path) = &self.last_export {
            ui.add_space(4.0);
            ui.label(self.t(Text::LastExport));
            ui.monospace(path);
        }
        ui.add_space(12.0);
        detail_counters(ui, self.language, self.client.state());
        ui.add_space(12.0);
        ui.heading(self.t(Text::Logs));
        ui.add_space(4.0);
        for entry in &self.client.state().logs {
            ui.monospace(format!(
                "[{}] {} {}",
                entry.timestamp, entry.level, entry.message
            ));
        }
    }

    pub(super) fn debug_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t(Text::Debug));
        ui.add_space(8.0);

        if ui.button(self.t(Text::RunHookReceiveProbe)).clicked() {
            self.run_hook_receive_probe();
        }
        if let Some(result) = &self.last_hook_probe {
            ui.add_space(4.0);
            ui.label(self.t(Text::LastHookReceiveProbe));
            ui.monospace(result);
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(self.t(Text::RelayProbePayloadBytes));
            ui.add(
                egui::DragValue::new(&mut self.relay_probe_payload_bytes)
                    .range(1..=60_000)
                    .speed(256),
            );
        });
        if ui.button(self.t(Text::RunRelayProbe)).clicked() {
            self.run_relay_probe();
        }
        if let Some(result) = &self.last_relay_probe {
            ui.add_space(4.0);
            ui.label(self.t(Text::LastRelayProbe));
            ui.monospace(result);
        }
    }

    fn connection_form(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t(Text::Home));
        ui.add_space(8.0);

        ui.label(self.t(Text::RelayHost));
        ui.add(TextEdit::singleline(&mut self.relay_host).desired_width(f32::INFINITY));

        ui.add_space(8.0);
        ui.label(self.t(Text::RelayPort));
        ui.add(egui::DragValue::new(&mut self.relay_port).range(1..=u16::MAX));

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
}
