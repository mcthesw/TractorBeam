mod events;
mod fonts;
mod join_code;
mod pages;
mod status;
mod widgets;

use std::time::{Duration, Instant};

use eframe::egui::{self, ScrollArea};
use rust_i18n::t;
use tractor_beam_core::{
    ClientConfigSelection, ConnectionProfile, ExternalRelayConfig, InputDelayError, JoinCode,
    LanAdapter, LanDirectConfig, LanJoinCode, LanProbeResult, LightPingTarget, RelayEndpoint,
    RelayJoinCode, RelayPreset, RuntimeState, SessionConfig, SessionCredential,
    SessionHealthConfig, SessionMode, SessionRouteConfig, SessionStatus, SteamIdentity,
    TransportChoice, default_lan_adapters,
};

use crate::{
    application::{
        ApplicationEvent, ApplicationHandle, ApplicationOperation, ApplicationSnapshot,
        BootstrapState,
    },
    i18n::{Language, set_language},
};

use status::StatusMessage;

const APPLICATION_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Default)]
struct ShutdownGate {
    deadline: Option<Instant>,
    close_allowed: bool,
}

impl ShutdownGate {
    fn request(&mut self, now: Instant) -> bool {
        if self.deadline.is_some() {
            return false;
        }
        self.deadline = Some(now + SHUTDOWN_DEADLINE);
        true
    }

    fn complete(&mut self) {
        self.close_allowed = true;
    }

    fn active(&self) -> bool {
        self.deadline.is_some()
    }

    fn timed_out(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| now >= deadline)
    }
}

fn initial_selected_account(
    accounts: &[SteamIdentity],
    selected_steam_id64: Option<&str>,
) -> Option<usize> {
    selected_steam_id64
        .and_then(|id| accounts.iter().position(|account| account.steam_id64 == id))
        .or_else(|| accounts.iter().position(|account| account.most_recent))
        .or_else(|| (!accounts.is_empty()).then_some(0))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    Home,
    Settings,
    Stats,
    Log,
    About,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RouteChoice {
    #[default]
    ExternalRelay,
    LanDirect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LanProbeDisposition {
    NoneReachable,
    JoinOne,
    Choose,
}

fn default_lan_adapter_selection(adapters: Vec<LanAdapter>) -> Vec<(LanAdapter, bool)> {
    let defaults = default_lan_adapters(&adapters);
    adapters
        .into_iter()
        .map(|adapter| {
            let selected = defaults
                .iter()
                .any(|default| default.adapter_id == adapter.adapter_id);
            (adapter, selected)
        })
        .collect()
}

fn lan_probe_disposition(result_count: usize) -> LanProbeDisposition {
    match result_count {
        0 => LanProbeDisposition::NoneReachable,
        1 => LanProbeDisposition::JoinOne,
        _ => LanProbeDisposition::Choose,
    }
}

fn route_switch_allowed(lan_room_active: bool, status: SessionStatus) -> bool {
    !lan_room_active && status == SessionStatus::Idle
}

pub struct BridgeApp {
    application: ApplicationHandle,
    application_snapshot: ApplicationSnapshot,
    form_initialized: bool,
    shutdown: ShutdownGate,
    language: Language,
    page: Page,
    relay_presets: Vec<RelayPreset>,
    route: RouteChoice,
    selected_relay: Option<usize>,
    relay_host: String,
    relay_port: u16,
    transport: TransportChoice,
    session_credential: SessionCredential,
    mode: SessionMode,
    selected_account: Option<usize>,
    manual_steam_id: String,
    manual_display_name: String,
    status_message: Option<StatusMessage>,
    input_delay_value: String,
    input_delay_message: Option<String>,
    start_error_dialog_open: bool,
    join_code_dialog_open: bool,
    join_code_input: String,
    join_code_message: Option<String>,
    lan_create_dialog_open: bool,
    lan_adapters: Vec<(LanAdapter, bool)>,
    pending_lan_invitation: Option<LanJoinCode>,
    lan_probe_results: Vec<LanProbeResult>,
    selected_lan_probe: Option<usize>,
    last_troubleshooting_package: Option<std::path::PathBuf>,
    session_health: SessionHealthConfig,
}

impl BridgeApp {
    pub fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        fonts::configure_fonts(&creation_context.egui_ctx);
        let language = Language::Chinese;
        set_language(language);
        let repaint_context = creation_context.egui_ctx.clone();
        let application = ApplicationHandle::spawn(move || repaint_context.request_repaint());
        let application_snapshot = application.snapshot();

        Self {
            application,
            application_snapshot,
            form_initialized: false,
            shutdown: ShutdownGate::default(),
            language,
            page: Page::Home,
            relay_presets: Vec::new(),
            route: RouteChoice::ExternalRelay,
            selected_relay: None,
            relay_host: String::new(),
            relay_port: 25_910,
            transport: TransportChoice::Tcp,
            session_credential: SessionCredential::generate(),
            mode: SessionMode::Pure,
            selected_account: None,
            manual_steam_id: String::new(),
            manual_display_name: String::new(),
            status_message: None,
            input_delay_value: String::new(),
            input_delay_message: None,
            start_error_dialog_open: false,
            join_code_dialog_open: false,
            join_code_input: String::new(),
            join_code_message: None,
            lan_create_dialog_open: false,
            lan_adapters: Vec::new(),
            pending_lan_invitation: None,
            lan_probe_results: Vec::new(),
            selected_lan_probe: None,
            last_troubleshooting_package: None,
            session_health: SessionHealthConfig::default(),
        }
    }

    fn initialize_form(&mut self) {
        if self.form_initialized {
            return;
        }
        let Some(loaded_config) = self.application_snapshot.loaded_config.clone() else {
            return;
        };
        self.relay_presets = loaded_config.config.relays.clone();
        self.selected_relay = loaded_config.config.selected_relay_index();
        self.transport = loaded_config.config.default_transport;
        self.mode = loaded_config.config.default_mode;
        self.session_health = loaded_config.config.session_health;
        self.selected_account = initial_selected_account(
            &self.application_snapshot.runtime.detected_accounts,
            loaded_config.config.selected_steam_id64.as_deref(),
        );
        self.status_message =
            (!loaded_config.warnings.is_empty()).then_some(StatusMessage::ConfigWarning);
        self.apply_selected_relay_defaults();
        self.form_initialized = true;
        self.startup_light_ping();
    }

    fn set_language(&mut self, language: Language) {
        if self.language != language {
            self.language = language;
            set_language(language);
        }
    }

    fn selected_steam_account(&self) -> Option<&SteamIdentity> {
        self.selected_account
            .and_then(|index| self.client_state().detected_accounts.get(index))
    }

    fn current_identity(&self) -> (String, String) {
        self.selected_steam_account().map_or_else(
            || {
                (
                    self.manual_steam_id.trim().to_owned(),
                    self.manual_display_name.trim().to_owned(),
                )
            },
            |account| (account.steam_id64.clone(), account.display_name.clone()),
        )
    }

    fn session_config(&self) -> SessionConfig {
        let (steam_id64, display_name) = self.current_identity();
        let route = match self.route {
            RouteChoice::ExternalRelay => SessionRouteConfig::ExternalRelay(ExternalRelayConfig {
                relay: RelayEndpoint::new(self.relay_host.trim(), self.relay_port),
                relay_name: self.selected_relay_preset().map(|relay| relay.name.clone()),
                transport: self.transport,
                session_credential: self.session_credential,
            }),
            RouteChoice::LanDirect => SessionRouteConfig::LanDirect(LanDirectConfig {
                session_credential: self
                    .pending_lan_invitation
                    .as_ref()
                    .map_or(self.session_credential, |invitation| {
                        invitation.session_credential
                    }),
                room: None,
            }),
        };
        SessionConfig {
            route,
            mode: self.mode,
            steam_id64,
            display_name,
            session_health: self.session_health,
        }
    }

    fn current_connection_profile(&self) -> ConnectionProfile {
        match self.transport {
            TransportChoice::Tcp => ConnectionProfile::Tcp,
            TransportChoice::Udp => ConnectionProfile::Udp,
        }
    }

    fn start(&mut self) {
        let accepted = self
            .application
            .start(self.session_config(), self.config_selection());
        if accepted {
            self.status_message = None;
            self.start_error_dialog_open = false;
        } else {
            self.show_busy_status();
        }
    }

    fn persist_selection(&self) {
        self.application.persist_selection(self.config_selection());
    }

    fn config_selection(&self) -> ClientConfigSelection {
        ClientConfigSelection {
            selected_relay: self.selected_relay_preset().map(|relay| relay.id.clone()),
            selected_steam_id64: {
                let (id, _) = self.current_identity();
                if id.is_empty() { None } else { Some(id) }
            },
        }
    }

    fn refresh_accounts(&mut self) {
        if !self.application.refresh_accounts() {
            self.show_busy_status();
        }
    }

    fn open_log_directory(&mut self) {
        if !self.application.open_log_directory() {
            self.show_busy_status();
        }
    }

    fn export_troubleshooting_package(&mut self) {
        if !self.application.export_troubleshooting_package() {
            self.show_busy_status();
        }
    }

    fn start_readiness_probe(&mut self) {
        let relay = RelayEndpoint::new(self.relay_host.trim(), self.relay_port);
        if self.application.start_readiness_probe(relay) {
            self.status_message = None;
        } else {
            self.show_busy_status();
        }
    }

    fn run_hook_receive_probe(&mut self) {
        if self.application.start_hook_receive_probe() {
            self.status_message = None;
        } else {
            self.show_busy_status();
        }
    }

    fn read_input_delay(&mut self) {
        if !self.application.read_input_delay() {
            self.show_busy_status();
        }
    }

    fn write_input_delay(&mut self) {
        let Ok(value) = self.input_delay_value.trim().parse::<i32>() else {
            self.input_delay_message = Some(t!("input_delay.invalid").into_owned());
            return;
        };
        if !self.application.write_input_delay(value) {
            self.show_busy_status();
        }
    }

    fn set_input_delay_error(&mut self, error: &InputDelayError) {
        self.input_delay_message = Some(status::input_delay_error_label(error));
    }

    fn startup_light_ping(&mut self) {
        let targets: Vec<LightPingTarget> = self
            .relay_presets
            .iter()
            .map(|relay| LightPingTarget {
                relay_id: Some(relay.id.clone()),
                relay_name: Some(relay.name.clone()),
                endpoint: relay.endpoint.clone(),
                transport: relay.preferred_transport(self.transport),
            })
            .collect();
        if !self.application.start_light_ping(targets) {
            self.show_busy_status();
        }
    }

    fn test_relay_latency(&mut self) {
        self.startup_light_ping();
    }

    fn stop(&mut self) {
        self.application.request_stop();
        self.status_message = None;
    }

    fn clear_logs(&mut self) {
        if !self.application.clear_logs() {
            self.show_busy_status();
        }
    }

    fn read_join_code_from_clipboard(&mut self) {
        if !self.application.read_clipboard() {
            self.show_busy_status();
        }
    }

    fn client_state(&self) -> &RuntimeState {
        &self.application_snapshot.runtime
    }

    fn mutations_enabled(&self) -> bool {
        self.application_snapshot.accepts_mutation()
    }

    fn show_busy_status(&mut self) {
        self.status_message = Some(StatusMessage::Busy);
    }

    fn lifecycle_view(&mut self, ui: &mut egui::Ui) -> bool {
        if self.shutdown.active() {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.heading(t!("status.shutting_down"));
                ui.spinner();
            });
            return true;
        }
        match self.application_snapshot.bootstrap {
            BootstrapState::Ready => false,
            BootstrapState::Initializing => {
                ui.vertical_centered(|ui| {
                    ui.add_space(48.0);
                    ui.heading(t!("status.initializing"));
                    ui.spinner();
                });
                true
            }
            BootstrapState::Failed => {
                ui.vertical_centered(|ui| {
                    ui.add_space(48.0);
                    ui.heading(t!("status.initialization_failed"));
                    ui.label(t!("status.initialization_retry_hint"));
                    if ui.button(t!("retry")).clicked() && !self.application.retry_bootstrap() {
                        self.show_busy_status();
                    }
                    if ui.button(t!("logs.open_directory")).clicked() {
                        self.open_log_directory();
                    }
                });
                true
            }
        }
    }
}

impl eframe::App for BridgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.sync_application();
        self.handle_close(ui.ctx());
        if self.application_snapshot.needs_polling() || self.shutdown.active() {
            ui.ctx().request_repaint_after(APPLICATION_POLL_INTERVAL);
        }

        if self.lifecycle_view(ui) {
            return;
        }

        egui::Panel::bottom("status_bar")
            .resizable(false)
            .exact_size(30.0)
            .show(ui, |ui| {
                self.status_bar(ui);
            });

        egui::CentralPanel::default().show(ui, |ui| {
            self.top_bar(ui);
            ui.separator();
            ui.add_space(8.0);
            if self.page == Page::Log {
                self.log_page(ui);
            } else {
                ScrollArea::vertical()
                    .id_salt("page_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.page {
                        Page::Home => self.home_page(ui),
                        Page::Settings => self.settings_page(ui),
                        Page::Stats => self.stats_page(ui),
                        Page::About => self.about_page(ui),
                        Page::Log => unreachable!(),
                    });
            }
        });

        self.start_error_dialog(ui.ctx());
        self.join_code_dialog(ui.ctx());
        self.lan_create_dialog(ui.ctx());
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
