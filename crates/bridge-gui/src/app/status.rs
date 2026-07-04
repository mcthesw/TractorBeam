use std::borrow::Cow;

use basement_bridge_core::{
    ClientError, ConfigError, ConnectionProfile, SessionQuality, SessionStatus, SessionStopReason,
};
use eframe::egui;

use crate::i18n::{Language, Text, text};

use super::BridgeApp;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum StatusMessage {
    ConfigWarning,
    ConfigError(ConfigError),
    Text(String),
}

impl StatusMessage {
    pub(super) fn from_client_error(error: &ClientError) -> Self {
        match error {
            ClientError::Config(error) => Self::ConfigError(*error),
            ClientError::Io(_) => Self::Text(error.to_string()),
        }
    }

    fn localized_text(&self, language: Language) -> Cow<'_, str> {
        match self {
            Self::ConfigWarning => text(language, Text::ConfigWarning),
            Self::ConfigError(error) => Cow::Owned(config_error_message(language, *error)),
            Self::Text(message) => Cow::Borrowed(message),
        }
    }
}

impl BridgeApp {
    pub(super) fn start_error_dialog(&mut self, context: &egui::Context) {
        if !self.start_error_dialog_open {
            return;
        }
        let Some(message) = self.status_message.as_ref() else {
            self.start_error_dialog_open = false;
            return;
        };
        let error = message.localized_text(self.language);

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
                ui.add(egui::Label::new(error.as_ref()).wrap());
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
            if state.status == SessionStatus::Idle
                && let Some(reason) = &state.last_stop_reason
            {
                ui.monospace(stop_reason_label(self.language, reason));
            }
            ui.separator();
            ui.monospace(format!(
                "{} {}",
                self.t(Text::Errors),
                state.counters.errors
            ));
            if let Some(health) = &state.latest_session_health {
                ui.separator();
                ui.label(format!(
                    "{}: {}",
                    self.t(Text::SessionQuality),
                    quality_label(self.language, health.quality)
                ));
                if let Some(p95) = health.runtime_rtt.latency.p95_ms {
                    ui.monospace(format!("RTT p95 {p95} ms"));
                }
            }
            if let Some(message) = &self.status_message {
                ui.separator();
                let text = message.localized_text(self.language);
                ui.colored_label(ui.visuals().error_fg_color, text.as_ref());
            }
        });
    }
}

fn stop_reason_label(language: Language, reason: &SessionStopReason) -> Cow<'static, str> {
    match reason {
        SessionStopReason::UserStopped => text(language, Text::StopReasonUserStopped),
        SessionStopReason::GameExited { .. } => text(language, Text::StopReasonGameExited),
        SessionStopReason::RuntimeEnded { .. } => text(language, Text::StopReasonRuntimeEnded),
    }
}

pub(super) fn connection_profile_label(
    language: Language,
    profile: ConnectionProfile,
) -> Cow<'static, str> {
    match profile {
        ConnectionProfile::Tcp => text(language, Text::Tcp),
        ConnectionProfile::Udp => text(language, Text::Udp),
    }
}

pub(super) fn status_label(language: Language, status: SessionStatus) -> Cow<'static, str> {
    match status {
        SessionStatus::Idle => text(language, Text::Idle),
        SessionStatus::Running => text(language, Text::Running),
    }
}

pub(super) fn quality_label(language: Language, quality: SessionQuality) -> Cow<'static, str> {
    match quality {
        SessionQuality::Unavailable => text(language, Text::QualityUnavailable),
        SessionQuality::Good => text(language, Text::QualityGood),
        SessionQuality::Watch => text(language, Text::QualityWatch),
        SessionQuality::Poor => text(language, Text::QualityPoor),
    }
}

fn config_error_message(language: Language, config_error: ConfigError) -> String {
    let message = match config_error {
        ConfigError::MissingRelayHost => text(language, Text::ConfigMissingRelayHost),
        ConfigError::InvalidRelayPort => text(language, Text::ConfigInvalidRelayPort),
        ConfigError::MissingRoom => text(language, Text::ConfigMissingRoom),
        ConfigError::MissingSteamId => text(language, Text::ConfigMissingSteamId),
        ConfigError::InvalidSteamId => text(language, Text::ConfigInvalidSteamId),
        ConfigError::InvalidSessionHealth => text(language, Text::ConfigInvalidSessionHealth),
    };
    format!("{}: {message}", text(language, Text::ConfigError))
}

#[cfg(test)]
mod tests {
    use basement_bridge_core::ConfigError;

    use crate::i18n::Language;

    use super::StatusMessage;

    #[test]
    fn status_config_warning_tracks_selected_language() {
        let message = StatusMessage::ConfigWarning;

        assert_eq!(
            message.localized_text(Language::Chinese),
            "配置文件有误，已使用可手动修改的默认值。"
        );
        assert_eq!(
            message.localized_text(Language::English),
            "Config not applied; defaults loaded."
        );
    }

    #[test]
    fn config_error_message_tracks_selected_language() {
        let message = StatusMessage::ConfigError(ConfigError::MissingRoom);

        assert_eq!(
            message.localized_text(Language::Chinese),
            "配置错误: 需要填写房间"
        );
        assert_eq!(
            message.localized_text(Language::English),
            "Configuration error: Room is required"
        );
    }

    #[test]
    fn raw_status_message_remains_untranslated() {
        let message = StatusMessage::Text("failed to open log directory".to_owned());

        assert_eq!(
            message.localized_text(Language::Chinese),
            "failed to open log directory"
        );
        assert_eq!(
            message.localized_text(Language::English),
            "failed to open log directory"
        );
    }
}
