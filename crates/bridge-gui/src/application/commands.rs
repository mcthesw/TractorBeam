use super::*;

pub(super) fn handle_command(
    command: ApplicationCommand,
    client: &mut BridgeClient,
    lan_room: &mut Option<LanRoomHandle>,
    snapshot: &Arc<SnapshotStore>,
    event_tx: &mpsc::Sender<ApplicationEvent>,
) {
    match command {
        ApplicationCommand::RetryBootstrap => {}
        ApplicationCommand::Start(mut request) => {
            if client.state().status != SessionStatus::Idle {
                send_application_event(event_tx, snapshot, ApplicationEvent::CommandRejected);
                return;
            }
            set_operation(snapshot, client, Some(ApplicationOperation::Starting));
            if let tractor_beam_core::SessionRouteConfig::LanDirect(route) =
                &mut request.config.route
            {
                route.room = lan_room.as_ref().map(LanRoomHandle::room);
            }
            let result = client.start_session(&request.config);
            if result.is_ok()
                && let Err(error) = save_client_config_selection(&request.selection)
            {
                send_application_event(
                    event_tx,
                    snapshot,
                    ApplicationEvent::SelectionSaveFailed(error.to_string()),
                );
            }
            set_operation(snapshot, client, None);
            send_application_event(event_tx, snapshot, ApplicationEvent::StartFinished(result));
        }
        ApplicationCommand::RefreshAccounts => {
            set_operation(
                snapshot,
                client,
                Some(ApplicationOperation::RefreshingAccounts),
            );
            client.refresh_steam_accounts();
            set_operation(snapshot, client, None);
            send_application_event(event_tx, snapshot, ApplicationEvent::AccountsRefreshed);
        }
        ApplicationCommand::StartReadinessProbe(relay) => {
            set_operation(snapshot, client, Some(ApplicationOperation::Probing));
            let result = client.start_readiness_probe(relay);
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::ReadinessProbeStarted(result),
            );
        }
        ApplicationCommand::StartHookReceiveProbe => {
            set_operation(snapshot, client, Some(ApplicationOperation::Probing));
            let result = client.start_hook_receive_probe();
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::HookReceiveProbeStarted(result),
            );
        }
        ApplicationCommand::StartLightPing(targets) => {
            set_operation(snapshot, client, Some(ApplicationOperation::Probing));
            let result = client.start_light_ping_probes(targets);
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::LightPingStarted(result),
            );
        }
        ApplicationCommand::ReadInputDelay => {
            set_operation(
                snapshot,
                client,
                Some(ApplicationOperation::ReadingInputDelay),
            );
            let result = client.read_input_delay();
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::InputDelayReadFinished(result),
            );
        }
        ApplicationCommand::WriteInputDelay(value) => {
            set_operation(
                snapshot,
                client,
                Some(ApplicationOperation::WritingInputDelay),
            );
            let result = client.write_input_delay(value);
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::InputDelayWriteFinished(result),
            );
        }
        ApplicationCommand::OpenLogDirectory => {
            set_operation(snapshot, client, Some(ApplicationOperation::OpeningLogs));
            let result = client
                .open_log_directory()
                .map_err(|error| error.to_string());
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::LogDirectoryOpened(result),
            );
        }
        ApplicationCommand::ExportTroubleshootingPackage => {
            set_operation(
                snapshot,
                client,
                Some(ApplicationOperation::ExportingTroubleshootingPackage),
            );
            let result = choose_troubleshooting_package_path()
                .map(|path| {
                    client
                        .export_troubleshooting_package(&path)
                        .map(Some)
                        .map_err(|error| error.to_string())
                })
                .unwrap_or(Ok(None));
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::TroubleshootingPackageExported(result),
            );
        }
        ApplicationCommand::ClearLogs => {
            client.clear_logs();
            publish_client(snapshot, client);
        }
        ApplicationCommand::ReadClipboard => {
            set_operation(
                snapshot,
                client,
                Some(ApplicationOperation::ReadingClipboard),
            );
            let result = read_clipboard_text();
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::ClipboardReadFinished(result),
            );
        }
        ApplicationCommand::EnumerateLanAdapters => {
            set_operation(snapshot, client, Some(ApplicationOperation::ConfiguringLan));
            let result = enumerate_lan_adapters().map_err(|error| error.to_string());
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::LanAdaptersEnumerated(result),
            );
        }
        ApplicationCommand::CreateLanRoom {
            steam_id64,
            display_name,
            adapters,
        } => {
            set_operation(snapshot, client, Some(ApplicationOperation::ConfiguringLan));
            let result = lan_candidate_addresses(&adapters)
                .map_err(|error| error.to_string())
                .and_then(|addresses| {
                    LanRoomHandle::create(
                        steam_id64,
                        display_name,
                        tractor_beam_core::SessionCredential::generate(),
                        &addresses,
                    )
                    .map_err(|error| error.to_string())
                })
                .and_then(|room| {
                    let code = room.invitation_code().map_err(|error| error.to_string())?;
                    write_clipboard_text(&code)?;
                    *lan_room = Some(room);
                    Ok(code)
                });
            client.record_lan_stage("bind", if result.is_ok() { "ok" } else { "failed" });
            publish_lan_room(snapshot, client, lan_room.as_ref());
            set_operation(snapshot, client, None);
            send_application_event(event_tx, snapshot, ApplicationEvent::LanRoomCreated(result));
        }
        ApplicationCommand::ProbeLanJoin(invitation) => {
            set_operation(snapshot, client, Some(ApplicationOperation::ConfiguringLan));
            let result = LanRoomHandle::probe(&invitation)
                .map(|results| (invitation, results))
                .map_err(|error| error.to_string());
            client.record_lan_stage(
                "probe",
                if result
                    .as_ref()
                    .is_ok_and(|(_, results)| !results.is_empty())
                {
                    "reachable"
                } else {
                    "unreachable"
                },
            );
            set_operation(snapshot, client, None);
            send_application_event(
                event_tx,
                snapshot,
                ApplicationEvent::LanProbeFinished(result),
            );
        }
        ApplicationCommand::JoinLanRoom {
            steam_id64,
            display_name,
            invitation,
            endpoint,
        } => {
            set_operation(snapshot, client, Some(ApplicationOperation::ConfiguringLan));
            let result = enumerate_lan_adapters()
                .map_err(|error| error.to_string())
                .and_then(|adapters| {
                    lan_candidate_addresses(&default_lan_adapters(&adapters))
                        .map_err(|error| error.to_string())
                })
                .and_then(|addresses| {
                    LanRoomHandle::join(steam_id64, display_name, &invitation, endpoint, &addresses)
                        .map_err(|error| error.to_string())
                })
                .map(|room| *lan_room = Some(room));
            client.record_lan_stage("admission", if result.is_ok() { "ok" } else { "failed" });
            publish_lan_room(snapshot, client, lan_room.as_ref());
            set_operation(snapshot, client, None);
            send_application_event(event_tx, snapshot, ApplicationEvent::LanRoomJoined(result));
        }
    }
}

#[cfg(windows)]
fn choose_troubleshooting_package_path() -> Option<PathBuf> {
    let filename = format!(
        "tractor-beam-troubleshooting-{}.zip",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    rfd::FileDialog::new()
        .set_title("Save Tractor Beam Troubleshooting Package")
        .set_file_name(filename)
        .add_filter("ZIP archive", &["zip"])
        .save_file()
}

#[cfg(not(windows))]
fn choose_troubleshooting_package_path() -> Option<PathBuf> {
    None
}

fn read_clipboard_text() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard.get_text().map_err(|error| error.to_string())
}

fn write_clipboard_text(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|error| error.to_string())
}
