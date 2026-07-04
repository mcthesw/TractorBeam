mod fonts;
mod pages;
mod status;
mod widgets;

use basement_bridge_core::{
    BridgeClient, ClientConfigSelection, ClientLogSink, ConnectionProfile,
    LocalDate, LightPingTarget, RelayEndpoint, RelayPreset, SessionConfig, SessionMode,
    SessionStatus, TransportChoice, load_client_config, resolve_room_template,
    save_client_config_selection,
};
use eframe::egui::{self, ScrollArea};

use crate::i18n::{Language, Text, text};

use status::error_message;

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
    Settings,
    Stats,
    Log,
    About,
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
    display_name: String,
    last_error: Option<String>,
    last_log_directory: Option<String>,
    start_error_dialog_open: bool,
    join_code_input: String,
    join_code_message: Option<String>,
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
            .position(|account| {
                loaded_config
                    .config
                    .selected_steam_id64
                    .as_deref()
                    .is_some_and(|id| id == account.steam_id64)
            });

        let startup_room = loaded_config
            .config
            .room
            .clone()
            .unwrap_or_else(generate_room_id);

        let initial_display_name = selected_account
            .and_then(|index| client.state().detected_accounts.get(index))
            .map(|account| account.display_name.clone())
            .unwrap_or_default();

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
            display_name: initial_display_name,
            last_error: (!loaded_config.warnings.is_empty())
                .then(|| text(Language::Chinese, Text::ConfigWarning).to_owned()),
            last_log_directory: None,
            start_error_dialog_open: false,
            join_code_input: String::new(),
            join_code_message: None,
        };
        app.apply_selected_relay_defaults();
        app.startup_light_ping();
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
        let (steam_id64, _) = self.current_identity();
        SessionConfig {
            relay: RelayEndpoint::new(self.relay_host.trim(), self.relay_port),
            relay_name: self.selected_relay_preset().map(|relay| relay.name.clone()),
            transport: self.transport,
            room: self.room.trim().to_owned(),
            mode: self.mode,
            steam_id64,
            display_name: self.display_name.trim().to_owned(),
            session_health: self.client.loaded_config().config.session_health,
        }
    }

    fn current_connection_profile(&self) -> ConnectionProfile {
        match self.transport {
            TransportChoice::Tcp => ConnectionProfile::Tcp,
            TransportChoice::Udp => ConnectionProfile::Udp,
        }
    }

    fn start(&mut self) {
        match self.client.start_session(&self.session_config()) {
            Ok(()) => {
                self.active_connection_profile = Some(self.current_connection_profile());
                self.last_error = None;
                self.start_error_dialog_open = false;
                self.persist_selection();
            }
            Err(error) => {
                self.last_error = Some(error_message(self.language, &error));
                self.start_error_dialog_open = true;
            }
        }
    }

    fn persist_selection(&self) {
        let selection = ClientConfigSelection {
            selected_relay: self
                .selected_relay_preset()
                .map(|relay| relay.id.clone()),
            room: Some(self.room.clone()),
            selected_steam_id64: {
                let (id, _) = self.current_identity();
                if id.is_empty() { None } else { Some(id) }
            },
        };
        let _ = save_client_config_selection(&selection);
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
        if let Err(error) = self.client.start_readiness_probe(relay) {
            self.last_error = Some(error_message(self.language, &error));
        } else {
            self.last_error = None;
        }
    }

    fn run_hook_receive_probe(&mut self) {
        if let Err(error) = self.client.start_hook_receive_probe() {
            self.last_error = Some(error_message(self.language, &error));
        } else {
            self.last_error = None;
        }
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
        let _ = self.client.start_light_ping_probes(targets);
    }

    fn retest_relays(&mut self) {
        self.startup_light_ping();
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

    fn copy_join_code(&self) -> String {
        let relay_id = self.selected_relay_preset().map(|relay| relay.id.clone());
        basement_bridge_core::JoinCode {
            relay_id,
            relay_host: self.relay_host.trim().to_owned(),
            relay_port: self.relay_port,
            room: self.room.trim().to_owned(),
        }
        .encode()
    }

    fn import_join_code(&mut self) {
        let input = if self.join_code_input.trim().is_empty() {
            return;
        } else {
            self.join_code_input.trim().to_owned()
        };
        match basement_bridge_core::JoinCode::decode(&input) {
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
                self.join_code_message = Some(self.t(Text::CodeImported).to_owned());
                self.last_error = None;
                self.persist_selection();
            }
            Err(error) => {
                self.join_code_message = Some(format!("{}: {error}", self.t(Text::CodeInvalid)));
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