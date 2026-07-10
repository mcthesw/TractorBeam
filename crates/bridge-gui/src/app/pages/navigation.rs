use eframe::egui::{self, ComboBox};
use rust_i18n::t;

use crate::{
    app::{BridgeApp, Page},
    i18n::Language,
};

impl BridgeApp {
    pub(in crate::app) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let selected_language_label = self.language.label();
        let mut selected_language = self.language;
        let home = t!("home");
        let settings = t!("settings");
        let stats = t!("stats");
        let log = t!("log");
        let about = t!("about");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.page, Page::Home, home);
                ui.selectable_value(&mut self.page, Page::Settings, settings);
                ui.selectable_value(&mut self.page, Page::Stats, stats);
                ui.selectable_value(&mut self.page, Page::Log, log);
                ui.selectable_value(&mut self.page, Page::About, about);
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("🌐");
                ComboBox::from_id_salt("language")
                    .selected_text(selected_language_label)
                    .width(112.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut selected_language,
                            Language::Chinese,
                            Language::Chinese.label(),
                        );
                        ui.selectable_value(
                            &mut selected_language,
                            Language::English,
                            Language::English.label(),
                        );
                    });
                self.set_language(selected_language);
            });
        });
    }
}
