use basement_bridge_core::{
    ClientError, ConfigError, SessionMode, SessionQuality, SessionStatus, SessionStopReason,
};
use eframe::egui;

use crate::i18n::{Language, Text, text};

use super::{BridgeApp, ConnectionProfile};

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
            if state.status == SessionStatus::Idle
                && let Some(reason) = &state.last_stop_reason
            {
                ui.monospace(stop_reason_label(self.language, reason));
            }
            ui.separator();
            ui.label(format!(
                "{}: {}",
                self.t(Text::Mode),
                mode_label(self.language, self.mode)
            ));
            ui.separator();
            ui.label(format!(
                "{}: {}",
                self.t(Text::ConnectionProfile),
                connection_profile_label(self.language, self.current_connection_profile())
            ));
            if self.connection_profile_pending() {
                ui.monospace(self.t(Text::ReconnectRequired));
            }
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
            if let Some(error) = &state.latest_hook_receive_probe_error {
                ui.separator();
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    hook_preflight_error_label(self.language),
                )
                .on_hover_text(error);
            }
            if let Some(error) = &self.last_error {
                ui.separator();
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    }
}

fn stop_reason_label(language: Language, reason: &SessionStopReason) -> String {
    match (language, reason) {
        (Language::Chinese, SessionStopReason::UserStopped) => "用户停止".to_owned(),
        (Language::Chinese, SessionStopReason::GameExited { .. }) => "游戏已关闭".to_owned(),
        (Language::Chinese, SessionStopReason::RuntimeEnded { .. }) => "会话已结束".to_owned(),
        (_, SessionStopReason::UserStopped) => "Stopped".to_owned(),
        (_, SessionStopReason::GameExited { .. }) => "Game closed".to_owned(),
        (_, SessionStopReason::RuntimeEnded { .. }) => "Session ended".to_owned(),
    }
}

fn hook_preflight_error_label(language: Language) -> &'static str {
    match language {
        Language::Chinese => "Hook 预检异常",
        Language::English => "Hook preflight issue",
    }
}

pub(super) fn connection_profile_label(
    language: Language,
    profile: ConnectionProfile,
) -> &'static str {
    match profile {
        ConnectionProfile::Tcp => text(language, Text::Tcp),
        ConnectionProfile::Udp => text(language, Text::Udp),
        ConnectionProfile::UdpFec => text(language, Text::UdpFecExperimental),
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

pub(super) fn quality_label(language: Language, quality: SessionQuality) -> &'static str {
    match (language, quality) {
        (Language::Chinese, SessionQuality::Unavailable) => "暂无数据",
        (Language::Chinese, SessionQuality::Good) => "良好",
        (Language::Chinese, SessionQuality::Watch) => "注意",
        (Language::Chinese, SessionQuality::Poor) => "较差",
        (_, SessionQuality::Unavailable) => "Unavailable",
        (_, SessionQuality::Good) => "Good",
        (_, SessionQuality::Watch) => "Watch",
        (_, SessionQuality::Poor) => "Poor",
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
