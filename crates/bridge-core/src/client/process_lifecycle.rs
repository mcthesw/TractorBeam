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

#[derive(Clone, Copy)]
struct ProcessLifecycleSettings {
    wait_timeout: Duration,
    poll_interval: Duration,
    wait_notice_interval: Duration,
    watch_interval: Duration,
}

impl Default for ProcessLifecycleSettings {
    fn default() -> Self {
        Self {
            wait_timeout: PROCESS_WAIT_TIMEOUT,
            poll_interval: PROCESS_POLL_INTERVAL,
            wait_notice_interval: PROCESS_WAIT_NOTICE_INTERVAL,
            watch_interval: PROCESS_WATCH_INTERVAL,
        }
    }
}

trait IsaacProcessService: Send + Sync {
    fn find(&self) -> Option<IsaacProcess>;
    fn is_running(&self, process: &IsaacProcess) -> bool;
}

struct SystemIsaacProcesses;

impl IsaacProcessService for SystemIsaacProcesses {
    fn find(&self) -> Option<IsaacProcess> {
        tractor_beam_isaac_injector::find_isaac_process()
    }

    fn is_running(&self, process: &IsaacProcess) -> bool {
        tractor_beam_isaac_injector::is_process_running(process)
    }
}

enum ProcessWait {
    Bound(IsaacProcess),
    Cancelled,
    TimedOut,
}

pub(super) async fn run(
    hook_paths: Option<NativeHookPaths>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) -> io::Result<()> {
    run_with(
        hook_paths,
        event_tx,
        cancellation,
        Arc::new(SystemIsaacProcesses),
        ProcessLifecycleSettings::default(),
    )
    .await;
    Ok(())
}

async fn run_with(
    hook_paths: Option<NativeHookPaths>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    processes: Arc<dyn IsaacProcessService>,
    settings: ProcessLifecycleSettings,
) {
    send_event(
        &event_tx,
        log_event(LogLevel::Info, "Waiting for Isaac process"),
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
    )
    .await
    {
        ProcessWait::Bound(process) => process,
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
        hook_lifecycle::inject_process(
            paths,
            process.clone(),
            event_tx.clone(),
            cancellation.clone(),
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
        if let Ok(Some(process)) = tokio::task::spawn_blocking(move || finder.find()).await {
            return ProcessWait::Bound(process);
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
        found: Mutex<VecDeque<Option<IsaacProcess>>>,
        running: Mutex<VecDeque<bool>>,
        observed: Mutex<Vec<IsaacProcess>>,
        find_calls: AtomicUsize,
    }

    impl FakeIsaacProcesses {
        fn new(found: Vec<Option<IsaacProcess>>, running: Vec<bool>) -> Self {
            Self {
                found: Mutex::new(found.into()),
                running: Mutex::new(running.into()),
                observed: Mutex::new(Vec::new()),
                find_calls: AtomicUsize::new(0),
            }
        }
    }

    impl IsaacProcessService for FakeIsaacProcesses {
        fn find(&self) -> Option<IsaacProcess> {
            self.find_calls.fetch_add(1, Ordering::SeqCst);
            self.found.lock().unwrap().pop_front().flatten()
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
            vec![Some(bound.clone()), Some(later)],
            vec![false],
        ));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let cancellation = CancellationToken::new();

        run_with(
            None,
            event_tx,
            cancellation.clone(),
            processes.clone(),
            fast_settings(Duration::from_secs(1)),
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
            processes.clone(),
            fast_settings(Duration::from_secs(1)),
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
            processes,
            fast_settings(Duration::from_millis(5)),
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

    fn fast_settings(wait_timeout: Duration) -> ProcessLifecycleSettings {
        ProcessLifecycleSettings {
            wait_timeout,
            poll_interval: Duration::from_millis(1),
            wait_notice_interval: Duration::from_secs(60),
            watch_interval: Duration::from_millis(1),
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
