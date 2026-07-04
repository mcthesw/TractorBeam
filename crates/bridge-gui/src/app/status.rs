use std::borrow::Cow;

use basement_bridge_core::{
    ClientError, ConfigError, ConnectionProfile, SessionQuality, SessionStatus, SessionStopReason,
};
use eframe::egui;
use rust_i18n::t;

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

    fn localized_text(&self) -> Cow<'_, str> {
        match self {
            Self::ConfigWarning => t!("config.warning"),
            Self::ConfigError(error) => Cow::Owned(config_error_message(*error)),
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
        let error = message.localized_text();

        let mut open = true;
        let mut close_requested = false;
        egui::Window::new(t!("start.failed"))
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
                if ui.button(t!("close")).clicked() {
                    close_requested = true;
                }
            });

        self.start_error_dialog_open = open && !close_requested;
    }

    pub(super) fn status_bar(&self, ui: &mut egui::Ui) {
        let state = self.client.state();
        ui.horizontal(|ui| {
            ui.label(format!("{}: {}", t!("status"), status_label(state.status)));
            if state.status == SessionStatus::Idle
                && let Some(reason) = &state.last_stop_reason
            {
                ui.monospace(stop_reason_label(reason));
            }
            ui.separator();
            ui.monospace(format!("{} {}", t!("errors"), state.counters.errors));
            if let Some(health) = &state.latest_session_health {
                ui.separator();
                ui.label(format!(
                    "{}: {}",
                    t!("session_quality"),
                    quality_label(health.quality)
                ));
                if let Some(p95) = health.runtime_rtt.latency.p95_ms {
                    ui.monospace(format!("RTT p95 {p95} ms"));
                }
            }
            if let Some(message) = &self.status_message {
                ui.separator();
                let text = message.localized_text();
                ui.colored_label(ui.visuals().error_fg_color, text.as_ref());
            }
        });
    }
}

fn stop_reason_label(reason: &SessionStopReason) -> Cow<'static, str> {
    match reason {
        SessionStopReason::UserStopped => t!("stop_reason.user_stopped"),
        SessionStopReason::GameExited { .. } => t!("stop_reason.game_exited"),
        SessionStopReason::RuntimeEnded { .. } => t!("stop_reason.runtime_ended"),
    }
}

pub(super) fn connection_profile_label(profile: ConnectionProfile) -> Cow<'static, str> {
    match profile {
        ConnectionProfile::Tcp => t!("transport.tcp"),
        ConnectionProfile::Udp => t!("transport.udp"),
    }
}

pub(super) fn status_label(status: SessionStatus) -> Cow<'static, str> {
    match status {
        SessionStatus::Idle => t!("status.idle"),
        SessionStatus::Running => t!("status.running"),
    }
}

pub(super) fn quality_label(quality: SessionQuality) -> Cow<'static, str> {
    match quality {
        SessionQuality::Unavailable => t!("quality.unavailable"),
        SessionQuality::Good => t!("quality.good"),
        SessionQuality::Watch => t!("quality.watch"),
        SessionQuality::Poor => t!("quality.poor"),
    }
}

fn config_error_message(config_error: ConfigError) -> String {
    let message = match config_error {
        ConfigError::MissingRelayHost => t!("config.missing_relay_host"),
        ConfigError::InvalidRelayPort => t!("config.invalid_relay_port"),
        ConfigError::MissingRoom => t!("config.missing_room"),
        ConfigError::MissingAdmission => t!("config.missing_admission"),
        ConfigError::MissingSteamId => t!("config.missing_steam_id"),
        ConfigError::InvalidSteamId => t!("config.invalid_steam_id"),
        ConfigError::InvalidSessionHealth => t!("config.invalid_session_health"),
    };
    format!("{}: {message}", t!("config.error"))
}

#[cfg(test)]
mod tests {
    use basement_bridge_core::ConfigError;

    use crate::i18n::{Language, set_language, with_locale_lock};

    use super::StatusMessage;

    #[test]
    fn status_config_warning_tracks_selected_language() {
        with_locale_lock(|| {
            let message = StatusMessage::ConfigWarning;

            set_language(Language::Chinese);
            assert_eq!(
                message.localized_text(),
                "配置文件有误，已使用可手动修改的默认值。"
            );
            set_language(Language::English);
            assert_eq!(
                message.localized_text(),
                "Config not applied; defaults loaded."
            );
        });
    }

    #[test]
    fn config_error_message_tracks_selected_language() {
        with_locale_lock(|| {
            let message = StatusMessage::ConfigError(ConfigError::MissingRoom);

            set_language(Language::Chinese);
            assert_eq!(message.localized_text(), "配置错误: 需要填写房间");
            set_language(Language::English);
            assert_eq!(
                message.localized_text(),
                "Configuration error: Room is required"
            );
        });
    }

    #[test]
    fn raw_status_message_remains_untranslated() {
        with_locale_lock(|| {
            let message = StatusMessage::Text("failed to open log directory".to_owned());

            set_language(Language::Chinese);
            assert_eq!(message.localized_text(), "failed to open log directory");
            set_language(Language::English);
            assert_eq!(message.localized_text(), "failed to open log directory");
        });
    }
}
