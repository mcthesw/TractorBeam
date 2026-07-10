use eframe::egui;
use rust_i18n::t;
use tractor_beam_core::build_info;

use crate::app::BridgeApp;

const PROTOCOL_VERSION: &str = "2.0";

impl BridgeApp {
    pub(in crate::app) fn about_page(&mut self, ui: &mut egui::Ui) {
        let about_label = t!("about");
        let desc_label = t!("about.desc");
        let version_label = t!("version");
        let proto_label = t!("about.protocol_version");
        ui.heading(about_label);
        ui.add_space(12.0);
        ui.label(desc_label);
        ui.add_space(16.0);
        egui::Grid::new("about_grid")
            .num_columns(2)
            .spacing([20.0, 6.0])
            .show(ui, |ui| {
                ui.label(version_label);
                ui.monospace(build_info::version_label());
                ui.end_row();
                ui.label(proto_label);
                ui.monospace(PROTOCOL_VERSION);
                ui.end_row();
            });
        ui.add_space(12.0);
        ui.hyperlink_to("GitHub", "https://github.com/mcthesw/TractorBeam");
        ui.add_space(2.0);
        ui.label(format!("{}: GNU AGPL-3.0-or-later", t!("license")));
    }
}
