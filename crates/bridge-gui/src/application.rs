use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::Duration,
};

use tractor_beam_core::{
    BridgeClient, ClientConfigSelection, ClientError, InputDelayError, InputDelayReport,
    LightPingTarget, LoadedClientConfig, RelayEndpoint, RuntimeState, SessionConfig, SessionStatus,
    load_client_config, save_client_config_selection,
};

use crate::logging::ClientLogFiles;

const COMMAND_QUEUE_CAPACITY: usize = 16;
const RUNTIME_POLL_INTERVAL: Duration = Duration::from_millis(25);
const CONTROL_NONE: u8 = 0;
const CONTROL_STOP: u8 = 1;
const CONTROL_SHUTDOWN: u8 = 2;

type WakeCallback = Arc<dyn Fn() + Send + Sync>;
type BootstrapFactory = Box<dyn FnMut() -> io::Result<(BridgeClient, LoadedClientConfig)> + Send>;

struct SnapshotStore {
    value: Mutex<ApplicationSnapshot>,
    wake: WakeCallback,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum BootstrapState {
    #[default]
    Initializing,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApplicationOperation {
    Starting,
    Stopping,
    RefreshingAccounts,
    Probing,
    ReadingInputDelay,
    WritingInputDelay,
    OpeningLogs,
    ExportingTroubleshootingPackage,
    ReadingClipboard,
    ShuttingDown,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ApplicationSnapshot {
    pub(crate) bootstrap: BootstrapState,
    pub(crate) bootstrap_error: Option<String>,
    pub(crate) operation: Option<ApplicationOperation>,
    pub(crate) runtime: RuntimeState,
    pub(crate) loaded_config: Option<LoadedClientConfig>,
    pub(crate) shutdown_complete: bool,
    command_generation: u64,
}

impl ApplicationSnapshot {
    #[must_use]
    pub(crate) fn accepts_mutation(&self) -> bool {
        self.bootstrap == BootstrapState::Ready
            && self.operation.is_none()
            && !self.shutdown_complete
    }

    #[must_use]
    pub(crate) fn needs_polling(&self) -> bool {
        self.bootstrap == BootstrapState::Initializing
            || self.operation.is_some()
            || self.runtime.status == SessionStatus::Running
    }
}

#[derive(Debug)]
pub(crate) enum ApplicationEvent {
    StartFinished(Result<(), ClientError>),
    StopFinished,
    AccountsRefreshed,
    ReadinessProbeStarted(Result<(), ClientError>),
    HookReceiveProbeStarted(Result<(), ClientError>),
    LightPingStarted(Result<(), ClientError>),
    InputDelayReadFinished(Result<InputDelayReport, InputDelayError>),
    InputDelayWriteFinished(Result<InputDelayReport, InputDelayError>),
    LogDirectoryOpened(Result<PathBuf, String>),
    TroubleshootingPackageExported(Result<Option<PathBuf>, String>),
    ClipboardReadFinished(Result<String, String>),
    SelectionSaveFailed(String),
    CommandRejected,
    ShutdownComplete,
}

#[derive(Debug)]
enum ApplicationCommand {
    RetryBootstrap,
    Start(Box<StartRequest>),
    RefreshAccounts,
    StartReadinessProbe(RelayEndpoint),
    StartHookReceiveProbe,
    StartLightPing(Vec<LightPingTarget>),
    ReadInputDelay,
    WriteInputDelay(i32),
    OpenLogDirectory,
    ExportTroubleshootingPackage,
    ClearLogs,
    ReadClipboard,
}

#[derive(Debug)]
struct StartRequest {
    config: SessionConfig,
    selection: ClientConfigSelection,
}

#[derive(Debug)]
struct QueuedCommand {
    command_generation: u64,
    command: ApplicationCommand,
}

pub(crate) struct ApplicationHandle {
    command_tx: SyncSender<QueuedCommand>,
    event_rx: Receiver<ApplicationEvent>,
    snapshot: Arc<SnapshotStore>,
    pending_selection: Arc<Mutex<Option<ClientConfigSelection>>>,
    control: Arc<AtomicU8>,
}

impl ApplicationHandle {
    #[must_use]
    pub(crate) fn spawn(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::spawn_with(wake, Box::new(production_bootstrap))
    }

    fn spawn_with(
        wake: impl Fn() + Send + Sync + 'static,
        bootstrap_factory: BootstrapFactory,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel();
        let snapshot = Arc::new(SnapshotStore {
            value: Mutex::new(ApplicationSnapshot::default()),
            wake: Arc::new(wake),
        });
        let pending_selection = Arc::new(Mutex::new(None));
        let control = Arc::new(AtomicU8::new(CONTROL_NONE));

        let worker_snapshot = Arc::clone(&snapshot);
        let worker_selection = Arc::clone(&pending_selection);
        let worker_control = Arc::clone(&control);
        let spawn_result = thread::Builder::new()
            .name("tractor-beam-application".to_owned())
            .spawn(move || {
                run_application(
                    command_rx,
                    event_tx,
                    worker_snapshot,
                    worker_selection,
                    worker_control,
                    bootstrap_factory,
                );
            });
        if let Err(error) = spawn_result {
            update_snapshot(&snapshot, |snapshot| {
                snapshot.bootstrap = BootstrapState::Failed;
                snapshot.bootstrap_error = Some(format!("Could not start application: {error}"));
            });
        }

        Self {
            command_tx,
            event_rx,
            snapshot,
            pending_selection,
            control,
        }
    }

    #[must_use]
    pub(crate) fn snapshot(&self) -> ApplicationSnapshot {
        lock(&self.snapshot.value).clone()
    }

    pub(crate) fn drain_events(&self) -> Vec<ApplicationEvent> {
        self.event_rx.try_iter().collect()
    }

    pub(crate) fn retry_bootstrap(&self) -> bool {
        self.submit(ApplicationCommand::RetryBootstrap)
    }

    pub(crate) fn start(&self, config: SessionConfig, selection: ClientConfigSelection) -> bool {
        self.submit(ApplicationCommand::Start(Box::new(StartRequest {
            config,
            selection,
        })))
    }

    pub(crate) fn request_stop(&self) {
        self.control.fetch_max(CONTROL_STOP, Ordering::Release);
    }

    pub(crate) fn request_shutdown(&self) {
        self.control.store(CONTROL_SHUTDOWN, Ordering::Release);
    }

    pub(crate) fn refresh_accounts(&self) -> bool {
        self.submit(ApplicationCommand::RefreshAccounts)
    }

    pub(crate) fn start_readiness_probe(&self, relay: RelayEndpoint) -> bool {
        self.submit(ApplicationCommand::StartReadinessProbe(relay))
    }

    pub(crate) fn start_hook_receive_probe(&self) -> bool {
        self.submit(ApplicationCommand::StartHookReceiveProbe)
    }

    pub(crate) fn start_light_ping(&self, targets: Vec<LightPingTarget>) -> bool {
        self.submit(ApplicationCommand::StartLightPing(targets))
    }

    pub(crate) fn read_input_delay(&self) -> bool {
        self.submit(ApplicationCommand::ReadInputDelay)
    }

    pub(crate) fn write_input_delay(&self, value: i32) -> bool {
        self.submit(ApplicationCommand::WriteInputDelay(value))
    }

    pub(crate) fn open_log_directory(&self) -> bool {
        self.submit(ApplicationCommand::OpenLogDirectory)
    }

    pub(crate) fn export_troubleshooting_package(&self) -> bool {
        self.submit(ApplicationCommand::ExportTroubleshootingPackage)
    }

    pub(crate) fn clear_logs(&self) -> bool {
        self.submit(ApplicationCommand::ClearLogs)
    }

    pub(crate) fn read_clipboard(&self) -> bool {
        self.submit(ApplicationCommand::ReadClipboard)
    }

    pub(crate) fn persist_selection(&self, selection: ClientConfigSelection) {
        *lock(&self.pending_selection) = Some(selection);
    }

    fn submit(&self, command: ApplicationCommand) -> bool {
        let queued = QueuedCommand {
            command_generation: lock(&self.snapshot.value).command_generation,
            command,
        };
        match self.command_tx.try_send(queued) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

fn run_application(
    command_rx: Receiver<QueuedCommand>,
    event_tx: mpsc::Sender<ApplicationEvent>,
    snapshot: Arc<SnapshotStore>,
    pending_selection: Arc<Mutex<Option<ClientConfigSelection>>>,
    control: Arc<AtomicU8>,
    mut bootstrap_factory: BootstrapFactory,
) {
    let mut client = bootstrap(&snapshot, &mut bootstrap_factory);

    loop {
        match control.swap(CONTROL_NONE, Ordering::AcqRel) {
            CONTROL_SHUTDOWN => {
                update_snapshot(&snapshot, |snapshot| {
                    snapshot.operation = Some(ApplicationOperation::ShuttingDown);
                });
                if let Some(client) = client.as_mut() {
                    client.shutdown();
                    publish_client(&snapshot, client);
                }
                update_snapshot(&snapshot, |snapshot| {
                    snapshot.operation = None;
                    snapshot.shutdown_complete = true;
                });
                send_application_event(&event_tx, &snapshot, ApplicationEvent::ShutdownComplete);
                return;
            }
            CONTROL_STOP => {
                if let Some(client) = client.as_mut() {
                    set_operation(&snapshot, client, Some(ApplicationOperation::Stopping));
                    client.stop_session();
                    set_operation(&snapshot, client, None);
                    send_application_event(&event_tx, &snapshot, ApplicationEvent::StopFinished);
                }
            }
            _ => {}
        }

        if let Some(client) = client.as_mut()
            && client.poll_events()
        {
            publish_client(&snapshot, client);
        }

        if let Some(selection) = lock(&pending_selection).take()
            && let Err(error) = save_client_config_selection(&selection)
        {
            send_application_event(
                &event_tx,
                &snapshot,
                ApplicationEvent::SelectionSaveFailed(error.to_string()),
            );
        }

        match command_rx.recv_timeout(RUNTIME_POLL_INTERVAL) {
            Ok(QueuedCommand {
                command: ApplicationCommand::RetryBootstrap,
                ..
            }) => {
                if client.is_none() {
                    client = bootstrap(&snapshot, &mut bootstrap_factory);
                }
            }
            Ok(
                queued @ QueuedCommand {
                    command: ApplicationCommand::OpenLogDirectory,
                    ..
                },
            ) if client.is_none() => {
                if failed_bootstrap_command_is_current(&snapshot, &queued) {
                    let result =
                        ClientLogFiles::open_default_directory().map_err(|error| error.to_string());
                    send_application_event(
                        &event_tx,
                        &snapshot,
                        ApplicationEvent::LogDirectoryOpened(result),
                    );
                } else {
                    send_application_event(&event_tx, &snapshot, ApplicationEvent::CommandRejected);
                }
            }
            Ok(queued) => {
                let Some(active_client) = client.as_mut() else {
                    send_application_event(&event_tx, &snapshot, ApplicationEvent::CommandRejected);
                    continue;
                };
                if !command_is_current(&snapshot, &queued) {
                    send_application_event(&event_tx, &snapshot, ApplicationEvent::CommandRejected);
                    continue;
                }
                handle_command(queued.command, active_client, &snapshot, &event_tx);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(client) = client.as_mut() {
                    client.shutdown();
                }
                return;
            }
        }
    }
}

fn bootstrap(
    snapshot: &Arc<SnapshotStore>,
    bootstrap_factory: &mut BootstrapFactory,
) -> Option<BridgeClient> {
    update_snapshot(snapshot, |snapshot| {
        snapshot.command_generation = snapshot.command_generation.saturating_add(1);
        snapshot.bootstrap = BootstrapState::Initializing;
        snapshot.bootstrap_error = None;
        snapshot.operation = None;
    });

    match bootstrap_factory() {
        Ok((client, loaded_config)) => {
            update_snapshot(snapshot, |snapshot| {
                snapshot.command_generation = snapshot.command_generation.saturating_add(1);
                snapshot.bootstrap = BootstrapState::Ready;
                snapshot.loaded_config = Some(loaded_config);
                snapshot.runtime = client.state().clone();
            });
            Some(client)
        }
        Err(error) => {
            tracing::error!(error = %error, "Application bootstrap failed");
            update_snapshot(snapshot, |snapshot| {
                snapshot.command_generation = snapshot.command_generation.saturating_add(1);
                snapshot.bootstrap = BootstrapState::Failed;
                snapshot.bootstrap_error = Some(error.to_string());
                snapshot.loaded_config = None;
                snapshot.runtime = RuntimeState::default();
            });
            None
        }
    }
}

fn production_bootstrap() -> io::Result<(BridgeClient, LoadedClientConfig)> {
    let loaded_config = load_client_config();
    let log_sink = Box::new(ClientLogFiles::new());
    let client = BridgeClient::with_config_and_log_sink(loaded_config.clone(), log_sink);
    Ok((client, loaded_config))
}

fn handle_command(
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

fn send_application_event(
    event_tx: &mpsc::Sender<ApplicationEvent>,
    snapshot: &Arc<SnapshotStore>,
    event: ApplicationEvent,
) {
    let _ = event_tx.send(event);
    (snapshot.wake)();
}

fn set_operation(
    snapshot: &Arc<SnapshotStore>,
    client: &BridgeClient,
    operation: Option<ApplicationOperation>,
) {
    update_snapshot(snapshot, |snapshot| {
        if snapshot.operation != operation {
            snapshot.command_generation = snapshot.command_generation.saturating_add(1);
        }
        snapshot.operation = operation;
        snapshot.runtime = client.state().clone();
    });
}

fn command_is_current(snapshot: &Arc<SnapshotStore>, command: &QueuedCommand) -> bool {
    let snapshot = lock(&snapshot.value);
    snapshot.bootstrap == BootstrapState::Ready
        && snapshot.operation.is_none()
        && !snapshot.shutdown_complete
        && snapshot.command_generation == command.command_generation
}

fn failed_bootstrap_command_is_current(
    snapshot: &Arc<SnapshotStore>,
    command: &QueuedCommand,
) -> bool {
    let snapshot = lock(&snapshot.value);
    snapshot.bootstrap == BootstrapState::Failed
        && snapshot.operation.is_none()
        && !snapshot.shutdown_complete
        && snapshot.command_generation == command.command_generation
}

fn publish_client(snapshot: &Arc<SnapshotStore>, client: &BridgeClient) {
    update_snapshot(snapshot, |snapshot| {
        snapshot.runtime = client.state().clone();
    });
}

fn update_snapshot(snapshot: &Arc<SnapshotStore>, update: impl FnOnce(&mut ApplicationSnapshot)) {
    {
        update(&mut lock(&snapshot.value));
    }
    (snapshot.wake)();
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
#[path = "application_tests.rs"]
mod tests;
