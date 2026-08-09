mod about;
mod helpers;
mod logs;
mod navigation;
mod relay_settings;

use std::borrow::Cow;

use eframe::egui::{self, TextEdit};
use rust_i18n::t;
use tractor_beam_core::{
    ConnectionProfile, DirectOutcomeWindow, HookReceiveProbeReport, HookStartupPhase,
    ReadinessProbeCaseReport, ReadinessProbeReport, RoomPathQualitySnapshot, RoomPathQualityState,
    RuntimeState, SessionCredential, SessionMode, SessionQuality, SessionStatus, TransportChoice,
    protocol::PeerPresence,
};

use helpers::*;

use super::{
    BridgeApp, PendingRoomAction, RouteChoice, route_switch_allowed,
    status::{connection_profile_label, quality_label, smoothness_summary},
    widgets::{account_label, detail_counters, help_icon, label_with_help, selected_account_label},
};

impl BridgeApp {
    pub(super) fn home_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(t!("home"));
        ui.add_space(8.0);

        ui.add_enabled_ui(self.mutations_enabled(), |ui| {
            label_with_help(ui, t!("route"), t!("help.route"));
            let route_before = self.route;
            let can_switch = route_switch_allowed(
                self.application_snapshot.room_active(),
                self.client_state().status,
            );
            ui.add_enabled_ui(can_switch, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(
                        &mut self.route,
                        RouteChoice::ExternalRelay,
                        t!("route.relay"),
                    );
                    ui.radio_value(&mut self.route, RouteChoice::LanDirect, t!("route.lan"));
                });
            });
            if self.route != route_before {
                self.lan_create_dialog_open = false;
                self.join_code_message = None;
            }
            ui.add_space(6.0);
            ui.add_enabled_ui(self.application_snapshot.lan_room.is_none(), |ui| {
                if self.route == RouteChoice::ExternalRelay {
                    if self.application_snapshot.relay_room_active {
                        self.relay_connection_ui(ui);
                    } else {
                        self.relay_section(ui, true);
                        ui.add_space(8.0);
                        self.steam_section(ui, true);
                    }
                } else {
                    self.steam_section(ui, true);
                }
            });
            ui.add_space(8.0);

            self.join_code_ui(ui);
            ui.add_space(8.0);
        });

        self.hook_progress_ui(ui);
        ui.add_space(8.0);

        if self.route == RouteChoice::LanDirect {
            self.lan_room_ui(ui);
        } else {
            self.room_members_ui(ui);
        }
    }

    fn join_code_ui(&mut self, ui: &mut egui::Ui) {
        let lan_direct = self.route == RouteChoice::LanDirect;
        let room_active = self.application_snapshot.room_active();
        let running = self.client_state().status == SessionStatus::Running;
        let hook_ready = self.client_state().hook_runtime_active
            && self.client_state().hook_startup.phase == HookStartupPhase::Ready;
        let start_label = if hook_ready {
            t!("start.reconnect")
        } else {
            t!("start")
        };
        let mutation_enabled = self.mutations_enabled();
        let join_code_label = if lan_direct {
            t!("lan.join_code")
        } else {
            t!("relay.join_code")
        };
        let join_code_help = if lan_direct {
            t!("help.lan_join_code")
        } else {
            t!("help.join_code")
        };
        label_with_help(ui, join_code_label, join_code_help);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.mutations_enabled(),
                    egui::Button::new(t!("room.generate")),
                )
                .clicked()
            {
                if room_active {
                    self.pending_room_action = Some(PendingRoomAction::NewRoom);
                    self.room_switch_dialog_open = true;
                } else if lan_direct {
                    if !self.application.enumerate_lan_adapters() {
                        self.show_busy_status();
                    }
                } else {
                    self.session_credential = SessionCredential::generate();
                    self.join_code_input.clear();
                    let _ = self.enter_relay_room();
                }
            }
            if ui
                .add_enabled(
                    self.mutations_enabled(),
                    egui::Button::new(t!("join_code.import")),
                )
                .clicked()
            {
                self.read_join_code_from_clipboard();
            }
            if ui
                .add_enabled(room_active, egui::Button::new(t!("join_code.copy")))
                .clicked()
            {
                if let Some(room) = &self.application_snapshot.lan_room {
                    ui.ctx().copy_text(room.invitation_code.clone());
                    self.join_code_message = Some(t!("join_code.copied").into_owned());
                } else {
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
            }
            if ui
                .add_enabled(
                    room_active
                        && !running
                        && mutation_enabled
                        && (!self.client_state().hook_runtime_active || hook_ready),
                    egui::Button::new(start_label),
                )
                .clicked()
            {
                self.start();
            }
            if ui
                .add_enabled(room_active, egui::Button::new(t!("room.leave")))
                .clicked()
            {
                self.leave_room();
            }
        });
        if let Some(message) = &self.join_code_message {
            ui.add_space(4.0);
            ui.label(message);
        }
    }

    fn lan_room_ui(&self, ui: &mut egui::Ui) {
        let Some(room) = &self.application_snapshot.lan_room else {
            ui.label(t!("lan.no_room"));
            return;
        };
        ui.separator();
        ui.heading(t!("lan.peers"));
        if room.peers.is_empty() {
            ui.label(t!("room.empty"));
        }
        for peer in &room.peers {
            let path = room
                .paths
                .iter()
                .find(|path| path.peer == peer.peer.identity);
            ui.horizontal(|ui| {
                ui.label(
                    peer.peer
                        .display_name
                        .as_deref()
                        .unwrap_or(t!("display_name").as_ref()),
                );
                ui.label(match peer.connection {
                    tractor_beam_core::LanPeerConnectionState::Discovered => {
                        t!("lan.peer_discovered")
                    }
                    tractor_beam_core::LanPeerConnectionState::Connected => {
                        t!("lan.peer_connected")
                    }
                    tractor_beam_core::LanPeerConnectionState::Reconnecting => {
                        t!("lan.peer_reconnecting")
                    }
                });
                ui.label(path.map_or_else(
                    || t!("lan.path_unavailable").into_owned(),
                    |path| match path.status {
                        tractor_beam_core::LanPeerPathStatus::Checking => {
                            t!("lan.path_checking").into_owned()
                        }
                        tractor_beam_core::LanPeerPathStatus::Usable => {
                            t!("lan.path_usable").into_owned()
                        }
                        tractor_beam_core::LanPeerPathStatus::Unavailable => {
                            t!("lan.path_unavailable").into_owned()
                        }
                    },
                ));
                if let Some(path) =
                    path.and_then(|path| path.local_endpoint.zip(path.remote_endpoint))
                {
                    ui.monospace(format!("{} → {}", path.0, path.1));
                }
            });
        }
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

    fn room_members_ui(&mut self, ui: &mut egui::Ui) {
        if !self.application_snapshot.relay_room_active {
            ui.label(t!("room.not_joined"));
            return;
        }
        let members_label = t!("room.members");
        ui.separator();
        ui.add_space(4.0);
        if let Some(mismatch) = self.client_state().steam_identity_mismatch {
            wrapped_colored_label(
                ui,
                ui.visuals().error_fg_color,
                &format!(
                    "{} {} / {} {}",
                    t!("room.game_steam_id_mismatch"),
                    mismatch.game_steam_id64,
                    t!("room.configured_steam_id"),
                    mismatch.room_steam_id64,
                ),
            );
            ui.horizontal(|ui| {
                if ui.button(t!("room.use_game_account")).clicked() {
                    self.select_game_steam_account(mismatch.game_steam_id64);
                    self.persist_selection();
                    let _ = self.rejoin_relay_room();
                }
                if ui.button(t!("room.change_account")).clicked() {
                    self.begin_relay_settings_edit();
                }
            });
            ui.add_space(8.0);
        } else if self.client_state().hook_startup.phase == HookStartupPhase::Ready
            && self.client_state().hook_ipc.game_steam_id64.is_none()
        {
            wrapped_colored_label(
                ui,
                egui::Color32::from_rgb(185, 124, 0),
                t!("room.game_steam_id_unverified").as_ref(),
            );
            ui.add_space(8.0);
        }
        if !self.client_state().missing_game_targets.is_empty() {
            let targets = self
                .client_state()
                .missing_game_targets
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            wrapped_colored_label(
                ui,
                ui.visuals().error_fg_color,
                &format!("{} {targets}", t!("room.steam_id_mismatch")),
            );
            ui.add_space(8.0);
        }
        ui.heading(members_label);
        ui.add_space(4.0);
        let peers = &self.client_state().room_peers;
        if peers.is_empty() {
            ui.label(t!("room.empty"));
            return;
        }
        let my_id = self.client_state().relay_room_steam_id64;
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
                            ui.label(t!("room.connected"));
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
            ui.add_enabled_ui(!self.application_snapshot.room_active(), |ui| {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(self.preset_supports_transport(TransportChoice::Tcp), |ui| {
                        ui.radio_value(&mut selected_profile, ConnectionProfile::Tcp, tcp);
                    });
                    ui.add_enabled_ui(self.preset_supports_transport(TransportChoice::Udp), |ui| {
                        ui.radio_value(&mut selected_profile, ConnectionProfile::Udp, udp);
                    });
                });
            });
            if selected_profile != profile_before {
                self.transport = selected_profile.transport();
                self.persist_selection();
            }

            ui.add_space(12.0);
            label_with_help(ui, mode_label, t!("help.mode"));
            let mode_before = self.mode;
            ui.add_enabled_ui(!self.client_state().hook_runtime_active, |ui| {
                ui.vertical(|ui| {
                    ui.radio_value(&mut self.mode, SessionMode::Official, official);
                    ui.radio_value(&mut self.mode, SessionMode::Fallback, fallback);
                    ui.radio_value(&mut self.mode, SessionMode::Pure, pure);
                });
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
