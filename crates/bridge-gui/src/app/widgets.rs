use std::borrow::Cow;

use eframe::egui;
use rust_i18n::t;
use tractor_beam_core::{RuntimeState, SteamIdentity};

const HELP_TOOLTIP_MAX_WIDTH: f32 = 320.0;

pub(super) fn help_icon(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) {
    let text = text.into();
    let response = ui.add_sized(
        [18.0, 18.0],
        egui::Label::new(
            egui::RichText::new("?")
                .small()
                .color(ui.visuals().weak_text_color()),
        )
        .sense(egui::Sense::hover()),
    );
    if response.contains_pointer() {
        response.show_tooltip_ui(|ui| {
            ui.set_max_width(HELP_TOOLTIP_MAX_WIDTH);
            ui.add(egui::Label::new(text).wrap());
        });
    }
}

pub(super) fn label_with_help(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    help: impl Into<egui::WidgetText>,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        help_icon(ui, help);
    });
}

pub(super) fn detail_counters(ui: &mut egui::Ui, state: &RuntimeState) {
    ui.horizontal(|ui| {
        ui.heading(t!("counters"));
        help_icon(ui, t!("help.counters"));
    });
    ui.add_space(6.0);
    counter_grid(
        ui,
        "detail_counters",
        &[
            (t!("counters.hook_to_relay"), state.counters.hook_to_relay),
            (t!("counters.relay_to_hook"), state.counters.relay_to_hook),
            (t!("counters.sent_bytes"), state.counters.sent_bytes),
            (t!("counters.received_bytes"), state.counters.received_bytes),
            (t!("errors"), state.counters.errors),
            (
                t!("counters.reconnect_dropped"),
                state.counters.reconnect_dropped_packets,
            ),
        ],
    );
}

pub(super) fn selected_account_label(
    selected_account: Option<usize>,
    accounts: &[SteamIdentity],
) -> String {
    selected_account
        .and_then(|index| accounts.get(index))
        .map_or_else(|| t!("manual").into_owned(), account_label)
}

pub(super) fn account_label(account: &SteamIdentity) -> String {
    format!("{} ({})", account.display_name, account.steam_id64)
}

fn counter_grid(ui: &mut egui::Ui, id: &'static str, counters: &[(Cow<'static, str>, u64)]) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            for (label, value) in counters {
                ui.label(label.as_ref());
                ui.monospace(value.to_string());
                ui.end_row();
            }
        });
}
