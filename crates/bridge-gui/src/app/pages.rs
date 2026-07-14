mod about;
mod helpers;
mod logs;
mod navigation;

use std::borrow::Cow;

use eframe::egui::{self, ComboBox, TextEdit};
use rust_i18n::t;
use tractor_beam_core::{
    ConnectionProfile, HookIpcConnectionState, HookReceiveProbeReport, HookStartupPhase,
    ReadinessProbeCaseReport, ReadinessProbeReport, RoomPathQualitySnapshot, RoomPathQualityState,
    RuntimeState, SessionCredential, SessionMode, SessionQuality, SessionStatus, TransportChoice,
    protocol::PeerPresence,
};

use helpers::*;

use super::{
    BridgeApp,
    status::{connection_profile_label, quality_label, smoothness_summary},
    widgets::{account_label, detail_counters, help_icon, label_with_help, selected_account_label},
};

impl BridgeApp {
    pub(super) fn home_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(t!("home"));
        ui.add_space(8.0);

        ui.add_enabled_ui(self.mutations_enabled(), |ui| {
            self.relay_section(ui);
            ui.add_space(8.0);

            self.steam_section(ui);
            ui.add_space(8.0);

            self.join_code_ui(ui);
            ui.add_space(8.0);
        });

        self.action_row(ui);
        ui.add_space(8.0);

        self.hook_progress_ui(ui);
        ui.add_space(8.0);

        self.room_members_ui(ui);
    }

    fn relay_section(&mut self, ui: &mut egui::Ui) {
        let relay_label = t!("relay.server");
        let manual_label = t!("relay.manual");
        let retest_label = t!("probe.test_latency");
        let host_label = t!("relay.host");
        label_with_help(ui, relay_label, t!("help.relay_server"));
        let mut selected_relay = self.selected_relay;
        let selected_text = selected_relay
            .and_then(|index| self.relay_presets.get(index))
            .map_or_else(
                || manual_label.clone(),
                |relay| Cow::Owned(self.relay_option_label(relay)),
            );
        ComboBox::from_id_salt("home_relay")
            .selected_text(selected_text)
            .width(400.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected_relay, None, manual_label);
                for (index, relay) in self.relay_presets.iter().enumerate() {
                    let label = self.relay_option_label(relay);
                    ui.selectable_value(&mut selected_relay, Some(index), label);
                }
            });
        if selected_relay != self.selected_relay {
            self.selected_relay = selected_relay;
            self.apply_selected_relay_defaults();
            self.persist_selection();
        }
        let manual = self.selected_relay.is_none();
        if manual {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_enabled(
                    manual,
                    TextEdit::singleline(&mut self.relay_host)
                        .hint_text(host_label)
                        .desired_width(310.0),
                );
                ui.add_enabled(
                    manual,
                    egui::DragValue::new(&mut self.relay_port).range(1..=u16::MAX),
                );
            });
        }
        ui.add_space(4.0);
        if ui.button(retest_label).clicked() {
            self.test_relay_latency();
        }
    }

    fn steam_section(&mut self, ui: &mut egui::Ui) {
        let accounts = self.client_state().detected_accounts.clone();
        let steam_label = t!("steam.account");
        let refresh_label = t!("steam.refresh_accounts");
        let manual_steam_label = t!("steam.manual_id");
        let display_name_label = t!("display_name");
        label_with_help(ui, steam_label, t!("help.steam_account"));
        if accounts.is_empty() {
            ui.label(t!("steam.no_accounts"));
        } else {
            let account_before = self.selected_account;
            ComboBox::from_id_salt("home_steam_account")
                .selected_text(selected_account_label(self.selected_account, &accounts))
                .width(400.0)
                .show_ui(ui, |ui| {
                    for (index, account) in accounts.iter().enumerate() {
                        ui.selectable_value(
                            &mut self.selected_account,
                            Some(index),
                            account_label(account),
                        );
                    }
                    ui.selectable_value(&mut self.selected_account, None, t!("manual"));
                });
            if self.selected_account != account_before {
                self.persist_selection();
            }
        }
        ui.add_space(2.0);
        if ui.button(refresh_label).clicked() {
            self.refresh_accounts();
            self.persist_selection();
        }
        if self.selected_account.is_none() {
            ui.add_space(4.0);
            ui.add(
                TextEdit::singleline(&mut self.manual_steam_id)
                    .hint_text(manual_steam_label)
                    .desired_width(400.0),
            );
            ui.add_space(2.0);
            ui.add(
                TextEdit::singleline(&mut self.manual_display_name)
                    .hint_text(display_name_label)
                    .desired_width(400.0),
            );
        }
    }

    fn join_code_ui(&mut self, ui: &mut egui::Ui) {
        let join_code_label = t!("join_code");
        let copy_label = t!("join_code.copy");
        let import_label = t!("join_code.import");
        let generate_label = t!("room.generate");
        ui.horizontal(|ui| {
            ui.label(join_code_label);
            help_icon(ui, t!("help.join_code"));
        });
        ui.horizontal(|ui| {
            if ui.button(copy_label).clicked() {
                match self.copy_join_code() {
                    Ok(code) => {
                        ui.ctx().copy_text(code);
                        self.join_code_message = Some(t!("join_code.copied").into_owned());
                    }
                    Err(error) => {
                        self.join_code_message =
                            Some(format!("{}: {error}", t!("join_code.invalid")));
                    }
                }
            }
            if ui.button(import_label).clicked() {
                self.read_join_code_from_clipboard();
            }
            if ui
                .add_enabled(self.mutations_enabled(), egui::Button::new(generate_label))
                .clicked()
            {
                self.session_credential = SessionCredential::generate();
                self.join_code_input.clear();
                self.join_code_message = Some(t!("room.generated").into_owned());
            }
        });
        if let Some(message) = &self.join_code_message {
            ui.add_space(4.0);
            ui.label(message);
        }
    }

    fn action_row(&mut self, ui: &mut egui::Ui) {
        let running = self.client_state().status == SessionStatus::Running;
        let starting =
            self.application_snapshot.operation == Some(super::ApplicationOperation::Starting);
        let mutation_enabled = self.mutations_enabled();
        let start_label = t!("start");
        let stop_label = t!("stop");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!running && mutation_enabled, egui::Button::new(start_label))
                .clicked()
            {
                self.start();
            }
            if ui
                .add_enabled(running || starting, egui::Button::new(stop_label))
                .clicked()
            {
                self.stop();
            }
        });
    }

    fn hook_progress_ui(&self, ui: &mut egui::Ui) {
        let startup = &self.client_state().hook_startup;
        if startup.phase == HookStartupPhase::NotStarted {
            return;
        }
        let progress_label = t!("hook.progress");
        ui.separator();
        ui.add_space(4.0);
        ui.heading(progress_label);
        ui.add_space(4.0);
        let (color, phase_text) = hook_phase_label(startup.phase);
        ui.horizontal(|ui| {
            ui.colored_label(color, "●");
            ui.label(phase_text);
        });
        if let Some(message) = &startup.message {
            ui.add_space(4.0);
            let rich = if startup.phase == HookStartupPhase::Failed {
                egui::RichText::new(message).color(ui.visuals().error_fg_color)
            } else {
                egui::RichText::new(message)
            };
            ui.add(egui::Label::new(rich).wrap());
        }
        if let Some(name) = &startup.process_name {
            ui.add_space(2.0);
            ui.monospace(format!(
                "{name} PID {}",
                startup.pid.map_or("-".to_owned(), |p| p.to_string())
            ));
        }
        if matches!(
            startup.phase,
            HookStartupPhase::WaitingForIsaac | HookStartupPhase::WaitingForHookEndpoint
        ) {
            ui.add_space(2.0);
            ui.monospace(format!(
                "{}: {}s",
                t!("elapsed"),
                unix_seconds().saturating_sub(startup.updated_at)
            ));
        }
        if startup.access_denied {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().error_fg_color, t!("hook.access_denied_hint"));
        }
    }

    fn room_members_ui(&self, ui: &mut egui::Ui) {
        let peers = &self.client_state().room_peers;
        let members_label = t!("room.members");
        ui.separator();
        ui.add_space(4.0);
        ui.heading(members_label);
        ui.add_space(4.0);
        if peers.is_empty() {
            ui.label(t!("room.empty"));
            return;
        }
        let (my_id, _) = self.current_identity();
        let my_id = my_id.parse::<u64>().ok();
        egui::ScrollArea::horizontal().show(ui, |ui| {
            egui::Grid::new("room_members")
                .num_columns(6)
                .striped(true)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    table_header(ui, t!("room.player"));
                    table_header(ui, t!("status"));
                    table_header(ui, t!("room.path.rtt"));
                    table_header(ui, t!("room.path.jitter"));
                    table_header(ui, t!("room.path.loss"));
                    table_header(ui, t!("room.path.freshness"));
                    ui.end_row();
                    for peer in peers {
                        let is_self = Some(peer.steam_id64) == my_id;
                        let fallback_name = peer.steam_id64.to_string();
                        let name = peer.display_name.as_deref().unwrap_or(&fallback_name);
                        let display = if is_self {
                            format!("▶ {name}")
                        } else {
                            name.to_owned()
                        };
                        let color = if is_self {
                            ui.visuals().strong_text_color()
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.colored_label(color, display);
                        if peer.presence == PeerPresence::Reconnecting {
                            ui.colored_label(
                                egui::Color32::from_rgb(185, 124, 0),
                                t!("room.reconnecting"),
                            );
                        } else {
                            ui.label(t!("status.running"));
                        }
                        let quality = self
                            .client_state()
                            .room_path_quality
                            .iter()
                            .find(|quality| quality.steam_id64 == peer.steam_id64);
                        room_path_quality_cells(ui, is_self, quality);
                        ui.end_row();
                    }
                });
        });
    }

    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui) {
        let settings_label = t!("settings");
        let profile_label = t!("connection_profile");
        let mode_label = t!("mode");
        let input_delay_label = t!("input_delay");
        let input_delay_read = t!("input_delay.read");
        let input_delay_write = t!("input_delay.write");
        let tcp = t!("transport.tcp");
        let udp = t!("transport.udp");
        let official = t!("mode.official");
        let fallback = t!("mode.fallback");
        let pure = t!("mode.pure");
        ui.heading(settings_label);
        ui.add_space(12.0);

        ui.add_enabled_ui(self.mutations_enabled(), |ui| {
            label_with_help(ui, profile_label, t!("help.connection_profile"));
            let profile_before = self.current_connection_profile();
            let mut selected_profile = profile_before;
            ui.horizontal(|ui| {
                ui.add_enabled_ui(self.preset_supports_transport(TransportChoice::Tcp), |ui| {
                    ui.radio_value(&mut selected_profile, ConnectionProfile::Tcp, tcp);
                });
                ui.add_enabled_ui(self.preset_supports_transport(TransportChoice::Udp), |ui| {
                    ui.radio_value(&mut selected_profile, ConnectionProfile::Udp, udp);
                });
            });
            if selected_profile != profile_before {
                self.transport = selected_profile.transport();
                self.persist_selection();
            }

            ui.add_space(12.0);
            label_with_help(ui, mode_label, t!("help.mode"));
            let mode_before = self.mode;
            ui.vertical(|ui| {
                ui.radio_value(&mut self.mode, SessionMode::Official, official);
                ui.radio_value(&mut self.mode, SessionMode::Fallback, fallback);
                ui.radio_value(&mut self.mode, SessionMode::Pure, pure);
            });
            if self.mode != mode_before {
                self.persist_selection();
            }

            ui.add_space(12.0);
            label_with_help(ui, input_delay_label, t!("help.input_delay"));
            let input_delay_enabled = input_delay_controls_enabled(self.client_state());
            ui.horizontal(|ui| {
                ui.add(
                    TextEdit::singleline(&mut self.input_delay_value)
                        .desired_width(120.0)
                        .hint_text("0"),
                );
                if ui
                    .add_enabled(input_delay_enabled, egui::Button::new(input_delay_read))
                    .clicked()
                {
                    self.read_input_delay();
                }
                if ui
                    .add_enabled(input_delay_enabled, egui::Button::new(input_delay_write))
                    .clicked()
                {
                    self.write_input_delay();
                }
            });
        });
        if let Some(message) = &self.input_delay_message {
            ui.add_space(4.0);
            ui.label(message);
        }
    }

    pub(super) fn stats_page(&mut self, ui: &mut egui::Ui) {
        let stats_label = t!("stats");
        let readiness_label = t!("probe.relay_readiness");
        let hook_recv_label = t!("probe.hook_receive");
        let run_hook_label = t!("probe.run_hook_receive");
        let probe_running_label = t!("probe.running");
        let run_readiness_label = t!("probe.run_readiness");
        ui.heading(stats_label);
        ui.add_space(8.0);

        session_health_summary(ui, self.client_state());
        ui.add_space(12.0);

        ui.separator();
        ui.add_space(8.0);
        detail_counters(ui, self.client_state());
        ui.add_space(12.0);

        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading(readiness_label);
            help_icon(ui, t!("help.probe.relay_readiness"));
        });
        let running = self.client_state().readiness_probe_running;
        if ui
            .add_enabled(
                !running && self.mutations_enabled(),
                egui::Button::new(run_readiness_label),
            )
            .clicked()
        {
            self.start_readiness_probe();
        }
        if running {
            ui.add_space(4.0);
            ui.label(probe_running_label.as_ref());
        }
        if let Some(report) = &self.client_state().latest_readiness_probe {
            ui.add_space(4.0);
            readiness_probe_table(ui, report);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.heading(hook_recv_label);
            help_icon(ui, t!("help.probe.hook_receive"));
        });
        if ui
            .add_enabled(self.mutations_enabled(), egui::Button::new(run_hook_label))
            .clicked()
        {
            self.run_hook_receive_probe();
        }
        if self.client_state().hook_probe_running {
            ui.add_space(4.0);
            ui.label(probe_running_label.as_ref());
        }
        if let Some(result) = &self.client_state().latest_hook_receive_probe {
            ui.add_space(4.0);
            hook_probe_table(ui, result);
        }
    }

    fn relay_latency_label(&self, endpoint: &tractor_beam_core::RelayEndpoint) -> String {
        let state = self.client_state();
        state
            .light_ping_reports
            .iter()
            .find(|report| &report.target.endpoint == endpoint)
            .map_or_else(
                || t!("probe.probing").into_owned(),
                |report| {
                    if let Some(ms) = report.median_rtt_ms {
                        format!("{ms} ms")
                    } else {
                        t!("probe.unreachable").into_owned()
                    }
                },
            )
    }

    fn relay_option_label(&self, relay: &tractor_beam_core::RelayPreset) -> String {
        format!(
            "{} ({})",
            relay.name,
            self.relay_latency_label(&relay.endpoint)
        )
    }
}
