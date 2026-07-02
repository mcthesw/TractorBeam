mod fonts;
mod pages;
mod status;
mod widgets;

use basement_bridge_core::{
    BridgeClient, ClientLogSink, ConnectionProfile, LocalDate, RelayEndpoint, RelayPreset,
    SessionConfig, SessionMode, SessionStatus, TransportChoice, load_client_config,
    resolve_room_template,
};
use eframe::egui::{self, ScrollArea};

use crate::i18n::{Language, Text, text};

use status::error_message;

/// Build a test room id of the form `YYYYMMDD-XXXX` (today + 4 random lowercase
/// alphanumerics). Used as the editable default room so testers do not have to
/// invent one, and so two independent test pairs on the same day do not collide.
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
    relay_presets: Vec<RelayPreset>,
    selected_relay: Option<usize>,
    relay_host: String,
    relay_port: u16,
    transport: TransportChoice,
    active_connection_profile: Option<ConnectionProfile>,
    room: String,
    mode: SessionMode,
    selected_account: Option<usize>,
    manual_steam_id: String,
    manual_display_name: String,
    last_error: Option<String>,
    last_log_directory: Option<String>,
    start_error_dialog_open: bool,
}

impl BridgeApp {
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        log_sink: Box<dyn ClientLogSink>,
    ) -> Self {
        fonts::configure_fonts(&creation_context.egui_ctx);

        let loaded_config = load_client_config();
        let client = BridgeClient::with_config_and_log_sink(loaded_config.clone(), log_sink);
        let selected_account = client
            .state()
            .detected_accounts
            .iter()
            .position(|account| account.most_recent)
            .or_else(|| (!client.state().detected_accounts.is_empty()).then_some(0));

        let startup_room = generate_room_id();

        let mut app = Self {
            client,
            language: Language::Chinese,
            page: Page::Home,
            relay_presets: loaded_config.config.relays.clone(),
            selected_relay: loaded_config.config.selected_relay_index(),
            relay_host: String::new(),
            relay_port: 25_910,
            transport: loaded_config.config.default_transport,
            active_connection_profile: None,
            room: startup_room,
            mode: loaded_config.config.default_mode,
            selected_account,
            manual_steam_id: String::new(),
            manual_display_name: String::new(),
            last_error: (!loaded_config.warnings.is_empty())
                .then(|| text(Language::Chinese, Text::ConfigWarning).to_owned()),
            last_log_directory: None,
            start_error_dialog_open: false,
        };
        app.apply_selected_relay_defaults();
        app
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
            relay_name: self.selected_relay_preset().map(|relay| relay.name.clone()),
            transport: self.transport,
            room: self.room.trim().to_owned(),
            mode: self.mode,
            steam_id64,
            display_name,
            session_health: self.client.loaded_config().config.session_health,
        }
    }

    fn current_connection_profile(&self) -> ConnectionProfile {
        match self.transport {
            TransportChoice::Tcp => ConnectionProfile::Tcp,
            TransportChoice::Udp => ConnectionProfile::Udp,
        }
    }

    fn set_connection_profile(&mut self, profile: ConnectionProfile) {
        self.transport = profile.transport();
    }

    fn connection_profile_pending(&self) -> bool {
        self.client.state().status == SessionStatus::Running
            && self
                .active_connection_profile
                .is_some_and(|active| active != self.current_connection_profile())
    }

    fn start(&mut self) {
        match self.client.start_session(&self.session_config()) {
            Ok(()) => {
                self.active_connection_profile = Some(self.current_connection_profile());
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

    fn open_log_directory(&mut self) {
        match self.client.open_log_directory() {
            Ok(path) => {
                self.last_error = None;
                self.last_log_directory = Some(path.display().to_string());
            }
            Err(error) => self.last_error = Some(error.to_string()),
        }
    }

    fn start_readiness_probe(&mut self) {
        let relay = RelayEndpoint::new(self.relay_host.trim(), self.relay_port);
        match self.client.start_readiness_probe(relay) {
            Ok(()) => {
                self.last_error = None;
            }
            Err(error) => {
                self.last_error = Some(error_message(self.language, &error));
            }
        }
    }

    fn run_hook_receive_probe(&mut self) {
        if let Err(error) = self.client.start_hook_receive_probe() {
            self.last_error = Some(error_message(self.language, &error));
        } else {
            self.last_error = None;
        }
    }

    fn selected_relay_preset(&self) -> Option<&RelayPreset> {
        self.selected_relay
            .and_then(|index| self.relay_presets.get(index))
    }

    fn relay_selection_label(&self) -> String {
        self.selected_relay_preset()
            .map_or_else(|| self.t(Text::ManualRelay).to_owned(), RelayPreset::label)
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
            .show(ui, |ui| {
                self.status_bar(ui);
            });

        egui::CentralPanel::default().show(ui, |ui| {
            self.top_bar(ui);
            ui.separator();
            ui.add_space(8.0);
            let page = self.page;
            match page {
                Page::Home | Page::Debug => {
                    ScrollArea::vertical()
                        .id_salt("page_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| match page {
                            Page::Home => self.home_page(ui),
                            Page::Debug => self.debug_page(ui),
                            Page::Diagnostics => unreachable!(),
                        });
                }
                Page::Diagnostics => self.diagnostics_page(ui),
            }
        });

        self.start_error_dialog(ui.ctx());
    }
}