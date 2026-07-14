use super::*;

pub(super) fn handle_command(
    command: ApplicationCommand,
    client: &mut BridgeClient,
    snapshot: &Arc<SnapshotStore>,
    event_tx: &mpsc::Sender<ApplicationEvent>,
) {
    match command {
        ApplicationCommand::RetryBootstrap => {}
        ApplicationCommand::Start(request) => {
            if client.state().status != SessionStatus::Idle {
                send_application_event(event_tx, snapshot, ApplicationEvent::CommandRejected);
                return;
            }
            set_operation(snapshot, client, Some(ApplicationOperation::Starting));
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
