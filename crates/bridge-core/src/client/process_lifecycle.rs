use std::{
    io,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::time;
use tokio_util::sync::CancellationToken;
use tractor_beam_isaac_injector::{IsaacProcess, NativeHookPaths};

use super::{
    HookStartupPhase, LogLevel, hook_lifecycle,
    state::{
        RuntimeEvent, RuntimeEventSender, SessionStopReason, error_counter, log_event,
        send_critical_event, send_event,
    },
};

const PROCESS_WAIT_TIMEOUT: Duration = Duration::from_secs(120);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(250);
const PROCESS_WAIT_NOTICE_INTERVAL: Duration = Duration::from_secs(30);
const PROCESS_WATCH_INTERVAL: Duration = Duration::from_secs(1);
const PREEXISTING_PROCESS_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct ProcessLifecycleSettings {
    wait_timeout: Duration,
    poll_interval: Duration,
    wait_notice_interval: Duration,
    watch_interval: Duration,
    preexisting_process_grace: Duration,
}

impl Default for ProcessLifecycleSettings {
    fn default() -> Self {
        Self {
            wait_timeout: PROCESS_WAIT_TIMEOUT,
            poll_interval: PROCESS_POLL_INTERVAL,
            wait_notice_interval: PROCESS_WAIT_NOTICE_INTERVAL,
            watch_interval: PROCESS_WATCH_INTERVAL,
            preexisting_process_grace: PREEXISTING_PROCESS_GRACE,
        }
    }
}

trait IsaacProcessService: Send + Sync {
    fn find_all(&self) -> Vec<IsaacProcess>;
    fn is_running(&self, process: &IsaacProcess) -> bool;
}

struct SystemIsaacProcesses;

impl IsaacProcessService for SystemIsaacProcesses {
    fn find_all(&self) -> Vec<IsaacProcess> {
        tractor_beam_isaac_injector::find_isaac_processes()
    }

    fn is_running(&self, process: &IsaacProcess) -> bool {
        tractor_beam_isaac_injector::is_process_running(process)
    }
}

enum ProcessWait {
    Bound {
        process: IsaacProcess,
        source: ProcessBindingSource,
        candidate_count: usize,
    },
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy)]
enum ProcessBindingSource {
    NewLaunch,
    PreexistingFallback,
}

impl std::fmt::Display for ProcessBindingSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewLaunch => formatter.write_str("new_launch"),
            Self::PreexistingFallback => formatter.write_str("preexisting_fallback"),
        }
    }
}

pub(super) async fn run(
    hook_paths: Option<NativeHookPaths>,
    preexisting_processes: Vec<IsaacProcess>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    ready_deadline: Option<super::hook_ipc::HookReadyDeadline>,
) -> io::Result<()> {
    run_with(
        hook_paths,
        event_tx,
        cancellation,
        ready_deadline,
        Arc::new(SystemIsaacProcesses),
        ProcessLifecycleSettings::default(),
        preexisting_processes,
    )
    .await;
    Ok(())
}

async fn run_with(
    hook_paths: Option<NativeHookPaths>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    ready_deadline: Option<super::hook_ipc::HookReadyDeadline>,
    processes: Arc<dyn IsaacProcessService>,
    settings: ProcessLifecycleSettings,
    preexisting_processes: Vec<IsaacProcess>,
) {
    send_event(
        &event_tx,
        log_event(LogLevel::Info, "Waiting for Isaac process"),
    );
    send_event(
        &event_tx,
        log_event(
            LogLevel::Info,
            format!(
                "Isaac process baseline count={} pids={}",
                preexisting_processes.len(),
                process_ids(&preexisting_processes)
            ),
        ),
    );
    if let Some(paths) = &hook_paths {
        hook_lifecycle::report_waiting_for_isaac(paths, &event_tx, "Waiting for Isaac process");
        send_event(
            &event_tx,
            log_event(
                LogLevel::Info,
                format!(
                    "Native Hook artifacts: injector={} hook={}",
                    paths.injector.display(),
                    paths.hook.display()
                ),
            ),
        );
    }

    let process = match wait_for_process(
        Arc::clone(&processes),
        &event_tx,
        &cancellation,
        hook_paths.as_ref(),
        settings,
        &preexisting_processes,
    )
    .await
    {
        ProcessWait::Bound {
            process,
            source,
            candidate_count,
        } => {
            send_event(
                &event_tx,
                log_event(
                    LogLevel::Info,
                    format!(
                        "Isaac process selected source={source} pid={} started_at={} candidates={candidate_count}",
                        process.pid, process.started_at
                    ),
                ),
            );
            process
        }
        ProcessWait::Cancelled => {
            if let Some(paths) = &hook_paths {
                hook_lifecycle::report_isaac_wait_failure(
                    paths,
                    &event_tx,
                    HookStartupPhase::Cancelled,
                    "Native Hook injection cancelled while waiting for Isaac",
                );
            }
            send_event(
                &event_tx,
                log_event(LogLevel::Info, "Isaac process wait cancelled"),
            );
            return;
        }
        ProcessWait::TimedOut => {
            let message = format!(
                "Isaac process was not found within {} seconds",
                settings.wait_timeout.as_secs()
            );
            if let Some(paths) = &hook_paths {
                hook_lifecycle::report_isaac_wait_failure(
                    paths,
                    &event_tx,
                    HookStartupPhase::Failed,
                    message.clone(),
                );
            }
            send_event(&event_tx, log_event(LogLevel::Error, message.clone()));
            send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
            send_critical_event(
                &event_tx,
                RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded { message }),
            )
            .await;
            cancellation.cancel();
            return;
        }
    };

    send_event(
        &event_tx,
        log_event(
            LogLevel::Info,
            format!(
                "Isaac process found; monitoring {} ({})",
                process.name, process.pid
            ),
        ),
    );
    if let Some(paths) = hook_paths {
        let ready_deadline = ready_deadline.expect("Native Hook readiness deadline is available");
        hook_lifecycle::inject_process(
            paths,
            process.clone(),
            event_tx.clone(),
            cancellation.clone(),
            ready_deadline,
        )
        .await;
    }
    if cancellation.is_cancelled() {
        return;
    }
    watch_process(processes, process, &event_tx, &cancellation, settings).await;
}

async fn wait_for_process(
    processes: Arc<dyn IsaacProcessService>,
    event_tx: &RuntimeEventSender,
    cancellation: &CancellationToken,
    hook_paths: Option<&NativeHookPaths>,
    settings: ProcessLifecycleSettings,
    preexisting_processes: &[IsaacProcess],
) -> ProcessWait {
    let started = Instant::now();
    let mut next_notice = settings.wait_notice_interval;
    loop {
        if cancellation.is_cancelled() {
            return ProcessWait::Cancelled;
        }
        if started.elapsed() >= settings.wait_timeout {
            return ProcessWait::TimedOut;
        }
        let finder = Arc::clone(&processes);
        if let Ok(candidates) = tokio::task::spawn_blocking(move || finder.find_all()).await {
            let candidate_count = candidates.len();
            if let Some(process) = newest_process(
                candidates
                    .iter()
                    .filter(|candidate| !preexisting_processes.contains(candidate)),
            ) {
                return ProcessWait::Bound {
                    process,
                    source: ProcessBindingSource::NewLaunch,
                    candidate_count,
                };
            }
            if started.elapsed() >= settings.preexisting_process_grace
                && let Some(process) = newest_process(
                    candidates
                        .iter()
                        .filter(|candidate| preexisting_processes.contains(candidate)),
                )
            {
                return ProcessWait::Bound {
                    process,
                    source: ProcessBindingSource::PreexistingFallback,
                    candidate_count,
                };
            }
        }
        if started.elapsed() >= next_notice {
            let elapsed_seconds = started.elapsed().as_secs();
            let message =
                format!("Still waiting for Isaac process after {elapsed_seconds} seconds");
            if let Some(paths) = hook_paths {
                hook_lifecycle::report_waiting_for_isaac(paths, event_tx, message.clone());
            }
            send_event(event_tx, log_event(LogLevel::Info, message));
            next_notice += settings.wait_notice_interval;
        }

        tokio::select! {
            () = cancellation.cancelled() => return ProcessWait::Cancelled,
            () = time::sleep(settings.poll_interval) => {}
        }
    }
}

fn newest_process<'a>(processes: impl Iterator<Item = &'a IsaacProcess>) -> Option<IsaacProcess> {
    processes
        .max_by_key(|process| (process.started_at, process.pid))
        .cloned()
}

fn process_ids(processes: &[IsaacProcess]) -> String {
    let ids = processes
        .iter()
        .map(|process| process.pid.to_string())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        "none".to_owned()
    } else {
        ids.join(",")
    }
}

async fn watch_process(
    processes: Arc<dyn IsaacProcessService>,
    process: IsaacProcess,
    event_tx: &RuntimeEventSender,
    cancellation: &CancellationToken,
    settings: ProcessLifecycleSettings,
) {
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return,
            () = time::sleep(settings.watch_interval) => {
                let monitor = Arc::clone(&processes);
                let expected = process.clone();
                let running = tokio::task::spawn_blocking(move || {
                    monitor.is_running(&expected)
                })
                .await
                .unwrap_or(false);
                if !running {
                    let reason = SessionStopReason::GameExited {
                        process_name: process.name.clone(),
                        pid: process.pid,
                    };
                    send_critical_event(event_tx, RuntimeEvent::SessionEnded(reason.clone())).await;
                    send_event(
                        event_tx,
                        log_event(LogLevel::Warn, format!("Session ended: {reason}")),
                    );
                    cancellation.cancel();
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;

    struct FakeIsaacProcesses {
        found: Mutex<VecDeque<Vec<IsaacProcess>>>,
        running: Mutex<VecDeque<bool>>,
        observed: Mutex<Vec<IsaacProcess>>,
        find_calls: AtomicUsize,
    }

    impl FakeIsaacProcesses {
        fn new(found: Vec<Vec<IsaacProcess>>, running: Vec<bool>) -> Self {
            Self {
                found: Mutex::new(found.into()),
                running: Mutex::new(running.into()),
                observed: Mutex::new(Vec::new()),
                find_calls: AtomicUsize::new(0),
            }
        }
    }

    impl IsaacProcessService for FakeIsaacProcesses {
        fn find_all(&self) -> Vec<IsaacProcess> {
            self.find_calls.fetch_add(1, Ordering::SeqCst);
            self.found.lock().unwrap().pop_front().unwrap_or_default()
        }

        fn is_running(&self, process: &IsaacProcess) -> bool {
            self.observed.lock().unwrap().push(process.clone());
            self.running.lock().unwrap().pop_front().unwrap_or(false)
        }
    }

    #[tokio::test]
    async fn exact_bound_process_exit_is_terminal_and_does_not_bind_a_relaunch() {
        let bound = process(42, 100);
        let later = process(42, 101);
        let processes = Arc::new(FakeIsaacProcesses::new(
            vec![vec![bound.clone()], vec![later]],
            vec![false],
        ));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let cancellation = CancellationToken::new();

        run_with(
            None,
            event_tx,
            cancellation.clone(),
            None,
            processes.clone(),
            fast_settings(Duration::from_secs(1)),
            Vec::new(),
        )
        .await;

        assert!(cancellation.is_cancelled());
        assert_eq!(processes.find_calls.load(Ordering::SeqCst), 1);
        assert_eq!(*processes.observed.lock().unwrap(), vec![bound.clone()]);
        assert!(received(&mut event_rx, |event| {
            matches!(
                event,
                RuntimeEvent::SessionEnded(SessionStopReason::GameExited {
                    process_name,
                    pid: 42,
                }) if process_name == &bound.name
            )
        }));
    }

    #[tokio::test]
    async fn cancellation_while_waiting_does_not_report_a_terminal_failure() {
        let processes = Arc::new(FakeIsaacProcesses::new(vec![], vec![]));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        run_with(
            None,
            event_tx,
            cancellation,
            None,
            processes.clone(),
            fast_settings(Duration::from_secs(1)),
            Vec::new(),
        )
        .await;

        assert_eq!(processes.find_calls.load(Ordering::SeqCst), 0);
        assert!(!received(&mut event_rx, |event| {
            matches!(event, RuntimeEvent::SessionEnded(_))
        }));
    }

    #[tokio::test]
    async fn process_discovery_timeout_is_a_terminal_failure() {
        let processes = Arc::new(FakeIsaacProcesses::new(vec![], vec![]));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let cancellation = CancellationToken::new();

        run_with(
            None,
            event_tx,
            cancellation.clone(),
            None,
            processes,
            fast_settings(Duration::from_millis(5)),
            Vec::new(),
        )
        .await;

        assert!(cancellation.is_cancelled());
        assert!(received(&mut event_rx, |event| {
            matches!(
                event,
                RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded { message })
                    if message.contains("not found")
            )
        }));
    }

    #[tokio::test]
    async fn new_launch_process_wins_over_preexisting_residue() {
        let residue = process(17_948, 100);
        let launched = process(12_348, 200);
        let processes = Arc::new(FakeIsaacProcesses::new(
            vec![
                vec![residue.clone()],
                vec![residue.clone(), launched.clone()],
            ],
            vec![false],
        ));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
        let cancellation = CancellationToken::new();
        let mut settings = fast_settings(Duration::from_secs(1));
        settings.preexisting_process_grace = Duration::from_millis(5);

        run_with(
            None,
            event_tx,
            cancellation.clone(),
            None,
            processes.clone(),
            settings,
            vec![residue],
        )
        .await;

        assert_eq!(*processes.observed.lock().unwrap(), vec![launched]);
        assert!(received(&mut event_rx, |event| {
            matches!(
                event,
                RuntimeEvent::Log(LogLevel::Info, message)
                    if message.contains("source=new_launch")
            )
        }));
    }

    #[tokio::test]
    async fn existing_game_is_used_only_after_new_launch_grace() {
        let existing = process(42, 100);
        let processes = Arc::new(FakeIsaacProcesses::new(
            vec![vec![existing.clone()]; 8],
            vec![false],
        ));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
        let cancellation = CancellationToken::new();
        let mut settings = fast_settings(Duration::from_secs(1));
        settings.preexisting_process_grace = Duration::from_millis(5);

        run_with(
            None,
            event_tx,
            cancellation.clone(),
            None,
            processes.clone(),
            settings,
            vec![existing.clone()],
        )
        .await;

        assert_eq!(*processes.observed.lock().unwrap(), vec![existing]);
        assert!(processes.find_calls.load(Ordering::SeqCst) > 1);
        assert!(received(&mut event_rx, |event| {
            matches!(
                event,
                RuntimeEvent::Log(LogLevel::Info, message)
                    if message.contains("source=preexisting_fallback")
            )
        }));
    }

    #[test]
    fn newest_process_is_deterministic() {
        let candidates = [process(7, 101), process(9, 101), process(42, 100)];

        assert_eq!(newest_process(candidates.iter()), Some(process(9, 101)));
    }

    fn fast_settings(wait_timeout: Duration) -> ProcessLifecycleSettings {
        ProcessLifecycleSettings {
            wait_timeout,
            poll_interval: Duration::from_millis(1),
            wait_notice_interval: Duration::from_secs(60),
            watch_interval: Duration::from_millis(1),
            preexisting_process_grace: Duration::ZERO,
        }
    }

    fn process(pid: u32, started_at: u64) -> IsaacProcess {
        IsaacProcess {
            pid,
            name: "isaac-ng.exe".to_owned(),
            started_at,
        }
    }

    fn received(
        event_rx: &mut tokio::sync::mpsc::Receiver<RuntimeEvent>,
        predicate: impl Fn(&RuntimeEvent) -> bool,
    ) -> bool {
        std::iter::from_fn(|| event_rx.try_recv().ok()).any(|event| predicate(&event))
    }
}
