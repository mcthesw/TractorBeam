use basement_bridge_core::{RuntimeState, SteamIdentity};
use eframe::egui;

use crate::i18n::{Language, Text, text};

pub(super) fn detail_counters(ui: &mut egui::Ui, language: Language, state: &RuntimeState) {
    ui.heading(text(language, Text::Counters));
    ui.add_space(6.0);
    counter_grid(
        ui,
        "detail_counters",
        &[
            (
                text(language, Text::HookToRelay),
                state.counters.hook_to_relay,
            ),
            (
                text(language, Text::RelayToHook),
                state.counters.relay_to_hook,
            ),
            (text(language, Text::SentBytes), state.counters.sent_bytes),
            (
                text(language, Text::ReceivedBytes),
                state.counters.received_bytes,
            ),
            (text(language, Text::Errors), state.counters.errors),
        ],
    );
}

pub(super) fn selected_account_label(
    selected_account: Option<usize>,
    accounts: &[SteamIdentity],
    language: Language,
) -> String {
    selected_account
        .and_then(|index| accounts.get(index))
        .map_or_else(|| text(language, Text::Manual).to_owned(), account_label)
}

pub(super) fn account_label(account: &SteamIdentity) -> String {
    format!("{} ({})", account.display_name, account.steam_id64)
}

fn counter_grid(ui: &mut egui::Ui, id: &'static str, counters: &[(&str, u64)]) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            for (label, value) in counters {
                ui.label(*label);
                ui.monospace(value.to_string());
                ui.end_row();
            }
        });
}
