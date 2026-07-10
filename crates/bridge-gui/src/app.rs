mod fonts;
mod pages;
mod status;
mod widgets;

use std::time::{Duration, Instant};

use eframe::egui::{self, ScrollArea};
use rust_i18n::t;
use tractor_beam_core::{
    ClientConfigSelection, ConnectionProfile, InputDelayError, JoinCode, LightPingTarget,
    LocalDate, RelayEndpoint, RelayPreset, RuntimeState, SessionConfig, SessionHealthConfig,
    SessionMode, SteamIdentity, TransportChoice, resolve_room_template,
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

fn generate_room_id() -> String {
    let date = resolve_room_template("{date:%Y%m%d}", LocalDate::today())
        .unwrap_or_else(|_| "20260101".to_owned());
    format!("{date}-{}", random_room_suffix())
}

fn random_room_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut seed = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ nanos;
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(4);
    for _ in 0..4 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        out.push(CHARSET[(seed % CHARSET.len() as u64) as usize] as char);
    }
    out
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

pub struct BridgeApp {
    application: ApplicationHandle,
    application_snapshot: ApplicationSnapshot,
    form_initialized: bool,
    shutdown: ShutdownGate,
    language: Language,
    page: Page,
    relay_presets: Vec<RelayPreset>,
    selected_relay: Option<usize>,
    relay_host: String,
    relay_port: u16,
    transport: TransportChoice,
    room: String,
    admission: String,
    mode: SessionMode,
    selected_account: Option<usize>,
    manual_steam_id: String,
    manual_display_name: String,
    status_message: Option<StatusMessage>,
    input_delay_value: String,
    input_delay_message: Option<String>,
    start_error_dialog_open: bool,
    join_code_input: String,
    join_code_message: Option<String>,
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
            selected_relay: None,
            relay_host: String::new(),
            relay_port: 25_910,
            transport: TransportChoice::Tcp,
            room: generate_room_id(),
            admission: JoinCode::generate_admission(),
            mode: SessionMode::Pure,
            selected_account: None,
            manual_steam_id: String::new(),
            manual_display_name: String::new(),
            status_message: None,
            input_delay_value: String::new(),
            input_delay_message: None,
            start_error_dialog_open: false,
            join_code_input: String::new(),
            join_code_message: None,
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
        self.room = loaded_config
            .config
            .room
            .clone()
            .unwrap_or_else(generate_room_id);
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
        SessionConfig {
            relay: RelayEndpoint::new(self.relay_host.trim(), self.relay_port),
            relay_name: self.selected_relay_preset().map(|relay| relay.name.clone()),
            transport: self.transport,
            room: self.room.trim().to_owned(),
            admission: self.admission.clone(),
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
            room: Some(self.room.clone()),
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

    fn export_diagnostics(&mut self) {
        if !self.application.export_diagnostics() {
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

    fn sync_application(&mut self) {
        self.application_snapshot = self.application.snapshot();
        self.initialize_form();
        for event in self.application.drain_events() {
            self.handle_application_event(event);
        }
    }

    fn handle_application_event(&mut self, event: ApplicationEvent) {
        match event {
            ApplicationEvent::StartFinished(result) => match result {
                Ok(()) => {
                    self.status_message = None;
                    self.start_error_dialog_open = false;
                }
                Err(error) => {
                    self.status_message = Some(StatusMessage::from_client_error(&error));
                    self.start_error_dialog_open = true;
                }
            },
            ApplicationEvent::StopFinished => self.status_message = None,
            ApplicationEvent::AccountsRefreshed => {
                self.selected_account =
                    initial_selected_account(&self.client_state().detected_accounts, None);
                self.persist_selection();
            }
            ApplicationEvent::ReadinessProbeStarted(result)
            | ApplicationEvent::HookReceiveProbeStarted(result)
            | ApplicationEvent::LightPingStarted(result) => match result {
                Ok(()) => self.status_message = None,
                Err(error) => {
                    self.status_message = Some(StatusMessage::from_client_error(&error));
                }
            },
            ApplicationEvent::InputDelayReadFinished(result) => match result {
                Ok(report) => {
                    self.input_delay_value = report.value.to_string();
                    self.input_delay_message =
                        Some(format!("{}: {}", t!("input_delay.read_ok"), report.value));
                    self.status_message = None;
                }
                Err(error) => self.set_input_delay_error(&error),
            },
            ApplicationEvent::InputDelayWriteFinished(result) => match result {
                Ok(report) => {
                    let mut message = format!("{}: {}", t!("input_delay.write_ok"), report.value);
                    if report.value < 0 {
                        message.push_str(t!("input_delay.negative_hint").as_ref());
                    }
                    self.input_delay_message = Some(message);
                    self.status_message = None;
                }
                Err(error) => self.set_input_delay_error(&error),
            },
            ApplicationEvent::LogDirectoryOpened(result) => match result {
                Ok(path) => {
                    self.status_message = None;
                    tracing::info!(directory = %path.display(), "Opened log directory");
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Could not open log directory");
                    self.status_message = Some(StatusMessage::LogOpenFailed);
                }
            },
            ApplicationEvent::DiagnosticsExported(result) => match result {
                Ok(path) => {
                    tracing::info!(path = %path.display(), "Diagnostics export completed");
                    self.status_message = Some(StatusMessage::DiagnosticsExported);
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Could not export diagnostics");
                    self.status_message = Some(StatusMessage::DiagnosticsExportFailed);
                }
            },
            ApplicationEvent::ClipboardReadFinished(result) => match result {
                Ok(text) if !text.trim().is_empty() => {
                    self.join_code_input = text;
                    self.import_join_code();
                }
                Ok(_) => self.join_code_message = Some(t!("clipboard.empty").into_owned()),
                Err(error) => {
                    self.join_code_message = Some(format!("{}: {error}", t!("clipboard.empty")));
                }
            },
            ApplicationEvent::SelectionSaveFailed(error) => {
                tracing::warn!(error = %error, "Could not save GUI selection");
                self.status_message = Some(StatusMessage::SelectionSaveFailed);
            }
            ApplicationEvent::CommandRejected => self.show_busy_status(),
            ApplicationEvent::ShutdownComplete => {}
        }
    }

    fn handle_close(&mut self, context: &egui::Context) {
        let close_requested = context.input(|input| input.viewport().close_requested());
        if close_requested && !self.shutdown.close_allowed {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.shutdown.request(Instant::now()) {
                self.application.request_shutdown();
            }
        }

        if self.application_snapshot.shutdown_complete {
            self.shutdown.complete();
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.shutdown.timed_out(Instant::now()) {
            tracing::error!("Application shutdown exceeded the three-second deadline");
            std::process::exit(0);
        }
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

    fn selected_relay_preset(&self) -> Option<&RelayPreset> {
        self.selected_relay
            .and_then(|index| self.relay_presets.get(index))
    }

    fn apply_selected_relay_defaults(&mut self) {
        let Some(relay) = self.selected_relay_preset().cloned() else {
            return;
        };
        self.transport = relay.preferred_transport(self.transport);
        self.relay_host = relay.endpoint.host;
        self.relay_port = relay.endpoint.port;
    }

    fn preset_supports_transport(&self, transport: TransportChoice) -> bool {
        self.selected_relay_preset()
            .is_none_or(|relay| relay.supports(transport))
    }

    fn copy_join_code(&self) -> String {
        let relay_id = self.selected_relay_preset().map(|relay| relay.id.clone());
        JoinCode {
            relay_id,
            relay_host: self.relay_host.trim().to_owned(),
            relay_port: self.relay_port,
            room: self.room.trim().to_owned(),
            admission: self.admission.clone(),
        }
        .encode()
    }

    fn import_join_code(&mut self) {
        let input = if self.join_code_input.trim().is_empty() {
            return;
        } else {
            self.join_code_input.trim().to_owned()
        };
        match JoinCode::decode(&input) {
            Ok(code) => {
                if let Some(ref relay_id) = code.relay_id {
                    if let Some(index) = self
                        .relay_presets
                        .iter()
                        .position(|relay| &relay.id == relay_id)
                    {
                        self.selected_relay = Some(index);
                        self.apply_selected_relay_defaults();
                    } else {
                        self.selected_relay = None;
                        self.relay_host = code.relay_host.clone();
                        self.relay_port = code.relay_port;
                    }
                } else {
                    self.selected_relay = None;
                    self.relay_host = code.relay_host.clone();
                    self.relay_port = code.relay_port;
                }
                self.room = code.room.clone();
                self.admission = code.admission.clone();
                self.join_code_message = Some(t!("join_code.imported").into_owned());
                self.status_message = None;
                self.persist_selection();
            }
            Err(error) => {
                self.join_code_message = Some(format!("{}: {error}", t!("join_code.invalid")));
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
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
