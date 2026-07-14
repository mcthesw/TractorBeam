use std::borrow::Cow;

use eframe::egui;
use rust_i18n::t;
use tractor_beam_core::{
    ClientError, ConfigError, ConnectionProfile, InputDelayError, RelayLinkState, SessionQuality,
    SessionStatus, SessionStopReason,
};

use super::{ApplicationOperation, BootstrapState, BridgeApp};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum StatusMessage {
    Busy,
    DiagnosticsExported,
    DiagnosticsExportFailed,
    LogOpenFailed,
    SelectionSaveFailed,
    ConfigWarning,
    ConfigError(ConfigError),
    Text(String),
}

impl StatusMessage {
    pub(super) fn from_client_error(error: &ClientError) -> Self {
        match error {
            ClientError::Config(error) => Self::ConfigError(*error),
            ClientError::Io(_) => Self::Text(error.to_string()),
            ClientError::InputDelay(error) => Self::Text(input_delay_error_label(error)),
        }
    }

    fn localized_text(&self) -> Cow<'_, str> {
        match self {
            Self::Busy => t!("operation.busy"),
            Self::DiagnosticsExported => t!("diagnostics.exported"),
            Self::DiagnosticsExportFailed => t!("diagnostics.export_failed"),
            Self::LogOpenFailed => t!("logs.open_failed"),
            Self::SelectionSaveFailed => t!("config.selection_save_failed"),
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
        let state = self.client_state();
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: {}",
                t!("status"),
                self.application_status_label()
            ));
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
                    quality_label(state.smoothness.level)
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

    fn application_status_label(&self) -> Cow<'static, str> {
        if let Some(operation) = self.application_snapshot.operation {
            return match operation {
                ApplicationOperation::Starting => t!("status.starting"),
                ApplicationOperation::Stopping => t!("status.stopping"),
                ApplicationOperation::ShuttingDown => t!("status.shutting_down"),
                ApplicationOperation::RefreshingAccounts
                | ApplicationOperation::Probing
                | ApplicationOperation::ReadingInputDelay
                | ApplicationOperation::WritingInputDelay
                | ApplicationOperation::OpeningLogs
                | ApplicationOperation::ExportingTroubleshootingPackage
                | ApplicationOperation::ReadingClipboard => t!("status.working"),
            };
        }
        match &self.client_state().relay_link {
            RelayLinkState::Reconnecting {
                attempt,
                elapsed_ms,
                ..
            } => {
                return Cow::Owned(format!(
                    "{} · {} #{} · {}s",
                    t!("status.reconnecting"),
                    t!("status.attempt"),
                    attempt,
                    elapsed_ms / 1_000
                ));
            }
            RelayLinkState::Recovered {
                outage_ms,
                full_join,
                ..
            } => {
                return Cow::Owned(format!(
                    "{} · {} ms",
                    if *full_join {
                        t!("status.rejoined")
                    } else {
                        t!("status.resumed")
                    },
                    outage_ms
                ));
            }
            RelayLinkState::RecoveryExhausted { .. } => {
                return t!("status.recovery_exhausted");
            }
            RelayLinkState::Inactive | RelayLinkState::Connected => {}
        }
        match self.application_snapshot.bootstrap {
            BootstrapState::Initializing => t!("status.initializing"),
            BootstrapState::Failed => t!("status.initialization_failed"),
            BootstrapState::Ready => status_label(self.client_state().status),
        }
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

pub(super) fn smoothness_summary(quality: SessionQuality) -> Cow<'static, str> {
    match quality {
        SessionQuality::Unavailable => t!("quality.summary.collecting"),
        SessionQuality::Good => t!("quality.summary.good"),
        SessionQuality::Watch => t!("quality.summary.watch"),
        SessionQuality::Poor => t!("quality.summary.poor"),
    }
}

fn config_error_message(config_error: ConfigError) -> String {
    let message = match config_error {
        ConfigError::MissingRelayHost => t!("config.missing_relay_host"),
        ConfigError::InvalidRelayPort => t!("config.invalid_relay_port"),
        ConfigError::MissingSteamId => t!("config.missing_steam_id"),
        ConfigError::InvalidSteamId => t!("config.invalid_steam_id"),
        ConfigError::InvalidSessionHealth => t!("config.invalid_session_health"),
    };
    format!("{}: {message}", t!("config.error"))
}

pub(super) fn input_delay_error_label(error: &InputDelayError) -> String {
    match error {
        InputDelayError::SessionNotRunning => t!("session.not_started").into_owned(),
        InputDelayError::UnsupportedMode => t!("input_delay.unsupported_mode").into_owned(),
        InputDelayError::HookNotReady => t!("input_delay.not_ready").into_owned(),
        InputDelayError::Hook(error) if error.as_str() == "target_not_found" => {
            t!("input_delay.target_not_found").into_owned()
        }
        InputDelayError::Hook(_) | InputDelayError::Io(_) => {
            format!("{}: {error}", t!("input_delay.failed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use tractor_beam_core::ConfigError;

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
            let message = StatusMessage::ConfigError(ConfigError::MissingSteamId);

            set_language(Language::Chinese);
            assert_eq!(message.localized_text(), "配置错误: 需要填写 SteamID64");
            set_language(Language::English);
            assert_eq!(
                message.localized_text(),
                "Configuration error: SteamID64 is required"
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
