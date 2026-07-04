use std::borrow::Cow;

use basement_bridge_core::{
    ConnectionProfile, HookReceiveProbeReport, HookStartupPhase, LogLevel,
    ReadinessProbeCaseReport, ReadinessProbeReport, RuntimeState, SessionMode, SessionQuality,
    SessionStatus, TransportChoice, build_info, protocol::PeerTransport,
};
use eframe::egui::{self, ComboBox, TextEdit};

use crate::i18n::{Language, Text, text};

use super::generate_room_id;
use super::{
    BridgeApp, Page,
    status::{connection_profile_label, quality_label},
    widgets::{account_label, detail_counters, selected_account_label},
};

const PROTOCOL_VERSION: &str = "1.0";
const LOG_LEVELS: [LogLevel; 5] = [
    LogLevel::Error,
    LogLevel::Warn,
    LogLevel::Info,
    LogLevel::Debug,
    LogLevel::Trace,
];

impl BridgeApp {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let selected_language = self.language.label();
        let home = self.t(Text::Home);
        let settings = self.t(Text::Settings);
        let stats = self.t(Text::Stats);
        let log = self.t(Text::Log);
        let about = self.t(Text::About);
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
                    .selected_text(selected_language)
                    .width(112.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.language,
                            Language::Chinese,
                            Language::Chinese.label(),
                        );
                        ui.selectable_value(
                            &mut self.language,
                            Language::English,
                            Language::English.label(),
                        );
                    });
            });
        });
    }

    pub(super) fn home_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.t(Text::Home));
        ui.add_space(8.0);

        self.relay_section(ui);
        ui.add_space(8.0);

        self.steam_section(ui);
        ui.add_space(8.0);

        self.join_code_ui(ui);
        ui.add_space(8.0);

        self.room_ui(ui);
        ui.add_space(8.0);

        self.action_row(ui);
        ui.add_space(8.0);

        self.hook_progress_ui(ui);
        ui.add_space(8.0);

        self.room_members_ui(ui);
    }

    fn relay_section(&mut self, ui: &mut egui::Ui) {
        let relay_label = self.t(Text::RelayServer);
        let manual_label = self.t(Text::ManualRelay);
        let retest_label = self.t(Text::TestLatency);
        let host_label = self.t(Text::RelayHost);
        ui.label(relay_label);
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
        let accounts = self.client.state().detected_accounts.clone();
        let steam_label = self.t(Text::SteamAccount);
        let refresh_label = self.t(Text::RefreshAccounts);
        let manual_steam_label = self.t(Text::ManualSteamId);
        let display_name_label = self.t(Text::DisplayName);
        ui.label(steam_label);
        if accounts.is_empty() {
            ui.label(self.t(Text::NoSteamAccounts));
        } else {
            let account_before = self.selected_account;
            ComboBox::from_id_salt("home_steam_account")
                .selected_text(selected_account_label(
                    self.selected_account,
                    &accounts,
                    self.language,
                ))
                .width(400.0)
                .show_ui(ui, |ui| {
                    for (index, account) in accounts.iter().enumerate() {
                        ui.selectable_value(
                            &mut self.selected_account,
                            Some(index),
                            account_label(account),
                        );
                    }
                    ui.selectable_value(
                        &mut self.selected_account,
                        None,
                        text(self.language, Text::Manual),
                    );
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
        let join_code_label = self.t(Text::JoinCode);
        let copy_label = self.t(Text::CopyCode);
        let import_label = self.t(Text::ImportCode);
        ui.label(join_code_label);
        ui.horizontal(|ui| {
            if ui.button(copy_label).clicked() {
                ui.ctx().copy_text(self.copy_join_code());
                self.join_code_message = Some(self.t(Text::CodeCopied).into_owned());
            }
            if ui.button(import_label).clicked() {
                match arboard::Clipboard::new() {
                    Ok(mut cb) => match cb.get_text() {
                        Ok(text) if !text.trim().is_empty() => {
                            self.join_code_input = text;
                            self.import_join_code();
                        }
                        _ => {
                            self.join_code_message =
                                Some(self.t(Text::ClipboardEmpty).into_owned());
                        }
                    },
                    Err(error) => {
                        self.join_code_message =
                            Some(format!("{}: {error}", self.t(Text::ClipboardEmpty)));
                    }
                }
            }
        });
        if let Some(message) = &self.join_code_message {
            ui.add_space(4.0);
            ui.label(message);
        }
    }

    fn room_ui(&mut self, ui: &mut egui::Ui) {
        let room_label = self.t(Text::Room);
        let generate_label = self.t(Text::GenerateRoom);
        ui.label(room_label);
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut self.room).desired_width(310.0));
            if ui.button(generate_label).clicked() {
                self.room = generate_room_id();
                self.persist_selection();
            }
        });
    }

    fn action_row(&mut self, ui: &mut egui::Ui) {
        let running = self.client.state().status == SessionStatus::Running;
        let start_label = self.t(Text::Start);
        let stop_label = self.t(Text::Stop);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!running, egui::Button::new(start_label))
                .clicked()
            {
                self.start();
            }
            if ui
                .add_enabled(running, egui::Button::new(stop_label))
                .clicked()
            {
                self.client.stop_session();
            }
        });
    }

    fn hook_progress_ui(&self, ui: &mut egui::Ui) {
        let startup = &self.client.state().hook_startup;
        if startup.phase == HookStartupPhase::NotStarted {
            return;
        }
        let progress_label = self.t(Text::HookProgress);
        ui.separator();
        ui.add_space(4.0);
        ui.heading(progress_label);
        ui.add_space(4.0);
        let (color, phase_text) = hook_phase_label(self.language, startup.phase);
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
                self.t(Text::Elapsed),
                unix_seconds().saturating_sub(startup.updated_at)
            ));
        }
        if startup.access_denied {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().error_fg_color, self.t(Text::AccessDeniedHint));
        }
    }

    fn room_members_ui(&self, ui: &mut egui::Ui) {
        let peers = &self.client.state().room_peers;
        let members_label = self.t(Text::RoomMembers);
        ui.separator();
        ui.add_space(4.0);
        ui.heading(members_label);
        ui.add_space(4.0);
        if peers.is_empty() {
            ui.label(self.t(Text::RoomEmpty));
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
                    ui.label(peer_transport_label(self.language, peer.transport));
                    ui.end_row();
                }
            });
    }

    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui) {
        let settings_label = self.t(Text::Settings);
        let profile_label = self.t(Text::ConnectionProfile);
        let mode_label = self.t(Text::Mode);
        let tcp = self.t(Text::Tcp);
        let udp = self.t(Text::Udp);
        let official = self.t(Text::Official);
        let fallback = self.t(Text::Fallback);
        let pure = self.t(Text::Pure);
        ui.heading(settings_label);
        ui.add_space(12.0);

        ui.label(profile_label);
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
        ui.label(mode_label);
        let mode_before = self.mode;
        ui.vertical(|ui| {
            ui.radio_value(&mut self.mode, SessionMode::Official, official);
            ui.radio_value(&mut self.mode, SessionMode::Fallback, fallback);
            ui.radio_value(&mut self.mode, SessionMode::Pure, pure);
        });
        if self.mode != mode_before {
            self.persist_selection();
        }
    }

    pub(super) fn stats_page(&mut self, ui: &mut egui::Ui) {
        let stats_label = self.t(Text::Stats);
        let readiness_label = self.t(Text::RelayReadiness);
        let hook_recv_label = self.t(Text::HookReceive);
        let run_hook_label = self.t(Text::RunHookReceiveProbe);
        let probe_running_label = self.t(Text::ProbeRunning);
        let run_readiness_label = self.t(Text::RunReadinessProbe);
        ui.heading(stats_label);
        ui.add_space(8.0);

        session_health_summary(ui, self.language, self.client.state());
        ui.add_space(12.0);

        ui.separator();
        ui.add_space(8.0);
        detail_counters(ui, self.language, self.client.state());
        ui.add_space(12.0);

        ui.separator();
        ui.add_space(8.0);
        ui.heading(readiness_label);
        let running = self.client.state().readiness_probe_running;
        if ui
            .add_enabled(!running, egui::Button::new(run_readiness_label))
            .clicked()
        {
            self.start_readiness_probe();
        }
        if running {
            ui.add_space(4.0);
            ui.label(probe_running_label.as_ref());
        }
        if let Some(report) = &self.client.state().latest_readiness_probe {
            ui.add_space(4.0);
            readiness_probe_table(ui, self.language, report);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.heading(hook_recv_label);
        if ui.button(run_hook_label).clicked() {
            self.run_hook_receive_probe();
        }
        if self.client.state().hook_probe_running {
            ui.add_space(4.0);
            ui.label(probe_running_label.as_ref());
        }
        if let Some(result) = &self.client.state().latest_hook_receive_probe {
            ui.add_space(4.0);
            hook_probe_table(ui, self.language, result);
        }
    }

    pub(super) fn log_page(&mut self, ui: &mut egui::Ui) {
        let log_label = self.t(Text::Log);
        let clear_label = self.t(Text::ClearLogs);
        let level_filter_label = self.t(Text::LogLevelFilter);
        let empty_label = self.t(Text::LogEmpty);
        let open_log_label = self.t(Text::OpenLogDirectory);
        ui.heading(log_label);
        ui.add_space(4.0);

        let logs = self.client.state().logs.clone();
        let max_level = ui.data_mut(|data| {
            data.get_persisted::<u8>("log_level_filter".into())
                .unwrap_or(LogLevel::Info as u8)
        });
        let mut selected_level = max_level;
        ui.horizontal(|ui| {
            ui.label(level_filter_label);
            ComboBox::from_id_salt("log_level_filter")
                .selected_text(level_name(self.language, u8_to_level(max_level)))
                .width(100.0)
                .show_ui(ui, |ui| {
                    for level in LOG_LEVELS {
                        ui.selectable_value(
                            &mut selected_level,
                            level as u8,
                            level_name(self.language, level),
                        );
                    }
                });
            if ui.button(clear_label).clicked() {
                self.client.clear_logs();
            }
            if ui.button(open_log_label).clicked() {
                self.open_log_directory();
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
                    if entry.level as u8 > filter_level as u8 {
                        continue;
                    }
                    ui.horizontal_top(|ui| {
                        ui.monospace(format!("[{}]", entry.timestamp));
                        ui.colored_label(log_level_color(ui, entry.level), entry.level.to_string());
                        ui.add(egui::Label::new(entry.message.as_str()).wrap());
                    });
                    ui.add_space(1.0);
                }
            });
    }

    pub(super) fn about_page(&mut self, ui: &mut egui::Ui) {
        let about_label = self.t(Text::About);
        let desc_label = self.t(Text::AboutDesc);
        let version_label = self.t(Text::Version);
        let proto_label = self.t(Text::ProtocolVersion);
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
        ui.hyperlink_to("GitHub", "https://github.com/mcthesw/Basement-Bridge");
        ui.add_space(2.0);
        ui.label(format!("{}: GNU AGPL-3.0-or-later", self.t(Text::License)));
    }

    fn relay_latency_label(&self, endpoint: &basement_bridge_core::RelayEndpoint) -> String {
        let state = self.client.state();
        state
            .light_ping_reports
            .iter()
            .find(|report| &report.target.endpoint == endpoint)
            .map_or_else(
                || self.t(Text::Probing).into_owned(),
                |report| {
                    if let Some(ms) = report.median_rtt_ms {
                        format!("{ms} ms")
                    } else {
                        self.t(Text::Unreachable).into_owned()
                    }
                },
            )
    }

    fn relay_option_label(&self, relay: &basement_bridge_core::RelayPreset) -> String {
        format!(
            "{} ({})",
            relay.name,
            self.relay_latency_label(&relay.endpoint)
        )
    }
}

fn hook_phase_label(
    language: Language,
    phase: HookStartupPhase,
) -> (egui::Color32, Cow<'static, str>) {
    match phase {
        HookStartupPhase::NotStarted => (egui::Color32::GRAY, text(language, Text::HookNotStarted)),
        HookStartupPhase::Configured => (
            egui::Color32::from_rgb(100, 149, 237),
            text(language, Text::HookConfigured),
        ),
        HookStartupPhase::WaitingForIsaac => (
            egui::Color32::from_rgb(255, 200, 0),
            text(language, Text::HookWaitingIsaac),
        ),
        HookStartupPhase::Injecting => (
            egui::Color32::from_rgb(255, 200, 0),
            text(language, Text::HookInjecting),
        ),
        HookStartupPhase::WaitingForHookEndpoint => (
            egui::Color32::from_rgb(255, 200, 0),
            text(language, Text::HookWaitingEndpoint),
        ),
        HookStartupPhase::EndpointReady => (
            egui::Color32::from_rgb(100, 200, 100),
            text(language, Text::HookEndpointReady),
        ),
        HookStartupPhase::Ready => (
            egui::Color32::from_rgb(100, 200, 100),
            text(language, Text::HookReady),
        ),
        HookStartupPhase::Failed => (
            egui::Color32::from_rgb(220, 80, 80),
            text(language, Text::HookFailed),
        ),
        HookStartupPhase::Cancelled => (egui::Color32::GRAY, text(language, Text::HookCancelled)),
    }
}

fn peer_transport_label(language: Language, transport: PeerTransport) -> Cow<'static, str> {
    match transport {
        PeerTransport::Udp => text(language, Text::Udp),
        PeerTransport::Tcp => text(language, Text::Tcp),
    }
}

fn readiness_probe_table(ui: &mut egui::Ui, language: Language, report: &ReadinessProbeReport) {
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
            table_header(ui, text(language, Text::Transport));
            table_header(ui, text(language, Text::Size));
            table_header(ui, text(language, Text::Lost));
            table_header(ui, text(language, Text::Latency));
            table_header(ui, text(language, Text::Jitter));
            ui.end_row();

            for case in &report.cases {
                ui.label(connection_profile_label(language, case.connection_profile));
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
            ui.label(text(language, Text::Details));
        }
        wrapped_colored_label(
            ui,
            ui.visuals().error_fg_color,
            &format!(
                "{} {} B: {reason}",
                connection_profile_label(language, case.connection_profile),
                case.payload_bytes
            ),
        );
    }
    if report.cases.is_empty() {
        ui.add_space(4.0);
        ui.label(text(language, Text::NoProbeData));
    }
}

fn hook_probe_table(ui: &mut egui::Ui, language: Language, report: &HookReceiveProbeReport) {
    egui::Grid::new("hook_probe_table")
        .num_columns(4)
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            table_header(ui, text(language, Text::Bytes));
            table_header(ui, text(language, Text::HookInput));
            table_header(ui, text(language, Text::HookAvailable));
            table_header(ui, text(language, Text::HookRead));
            ui.end_row();

            ui.label(format!("{} B", report.sent_bytes));
            probe_bool_cell(ui, language, report.local_in);
            probe_bool_cell(ui, language, report.available_hit);
            probe_bool_cell(ui, language, report.read_hit);
            ui.end_row();
        });
}

fn session_health_summary(ui: &mut egui::Ui, language: Language, state: &RuntimeState) {
    ui.heading(text(language, Text::SessionQuality));
    ui.add_space(6.0);
    let Some(snapshot) = &state.latest_session_health else {
        ui.label(text(language, Text::SessionNotStarted));
        return;
    };
    let quality_color = match snapshot.quality {
        SessionQuality::Good => ui.visuals().strong_text_color(),
        SessionQuality::Watch | SessionQuality::Poor => ui.visuals().error_fg_color,
        SessionQuality::Unavailable => ui.visuals().weak_text_color(),
    };
    ui.horizontal(|ui| {
        ui.colored_label(quality_color, "●");
        ui.label(quality_label(language, snapshot.quality));
    });
    ui.add_space(4.0);
    egui::Grid::new("session_health_summary")
        .num_columns(2)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            ui.label(text(language, Text::RuntimeRtt));
            ui.monospace(display_latency_ms(snapshot.runtime_rtt.latency.p95_ms));
            ui.end_row();

            ui.label(text(language, Text::QueueDrops));
            ui.monospace(snapshot.queues.total_dropped().to_string());
            ui.end_row();

            ui.label(text(language, Text::SequenceGaps));
            ui.monospace(snapshot.source_sequence.gaps.to_string());
            ui.end_row();

            ui.label(text(language, Text::PacketGaps));
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

fn probe_bool_cell(ui: &mut egui::Ui, language: Language, value: bool) {
    let color = if value {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().error_fg_color
    };
    let label = if value {
        text(language, Text::Yes)
    } else {
        text(language, Text::No)
    };
    ui.colored_label(color, label);
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

fn log_level_color(ui: &egui::Ui, level: LogLevel) -> egui::Color32 {
    match level {
        LogLevel::Trace => ui.visuals().weak_text_color(),
        LogLevel::Debug => egui::Color32::from_rgb(95, 125, 155),
        LogLevel::Info => ui.visuals().text_color(),
        LogLevel::Warn => egui::Color32::from_rgb(185, 124, 0),
        LogLevel::Error => ui.visuals().error_fg_color,
    }
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn u8_to_level(value: u8) -> LogLevel {
    match value {
        0 => LogLevel::Error,
        1 => LogLevel::Warn,
        2 => LogLevel::Info,
        3 => LogLevel::Debug,
        _ => LogLevel::Trace,
    }
}

fn level_name(language: Language, level: LogLevel) -> Cow<'static, str> {
    match level {
        LogLevel::Error => text(language, Text::LogLevelError),
        LogLevel::Warn => text(language, Text::LogLevelWarn),
        LogLevel::Info => text(language, Text::LogLevelInfo),
        LogLevel::Debug => text(language, Text::LogLevelDebug),
        LogLevel::Trace => text(language, Text::LogLevelTrace),
    }
}
