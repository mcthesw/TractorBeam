use basement_bridge_core::{ClientError, ConfigError, SessionMode, SessionStatus};
use eframe::egui;

use crate::i18n::{Language, Text, text};

use super::BridgeApp;

impl BridgeApp {
    pub(super) fn start_error_dialog(&mut self, context: &egui::Context) {
        if !self.start_error_dialog_open {
            return;
        }
        let Some(error) = self.last_error.clone() else {
            self.start_error_dialog_open = false;
            return;
        };

        let mut open = true;
        let mut close_requested = false;
        egui::Window::new(self.t(Text::StartFailed))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .default_width(340.0)
            .open(&mut open)
            .resizable(false)
            .show(context, |ui| {
                ui.set_min_width(280.0);
                ui.set_max_width(420.0);
                ui.add(egui::Label::new(error).wrap());
                ui.add_space(8.0);
                if ui.button(self.t(Text::Close)).clicked() {
                    close_requested = true;
                }
            });

        self.start_error_dialog_open = open && !close_requested;
    }

    pub(super) fn status_bar(&self, ui: &mut egui::Ui) {
        let state = self.client.state();
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: {}",
                self.t(Text::Status),
                status_label(self.language, state.status)
            ));
            ui.separator();
            ui.label(format!(
                "{}: {}",
                self.t(Text::Mode),
                mode_label(self.language, self.mode)
            ));
            ui.separator();
            ui.monospace(format!(
                "{} {}",
                self.t(Text::HookToRelay),
                state.counters.hook_to_relay
            ));
            ui.monospace(format!(
                "{} {}",
                self.t(Text::RelayToHook),
                state.counters.relay_to_hook
            ));
            ui.monospace(format!(
                "{} {}",
                self.t(Text::Errors),
                state.counters.errors
            ));
            if let Some(error) = &self.last_error {
                ui.separator();
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    }
}

pub(super) fn status_label(language: Language, status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => text(language, Text::Idle),
        SessionStatus::Running => text(language, Text::Running),
    }
}

pub(super) fn mode_label(language: Language, mode: SessionMode) -> &'static str {
    match mode {
        SessionMode::Official => text(language, Text::Official),
        SessionMode::Fallback => text(language, Text::Fallback),
        SessionMode::Pure => text(language, Text::Pure),
    }
}

pub(super) fn error_message(language: Language, error: &ClientError) -> String {
    let ClientError::Config(config_error) = error else {
        return error.to_string();
    };
    let message = match (language, *config_error) {
        (Language::Chinese, ConfigError::MissingRelayHost) => "需要填写 Relay 地址",
        (Language::Chinese, ConfigError::InvalidRelayPort) => "Relay 端口无效",
        (Language::Chinese, ConfigError::MissingRoom) => "需要填写房间",
        (Language::Chinese, ConfigError::MissingSteamId) => "需要填写 SteamID64",
        (Language::Chinese, ConfigError::InvalidSteamId) => "SteamID64 只能包含数字",
        (_, other) => return format!("{}: {other}", text(language, Text::ConfigError)),
    };
    format!("{}: {message}", text(language, Text::ConfigError))
}
