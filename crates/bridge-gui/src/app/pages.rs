mod about;
mod logs;
mod navigation;

use std::borrow::Cow;

use eframe::egui::{self, ComboBox, TextEdit};
use rust_i18n::t;
use tractor_beam_core::{
    ConnectionProfile, HookIpcConnectionState, HookReceiveProbeReport, HookStartupPhase,
    ReadinessProbeCaseReport, ReadinessProbeReport, RuntimeState, SessionCredential, SessionMode,
    SessionQuality, SessionStatus, TransportChoice, protocol::PeerTransport,
};

use super::{
    BridgeApp,
    status::{connection_profile_label, quality_label},
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
        egui::Grid::new("room_members")
            .num_columns(3)
            .striped(true)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                for peer in peers {
                    let is_self = peer.steam_id64 == my_id;
                    let name = peer.display_name.as_deref().unwrap_or(&peer.steam_id64);
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
                    ui.label(peer_transport_label(peer.transport));
                    ui.end_row();
                }
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

fn hook_phase_label(phase: HookStartupPhase) -> (egui::Color32, Cow<'static, str>) {
    match phase {
        HookStartupPhase::NotStarted => (egui::Color32::GRAY, t!("hook.not_started")),
        HookStartupPhase::Configured => (
            egui::Color32::from_rgb(100, 149, 237),
            t!("hook.configured"),
        ),
        HookStartupPhase::WaitingForIsaac => (
            egui::Color32::from_rgb(255, 200, 0),
            t!("hook.waiting_isaac"),
        ),
        HookStartupPhase::Injecting => (egui::Color32::from_rgb(255, 200, 0), t!("hook.injecting")),
        HookStartupPhase::WaitingForHookEndpoint => (
            egui::Color32::from_rgb(255, 200, 0),
            t!("hook.waiting_endpoint"),
        ),
        HookStartupPhase::EndpointReady => (
            egui::Color32::from_rgb(100, 200, 100),
            t!("hook.endpoint_ready"),
        ),
        HookStartupPhase::Ready => (egui::Color32::from_rgb(100, 200, 100), t!("hook.ready")),
        HookStartupPhase::Failed => (egui::Color32::from_rgb(220, 80, 80), t!("hook.failed")),
        HookStartupPhase::Cancelled => (egui::Color32::GRAY, t!("hook.cancelled")),
    }
}

fn input_delay_controls_enabled(state: &RuntimeState) -> bool {
    state.status == SessionStatus::Running
        && matches!(
            state.active_session_mode,
            Some(SessionMode::Fallback | SessionMode::Pure)
        )
        && state.hook_ipc.connection == HookIpcConnectionState::Connected
}

fn peer_transport_label(transport: PeerTransport) -> Cow<'static, str> {
    match transport {
        PeerTransport::Udp => t!("transport.udp"),
        PeerTransport::Tcp => t!("transport.tcp"),
    }
}

fn readiness_probe_table(ui: &mut egui::Ui, report: &ReadinessProbeReport) {
    let horizontal_spacing = 12.0;
    let columns = 5.0;
    let col_width =
        ((ui.available_width() - horizontal_spacing * (columns - 1.0)) / columns).max(72.0);
    egui::Grid::new("readiness_probe_table")
        .num_columns(5)
        .min_col_width(col_width)
        .striped(true)
        .spacing([horizontal_spacing, 4.0])
        .show(ui, |ui| {
            table_header(ui, t!("transport"));
            table_header(ui, t!("size"));
            table_header(ui, t!("lost"));
            table_header(ui, t!("latency"));
            table_header(ui, t!("jitter"));
            ui.end_row();

            for case in &report.cases {
                ui.label(connection_profile_label(case.connection_profile));
                ui.label(format!("{} B", case.payload_bytes));
                ui.label(lost_summary(case));
                ui.add(egui::Label::new(latency_summary(case)).wrap());
                ui.label(display_latency(case.jitter_ms));
                ui.end_row();
            }
        });
    let failed_cases = report
        .cases
        .iter()
        .filter_map(|case| case.failure_reason.as_ref().map(|reason| (case, reason)));
    for (index, (case, reason)) in failed_cases.enumerate() {
        if index == 0 {
            ui.add_space(4.0);
            ui.label(t!("details"));
        }
        wrapped_colored_label(
            ui,
            ui.visuals().error_fg_color,
            &format!(
                "{} {} B: {reason}",
                connection_profile_label(case.connection_profile),
                case.payload_bytes
            ),
        );
    }
    if report.cases.is_empty() {
        ui.add_space(4.0);
        ui.label(t!("probe.no_data"));
    }
}

fn hook_probe_table(ui: &mut egui::Ui, report: &HookReceiveProbeReport) {
    egui::Grid::new("hook_probe_table")
        .num_columns(5)
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            table_header(ui, t!("hook.connection"));
            table_header(ui, t!("hook.protocol"));
            table_header(ui, t!("hook.reconnects"));
            table_header(ui, t!("hook.dropped"));
            table_header(ui, t!("hook.malformed"));
            ui.end_row();

            ui.label(&report.connection);
            ui.label(match (report.protocol_major, report.protocol_minor) {
                (Some(major), Some(minor)) => format!("{major}.{minor}"),
                _ => "-".to_owned(),
            });
            ui.label(report.reconnects.to_string());
            ui.label(format!(
                "{} / {}",
                report.hook_data_dropped, report.client_data_dropped
            ));
            ui.label(report.malformed_frames.to_string());
            ui.end_row();
        });
    if let Some(error) = &report.last_error {
        ui.add_space(4.0);
        wrapped_colored_label(ui, ui.visuals().error_fg_color, error);
    }
}

fn session_health_summary(ui: &mut egui::Ui, state: &RuntimeState) {
    ui.horizontal(|ui| {
        ui.heading(t!("session_quality"));
        help_icon(ui, t!("help.session_quality"));
    });
    ui.add_space(6.0);
    let Some(snapshot) = &state.latest_session_health else {
        ui.label(t!("session.not_started"));
        return;
    };
    let quality_color = match snapshot.quality {
        SessionQuality::Good => ui.visuals().strong_text_color(),
        SessionQuality::Watch | SessionQuality::Poor => ui.visuals().error_fg_color,
        SessionQuality::Unavailable => ui.visuals().weak_text_color(),
    };
    ui.horizontal(|ui| {
        ui.colored_label(quality_color, "●");
        ui.label(quality_label(snapshot.quality));
    });
    ui.add_space(4.0);
    egui::Grid::new("session_health_summary")
        .num_columns(2)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            ui.label(t!("health.runtime_rtt"));
            ui.monospace(display_latency_ms(snapshot.runtime_rtt.latency.p95_ms));
            ui.end_row();

            ui.label(t!("health.queue_drops"));
            ui.monospace(snapshot.queues.total_dropped().to_string());
            ui.end_row();

            ui.label(t!("health.sequence_gaps"));
            ui.monospace(snapshot.source_sequence.gaps.to_string());
            ui.end_row();

            ui.label(t!("health.packet_gaps"));
            ui.monospace(display_latency_ms(snapshot.relay_recv.gap.p95_ms));
            ui.end_row();
        });
}

fn table_header(ui: &mut egui::Ui, value: Cow<'static, str>) {
    ui.label(egui::RichText::new(value).strong());
}

fn wrapped_colored_label(ui: &mut egui::Ui, color: egui::Color32, value: &str) {
    ui.add(egui::Label::new(egui::RichText::new(value).color(color)).wrap());
}

fn latency_summary(report: &ReadinessProbeCaseReport) -> String {
    format!(
        "median={} ms p95={} ms",
        display_latency(report.median_latency_ms),
        display_latency(report.p95_latency_ms)
    )
}

fn lost_summary(report: &ReadinessProbeCaseReport) -> String {
    if report.packets_sent == 0 {
        "-".to_owned()
    } else {
        format!("{}/{}", report.missing_packets, report.packets_sent)
    }
}

fn display_latency(value: Option<u128>) -> String {
    value.map_or_else(
        || "-".to_owned(),
        |value| {
            if value == 0 {
                "<1".to_owned()
            } else {
                value.to_string()
            }
        },
    )
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

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
