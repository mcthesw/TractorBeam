use std::borrow::Cow;

use chrono::{Local, TimeZone as _};
use eframe::egui::{self, ComboBox};
use rust_i18n::t;
use tractor_beam_core::LogLevel;

use crate::app::BridgeApp;

const LOG_LEVELS: [LogLevel; 5] = [
    LogLevel::Trace,
    LogLevel::Debug,
    LogLevel::Info,
    LogLevel::Warn,
    LogLevel::Error,
];

impl BridgeApp {
    pub(in crate::app) fn log_page(&mut self, ui: &mut egui::Ui) {
        let log_label = t!("log");
        let clear_label = t!("logs.clear");
        let level_filter_label = t!("logs.level_filter");
        let empty_label = t!("logs.empty");
        let open_log_label = t!("logs.open_directory");
        let export_diagnostics_label = t!("diagnostics.export");
        ui.heading(log_label);
        ui.add_space(4.0);

        let logs = self.client_state().logs.clone();
        let max_level = ui.data_mut(|data| {
            data.get_persisted::<u8>("log_level_filter".into())
                .unwrap_or(LogLevel::Info as u8)
        });
        let mut selected_level = max_level;
        ui.horizontal(|ui| {
            ui.label(level_filter_label);
            ComboBox::from_id_salt("log_level_filter")
                .selected_text(level_name(u8_to_level(max_level)))
                .width(100.0)
                .show_ui(ui, |ui| {
                    for level in LOG_LEVELS {
                        ui.selectable_value(&mut selected_level, level as u8, level_name(level));
                    }
                });
            if ui
                .add_enabled(self.mutations_enabled(), egui::Button::new(clear_label))
                .clicked()
            {
                self.clear_logs();
            }
            if ui
                .add_enabled(self.mutations_enabled(), egui::Button::new(open_log_label))
                .clicked()
            {
                self.open_log_directory();
            }
            if ui
                .add_enabled(
                    self.mutations_enabled(),
                    egui::Button::new(export_diagnostics_label),
                )
                .clicked()
            {
                self.export_diagnostics_bundle();
            }
            if self.last_diagnostics_bundle.is_some()
                && ui.button(t!("diagnostics.reveal")).clicked()
            {
                self.reveal_diagnostics_bundle();
            }
        });
        ui.data_mut(|data| {
            data.insert_persisted("log_level_filter".into(), selected_level);
        });
        let filter_level = u8_to_level(selected_level);
        ui.separator();
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .id_salt("log_scroll")
            .auto_shrink([false, true])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if logs.is_empty() {
                    ui.label(empty_label);
                    return;
                }
                ui.set_min_width(ui.available_width());
                for entry in &logs {
                    if (entry.level as u8) < (filter_level as u8) {
                        continue;
                    }
                    ui.horizontal_top(|ui| {
                        ui.monospace(format!("[{}]", format_log_timestamp(entry.timestamp_ms)));
                        ui.colored_label(log_level_color(ui, entry.level), entry.level.to_string());
                        ui.add(egui::Label::new(entry.message.as_str()).wrap());
                    });
                    ui.add_space(1.0);
                }
            });
    }
}

fn format_log_timestamp(timestamp_ms: u64) -> String {
    let timestamp_ms = i64::try_from(timestamp_ms).unwrap_or(i64::MAX);
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map_or_else(
            || "0000-00-00 00:00:00.000".to_owned(),
            |timestamp| timestamp.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        )
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

fn u8_to_level(value: u8) -> LogLevel {
    match value {
        0 => LogLevel::Trace,
        1 => LogLevel::Debug,
        2 => LogLevel::Info,
        3 => LogLevel::Warn,
        _ => LogLevel::Error,
    }
}

fn level_name(level: LogLevel) -> Cow<'static, str> {
    match level {
        LogLevel::Error => t!("log_level.error"),
        LogLevel::Warn => t!("log_level.warn"),
        LogLevel::Info => t!("log_level.info"),
        LogLevel::Debug => t!("log_level.debug"),
        LogLevel::Trace => t!("log_level.trace"),
    }
}

#[cfg(test)]
mod tests {
    use super::format_log_timestamp;

    #[test]
    fn local_log_timestamp_has_stable_millisecond_shape() {
        let formatted = format_log_timestamp(1_767_225_600_123);
        assert_eq!(formatted.len(), 23);
        assert_eq!(&formatted[4..5], "-");
        assert_eq!(&formatted[7..8], "-");
        assert_eq!(&formatted[10..11], " ");
        assert_eq!(&formatted[13..14], ":");
        assert_eq!(&formatted[16..17], ":");
        assert_eq!(&formatted[19..20], ".");
        assert!(formatted.ends_with(".123"));
    }

    #[test]
    fn timestamps_within_one_second_remain_distinct() {
        assert_ne!(
            format_log_timestamp(1_767_225_600_001),
            format_log_timestamp(1_767_225_600_999)
        );
    }
}
