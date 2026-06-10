mod fonts;
mod pages;
mod status;
mod widgets;

use basement_bridge_core::{
    BridgeClient, ClientLogSink, DEFAULT_RELAY_PROBE_PAYLOAD_BYTES, RelayEndpoint, SessionConfig,
    SessionMode, SessionStatus, TransportChoice, load_client_config,
};
use eframe::egui::{self, ScrollArea};

use crate::i18n::{Language, Text, text};

use status::error_message;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    Home,
    Diagnostics,
    Debug,
}

pub struct BridgeApp {
    client: BridgeClient,
    language: Language,
    page: Page,
    relay_host: String,
    relay_port: u16,
    transport: TransportChoice,
    room: String,
    mode: SessionMode,
    selected_account: Option<usize>,
    manual_steam_id: String,
    manual_display_name: String,
    last_error: Option<String>,
    last_export: Option<String>,
    last_relay_probe: Option<String>,
    last_hook_probe: Option<String>,
    relay_probe_payload_bytes: usize,
    start_error_dialog_open: bool,
}

impl BridgeApp {
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        log_sink: Box<dyn ClientLogSink>,
    ) -> Self {
        fonts::configure_fonts(&creation_context.egui_ctx);

        let client = BridgeClient::with_config_and_log_sink(load_client_config(), log_sink);
        let selected_account = client
            .state()
            .detected_accounts
            .iter()
            .position(|account| account.most_recent)
            .or_else(|| (!client.state().detected_accounts.is_empty()).then_some(0));

        Self {
            client,
            language: Language::Chinese,
            page: Page::Home,
            relay_host: String::new(),
            relay_port: 25_910,
            transport: TransportChoice::Udp,
            room: String::new(),
            mode: SessionMode::Pure,
            selected_account,
            manual_steam_id: String::new(),
            manual_display_name: String::new(),
            last_error: None,
            last_export: None,
            last_relay_probe: None,
            last_hook_probe: None,
            relay_probe_payload_bytes: DEFAULT_RELAY_PROBE_PAYLOAD_BYTES,
            start_error_dialog_open: false,
        }
    }

    fn t(&self, key: Text) -> &'static str {
        text(self.language, key)
    }

    fn current_identity(&self) -> (String, String) {
        self.selected_account
            .and_then(|index| self.client.state().detected_accounts.get(index))
            .map_or_else(
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
            transport: self.transport,
            room: self.room.trim().to_owned(),
            mode: self.mode,
            steam_id64,
            display_name,
        }
    }

    fn start(&mut self) {
        match self.client.start_session(&self.session_config()) {
            Ok(()) => {
                self.last_error = None;
                self.start_error_dialog_open = false;
            }
            Err(error) => {
                self.last_error = Some(error_message(self.language, &error));
                self.start_error_dialog_open = true;
            }
        }
    }

    fn refresh_accounts(&mut self) {
        self.client.refresh_steam_accounts();
        self.selected_account = self
            .client
            .state()
            .detected_accounts
            .iter()
            .position(|account| account.most_recent)
            .or_else(|| (!self.client.state().detected_accounts.is_empty()).then_some(0));
    }

    fn export_diagnostics(&mut self) {
        match self.client.export_diagnostics() {
            Ok(path) => {
                self.last_error = None;
                self.last_export = Some(path.display().to_string());
            }
            Err(error) => self.last_error = Some(error.to_string()),
        }
    }

    fn run_relay_probe(&mut self) {
        let relay = RelayEndpoint::new(self.relay_host.trim(), self.relay_port);
        match self.client.run_relay_probe_with_transport_payload(
            relay,
            self.transport,
            self.relay_probe_payload_bytes,
        ) {
            Ok(report) => {
                self.last_error = None;
                self.last_relay_probe = Some(report.to_string());
            }
            Err(error) => {
                let message = format!("Relay probe failed: {error}");
                self.last_error = Some(message.clone());
                self.last_relay_probe = Some(message);
            }
        }
    }

    fn run_hook_receive_probe(&mut self) {
        match self.client.run_hook_receive_probe() {
            Ok(report) => {
                self.last_error = None;
                self.last_hook_probe = Some(report.to_string());
            }
            Err(error) => {
                let message = format!("Hook receive probe failed: {error}");
                self.last_error = Some(message.clone());
                self.last_hook_probe = Some(message);
            }
        }
    }
}

impl eframe::App for BridgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.client.poll_events() {
            ui.ctx().request_repaint();
        }
        if self.client.state().status == SessionStatus::Running {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }

        egui::Panel::bottom("status_bar")
            .resizable(false)
            .exact_size(30.0)
            .show_inside(ui, |ui| {
                self.status_bar(ui);
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("app_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.top_bar(ui);
                    ui.separator();
                    ui.add_space(8.0);
                    match self.page {
                        Page::Home => self.home_page(ui),
                        Page::Diagnostics => self.diagnostics_page(ui),
                        Page::Debug => self.debug_page(ui),
                    }
                });
        });

        self.start_error_dialog(ui.ctx());
    }
}
