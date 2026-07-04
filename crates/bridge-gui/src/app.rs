mod fonts;
mod pages;
mod status;
mod widgets;

use basement_bridge_core::{
    BridgeClient, ClientConfigSelection, ClientLogSink, ConnectionProfile, LightPingTarget,
    LocalDate, RelayEndpoint, RelayPreset, SessionConfig, SessionMode, SessionStatus,
    SteamIdentity, TransportChoice, load_client_config, resolve_room_template,
    save_client_config_selection,
};
use eframe::egui::{self, ScrollArea};
use rust_i18n::t;

use crate::i18n::{Language, set_language};

use status::StatusMessage;

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
    status_message: Option<StatusMessage>,
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

        let selected_account = initial_selected_account(
            &client.state().detected_accounts,
            loaded_config.config.selected_steam_id64.as_deref(),
        );

        let startup_room = loaded_config
            .config
            .room
            .clone()
            .unwrap_or_else(generate_room_id);

        let language = Language::Chinese;
        set_language(language);

        let mut app = Self {
            client,
            language,
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
            status_message: (!loaded_config.warnings.is_empty())
                .then_some(StatusMessage::ConfigWarning),
            last_log_directory: None,
            start_error_dialog_open: false,
            join_code_input: String::new(),
            join_code_message: None,
        };
        app.apply_selected_relay_defaults();
        app.startup_light_ping();
        app
    }

    fn set_language(&mut self, language: Language) {
        if self.language != language {
            self.language = language;
            set_language(language);
        }
    }

    fn selected_steam_account(&self) -> Option<&SteamIdentity> {
        self.selected_account
            .and_then(|index| self.client.state().detected_accounts.get(index))
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

    fn start(&mut self) {
        match self.client.start_session(&self.session_config()) {
            Ok(()) => {
                self.active_connection_profile = Some(self.current_connection_profile());
                self.status_message = None;
                self.start_error_dialog_open = false;
                self.persist_selection();
            }
            Err(error) => {
                self.status_message = Some(StatusMessage::from_client_error(&error));
                self.start_error_dialog_open = true;
            }
        }
    }

    fn persist_selection(&self) {
        let selection = ClientConfigSelection {
            selected_relay: self.selected_relay_preset().map(|relay| relay.id.clone()),
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
        self.selected_account =
            initial_selected_account(&self.client.state().detected_accounts, None);
    }

    fn open_log_directory(&mut self) {
        match self.client.open_log_directory() {
            Ok(path) => {
                self.status_message = None;
                self.last_log_directory = Some(path.display().to_string());
            }
            Err(error) => self.status_message = Some(StatusMessage::Text(error.to_string())),
        }
    }

    fn start_readiness_probe(&mut self) {
        let relay = RelayEndpoint::new(self.relay_host.trim(), self.relay_port);
        if let Err(error) = self.client.start_readiness_probe(relay) {
            self.status_message = Some(StatusMessage::from_client_error(&error));
        } else {
            self.status_message = None;
        }
    }

    fn run_hook_receive_probe(&mut self) {
        if let Err(error) = self.client.start_hook_receive_probe() {
            self.status_message = Some(StatusMessage::from_client_error(&error));
        } else {
            self.status_message = None;
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

    fn test_relay_latency(&mut self) {
        self.startup_light_ping();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn account(steam_id64: &str, most_recent: bool) -> SteamIdentity {
        SteamIdentity {
            steam_id64: steam_id64.to_owned(),
            display_name: format!("User {steam_id64}"),
            most_recent,
        }
    }

    #[test]
    fn initial_selection_prefers_saved_account() {
        let accounts = [
            account("76561198000000001", true),
            account("76561198000000002", false),
        ];

        let selected = initial_selected_account(&accounts, Some("76561198000000002"));

        assert_eq!(selected, Some(1));
    }

    #[test]
    fn initial_selection_uses_most_recent_without_saved_match() {
        let accounts = [
            account("76561198000000001", false),
            account("76561198000000002", true),
        ];

        let selected = initial_selected_account(&accounts, Some("76561198000000003"));

        assert_eq!(selected, Some(1));
    }

    #[test]
    fn initial_selection_falls_back_to_first_account() {
        let accounts = [
            account("76561198000000001", false),
            account("76561198000000002", false),
        ];

        let selected = initial_selected_account(&accounts, None);

        assert_eq!(selected, Some(0));
    }

    #[test]
    fn initial_selection_handles_empty_accounts() {
        let selected = initial_selected_account(&[], None);

        assert_eq!(selected, None);
    }
}
