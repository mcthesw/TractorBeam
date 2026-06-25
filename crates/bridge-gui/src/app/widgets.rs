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

pub(super) fn udp_fec_summary(ui: &mut egui::Ui, state: &RuntimeState) {
    let Some(snapshot) = &state.latest_udp_fec else {
        return;
    };
    if snapshot.is_empty() {
        return;
    }
    ui.add_space(12.0);
    ui.heading("UDP FEC");
    ui.add_space(6.0);
    egui::Grid::new("udp_fec_summary")
        .num_columns(3)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            table_header(ui, "dir");
            table_header(ui, "profile");
            table_header(ui, "packets");
            ui.end_row();

            if let Some(send) = &snapshot.send {
                ui.label("send");
                ui.monospace(send.profile.as_deref().unwrap_or("-"));
                ui.monospace(format!(
                    "orig={} repair={} oversized={} bytes={}",
                    send.original_packets,
                    send.repair_packets,
                    send.oversized_passthrough_packets,
                    send.repair_bytes
                ));
                ui.end_row();
            }
            if let Some(receive) = &snapshot.receive {
                ui.label("recv");
                ui.monospace(receive.profile.as_deref().unwrap_or("-"));
                ui.monospace(format!(
                    "orig={} repair={} recovered={} unrecovered={} delay_p95={}",
                    receive.original_packets,
                    receive.repair_packets,
                    receive.recovered_packets,
                    receive.unrecovered_groups,
                    display_latency_ms(receive.decode_delay_p95_ms)
                ));
                ui.end_row();
            }
        });
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

fn table_header(ui: &mut egui::Ui, value: &str) {
    ui.label(egui::RichText::new(value).strong());
}

fn display_latency_ms(value: Option<u64>) -> String {
    value.map_or_else(
        || "-".to_owned(),
        |value| {
            if value == 0 {
                "<1 ms".to_owned()
            } else {
                format!("{value} ms")
            }
        },
    )
}
