use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

use tokio::time;
use tokio_util::sync::CancellationToken;

use super::{
    LogLevel,
    hook_config::HOOK_OUT,
    probe,
    state::{
        HookStartupPhase, HookStartupState, RuntimeEvent, RuntimeEventSender, SessionStopReason,
        error_counter, log_event, send_event, unix_seconds,
    },
};

const INJECTOR_HELPER_TIMEOUT: Duration = Duration::from_secs(60);
const ISAAC_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(250);
const ISAAC_WAIT_NOTICE_INTERVAL: Duration = Duration::from_secs(60);
const PROCESS_WATCH_INTERVAL: Duration = Duration::from_secs(1);
const HOOK_ENDPOINT_WAIT_TIMEOUT: Duration = Duration::from_secs(35);
const HOOK_ENDPOINT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HOOK_ENDPOINT_WAIT_NOTICE_INTERVAL: Duration = Duration::from_secs(5);

enum HookEndpointWaitError {
    Cancelled,
    NotReady(io::Error),
}

pub(super) async fn injector_task(
    paths: basement_isaac_injector::NativeHookPaths,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) {
    send_event(
        &event_tx,
        log_event(LogLevel::Info, "Waiting for Isaac process"),
    );
    send_event(
        &event_tx,
        RuntimeEvent::HookStartup(Box::new(hook_startup_state(
            HookStartupPhase::WaitingForIsaac,
            &paths,
            None,
            None,
            "Waiting for Isaac process",
        ))),
    );
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

    let Some(process) = wait_for_isaac_process(&paths, &event_tx, &cancellation).await else {
        send_event(
            &event_tx,
            RuntimeEvent::HookStartup(Box::new(hook_startup_state(
                HookStartupPhase::Cancelled,
                &paths,
                None,
                None,
                "Native Hook injection cancelled while waiting for Isaac",
            ))),
        );
        send_event(
            &event_tx,
            log_event(LogLevel::Info, "Native Hook injection cancelled"),
        );
        return;
    };

    let process_name = process.name.clone();
    let process_id = process.pid;
    let hook_path = paths.hook.clone();
    send_event(
        &event_tx,
        RuntimeEvent::HookStartup(Box::new(hook_startup_state(
            HookStartupPhase::Injecting,
            &paths,
            Some(&process_name),
            Some(process_id),
            "Isaac process found; starting Native Hook injector helper",
        ))),
    );
    send_event(
        &event_tx,
        log_event(
            LogLevel::Info,
            format!(
                "Starting Native Hook injector helper for {process_name} ({process_id}): injector={} hook={}",
                paths.injector.display(),
                hook_path.display()
            ),
        ),
    );
    let injection_paths = paths.clone();
    let injection = tokio::task::spawn_blocking(move || {
        basement_isaac_injector::run_injector(&injection_paths, process_id)
    });

    tokio::select! {
        () = cancellation.cancelled() => {
            send_event(
                &event_tx,
                RuntimeEvent::HookStartup(Box::new(hook_startup_state(
                    HookStartupPhase::Cancelled,
                    &paths,
                    Some(&process_name),
                    Some(process_id),
                    "Native Hook injection cancelled while injector helper was running",
                ))),
            );
            send_event(&event_tx, log_event(LogLevel::Info, "Native Hook injection cancelled"));
        }
        result = time::timeout(INJECTOR_HELPER_TIMEOUT, injection) => {
            let mut failure_message = None;
            let injected = match result {
                Ok(Ok(Ok(()))) => {
                    send_event(
                        &event_tx,
                        log_event(
                            LogLevel::Info,
                            format!(
                                "Native Hook injected into {process_name} ({process_id}) from {}",
                                hook_path.display()
                            ),
                        ),
                    );
                    true
                }
                Ok(Ok(Err(error))) => {
                    let message = format!(
                        "Native Hook injection failed: {}",
                        injection_support_message(&error)
                    );
                    let mut state = hook_startup_state(
                        HookStartupPhase::Failed,
                        &paths,
                        Some(&process_name),
                        Some(process_id),
                        message.clone(),
                    );
                    state.access_denied = error.is_access_denied();
                    send_event(&event_tx, RuntimeEvent::HookStartup(Box::new(state)));
                    send_event(&event_tx, log_event(LogLevel::Error, message.clone()));
                    send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
                    failure_message = Some(message);
                    false
                }
                Ok(Err(error)) => {
                    let message = format!("Native Hook injection task failed: {error}");
                    send_event(&event_tx, log_event(LogLevel::Error, message.clone()));
                    send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
                    failure_message = Some(message);
                    false
                }
                Err(_) => {
                    let message = format!(
                        "Native Hook injector helper timed out after {} seconds",
                        INJECTOR_HELPER_TIMEOUT.as_secs()
                    );
                    send_event(
                        &event_tx,
                        RuntimeEvent::HookStartup(Box::new(hook_startup_state(
                            HookStartupPhase::Failed,
                            &paths,
                            Some(&process_name),
                            Some(process_id),
                            message.clone(),
                        ))),
                    );
                    send_event(&event_tx, log_event(LogLevel::Error, message.clone()));
                    send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
                    failure_message = Some(message);
                    false
                }
            };
            if let Some(message) = failure_message {
                send_event(
                    &event_tx,
                    RuntimeEvent::HookReceiveProbeFinished(Err(message.clone())),
                );
                send_event(
                    &event_tx,
                    RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded { message }),
                );
                cancellation.cancel();
            }
            if injected {
                let mut state = hook_startup_state(
                    HookStartupPhase::WaitingForHookEndpoint,
                    &paths,
                    Some(&process_name),
                    Some(process_id),
                    format!(
                        "Injection succeeded; waiting up to {} seconds for Hook receive endpoint",
                        HOOK_ENDPOINT_WAIT_TIMEOUT.as_secs()
                    ),
                );
                state.injected = true;
                send_event(&event_tx, RuntimeEvent::HookStartup(Box::new(state)));
                run_hook_startup_preflight(
                    hook_path.with_file_name(crate::diagnostics::BRIDGE_HOOK_LOG),
                    &paths,
                    &process_name,
                    process_id,
                    &event_tx,
                    &cancellation,
                )
                .await;
                watch_isaac_process(process_name, process_id, &event_tx, &cancellation).await;
            }
        }
    }
}

async fn run_hook_startup_preflight(
    hook_log_path: PathBuf,
    paths: &basement_isaac_injector::NativeHookPaths,
    process_name: &str,
    process_id: u32,
    event_tx: &RuntimeEventSender,
    cancellation: &CancellationToken,
) {
    match wait_for_hook_receive_endpoint(paths, process_name, process_id, event_tx, cancellation)
        .await
    {
        Ok(()) => {
            let mut state = hook_startup_state(
                HookStartupPhase::EndpointReady,
                paths,
                Some(process_name),
                Some(process_id),
                format!("Hook receive endpoint is ready at {HOOK_OUT}/UDP"),
            );
            state.injected = true;
            state.endpoint_ready = true;
            send_event(event_tx, RuntimeEvent::HookStartup(Box::new(state)));
            send_event(
                event_tx,
                log_event(
                    LogLevel::Info,
                    format!("Hook receive endpoint is ready at {HOOK_OUT}/UDP"),
                ),
            );
        }
        Err(HookEndpointWaitError::Cancelled) => {
            let mut state = hook_startup_state(
                HookStartupPhase::Cancelled,
                paths,
                Some(process_name),
                Some(process_id),
                "Hook receive endpoint wait cancelled",
            );
            state.injected = true;
            send_event(event_tx, RuntimeEvent::HookStartup(Box::new(state)));
            send_event(
                event_tx,
                log_event(LogLevel::Info, "Hook receive endpoint wait cancelled"),
            );
            return;
        }
        Err(HookEndpointWaitError::NotReady(error)) => {
            let message = format!(
                "Hook receive endpoint did not become ready within {} seconds after injection: {error}",
                HOOK_ENDPOINT_WAIT_TIMEOUT.as_secs()
            );
            let mut state = hook_startup_state(
                HookStartupPhase::Failed,
                paths,
                Some(process_name),
                Some(process_id),
                message.clone(),
            );
            state.injected = true;
            send_event(event_tx, RuntimeEvent::HookStartup(Box::new(state)));
            send_event(event_tx, log_event(LogLevel::Error, message.clone()));
            send_event(
                event_tx,
                RuntimeEvent::HookReceiveProbeFinished(Err(message.clone())),
            );
            send_event(
                event_tx,
                RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded { message }),
            );
            send_event(event_tx, RuntimeEvent::CounterDelta(error_counter()));
            cancellation.cancel();
            return;
        }
    }
    send_event(
        event_tx,
        log_event(
            LogLevel::Info,
            format!(
                "Running Hook startup preflight with {}",
                hook_log_path.display()
            ),
        ),
    );
    let probe =
        tokio::task::spawn_blocking(move || probe::run_hook_receive_probe(Some(hook_log_path)));
    tokio::select! {
        () = cancellation.cancelled() => {}
        result = probe => match result {
            Ok(Ok(report)) => {
                let mut state = hook_startup_state(
                    HookStartupPhase::Ready,
                    paths,
                    Some(process_name),
                    Some(process_id),
                    "Hook startup ready; Hook receive probe succeeded",
                );
                state.injected = true;
                state.endpoint_ready = true;
                send_event(event_tx, RuntimeEvent::HookStartup(Box::new(state)));
                send_event(event_tx, log_event(LogLevel::Info, report.to_string()));
                send_event(event_tx, RuntimeEvent::HookReceiveProbeFinished(Ok(report)));
            }
            Ok(Err(error)) => {
                let message = format!(
                    "Hook receive probe warning: {error}; {HOOK_OUT}/UDP is bound, continuing because this probe may be log-sampling sensitive"
                );
                let mut state = hook_startup_state(
                    HookStartupPhase::Ready,
                    paths,
                    Some(process_name),
                    Some(process_id),
                    message.clone(),
                );
                state.injected = true;
                state.endpoint_ready = true;
                send_event(event_tx, RuntimeEvent::HookStartup(Box::new(state)));
                send_event(event_tx, log_event(LogLevel::Warn, message.clone()));
                send_event(event_tx, RuntimeEvent::HookReceiveProbeWarning(message));
            }
            Err(error) => {
                let message = format!(
                    "Hook receive probe warning: task failed: {error}; {HOOK_OUT}/UDP is bound"
                );
                let mut state = hook_startup_state(
                    HookStartupPhase::Ready,
                    paths,
                    Some(process_name),
                    Some(process_id),
                    message.clone(),
                );
                state.injected = true;
                state.endpoint_ready = true;
                send_event(event_tx, RuntimeEvent::HookStartup(Box::new(state)));
                send_event(event_tx, log_event(LogLevel::Warn, message.clone()));
                send_event(event_tx, RuntimeEvent::HookReceiveProbeWarning(message));
            }
        }
    }
}

async fn wait_for_hook_receive_endpoint(
    paths: &basement_isaac_injector::NativeHookPaths,
    process_name: &str,
    process_id: u32,
    event_tx: &RuntimeEventSender,
    cancellation: &CancellationToken,
) -> Result<(), HookEndpointWaitError> {
    let started = Instant::now();
    let mut next_notice = HOOK_ENDPOINT_WAIT_NOTICE_INTERVAL;
    loop {
        if let Err(error) = verify_hook_receive_endpoint_bound() {
            if started.elapsed() >= HOOK_ENDPOINT_WAIT_TIMEOUT {
                return Err(HookEndpointWaitError::NotReady(error));
            }
            if started.elapsed() >= next_notice {
                let elapsed_seconds = started.elapsed().as_secs();
                let mut state = hook_startup_state(
                    HookStartupPhase::WaitingForHookEndpoint,
                    paths,
                    Some(process_name),
                    Some(process_id),
                    format!(
                        "Still waiting for Hook receive endpoint at {HOOK_OUT}/UDP after {elapsed_seconds} seconds"
                    ),
                );
                state.injected = true;
                send_event(event_tx, RuntimeEvent::HookStartup(Box::new(state)));
                send_event(
                    event_tx,
                    log_event(
                        LogLevel::Info,
                        format!(
                            "Still waiting for Hook receive endpoint at {HOOK_OUT}/UDP after {elapsed_seconds} seconds"
                        ),
                    ),
                );
                next_notice += HOOK_ENDPOINT_WAIT_NOTICE_INTERVAL;
            }
        } else {
            return Ok(());
        }
        tokio::select! {
            () = cancellation.cancelled() => {
                return Err(HookEndpointWaitError::Cancelled);
            }
            () = time::sleep(HOOK_ENDPOINT_POLL_INTERVAL) => {}
        }
    }
}

fn verify_hook_receive_endpoint_bound() -> io::Result<()> {
    match std::net::UdpSocket::bind(HOOK_OUT) {
        Ok(socket) => {
            drop(socket);
            Err(io::Error::other(format!(
                "Hook receive endpoint {HOOK_OUT}/UDP is not occupied by Native Hook"
            )))
        }
        Err(error) if is_address_in_use(&error) => Ok(()),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("could not verify Hook receive endpoint {HOOK_OUT}/UDP: {error}"),
        )),
    }
}

fn is_address_in_use(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::AddrInUse || error.raw_os_error() == Some(10_048)
}

async fn watch_isaac_process(
    process_name: String,
    process_id: u32,
    event_tx: &RuntimeEventSender,
    cancellation: &CancellationToken,
) {
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return,
            () = time::sleep(PROCESS_WATCH_INTERVAL) => {
                let running = tokio::task::spawn_blocking(move || {
                    basement_isaac_injector::is_process_running(process_id)
                })
                .await
                .unwrap_or(false);
                if !running {
                    let reason = SessionStopReason::GameExited {
                        process_name,
                        pid: process_id,
                    };
                    send_event(event_tx, RuntimeEvent::SessionEnded(reason.clone()));
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

async fn wait_for_isaac_process(
    paths: &basement_isaac_injector::NativeHookPaths,
    event_tx: &RuntimeEventSender,
    cancellation: &CancellationToken,
) -> Option<basement_isaac_injector::IsaacProcess> {
    let started = Instant::now();
    let mut next_notice = ISAAC_WAIT_NOTICE_INTERVAL;
    loop {
        if let Some(process) = basement_isaac_injector::find_isaac_process() {
            return Some(process);
        }
        if started.elapsed() >= next_notice {
            let elapsed_seconds = started.elapsed().as_secs();
            send_event(
                event_tx,
                RuntimeEvent::HookStartup(Box::new(hook_startup_state(
                    HookStartupPhase::WaitingForIsaac,
                    paths,
                    None,
                    None,
                    format!("Still waiting for Isaac process after {elapsed_seconds} seconds"),
                ))),
            );
            send_event(
                event_tx,
                log_event(
                    LogLevel::Info,
                    format!("Still waiting for Isaac process after {elapsed_seconds} seconds"),
                ),
            );
            next_notice += ISAAC_WAIT_NOTICE_INTERVAL;
        }

        tokio::select! {
            () = cancellation.cancelled() => return None,
            () = time::sleep(ISAAC_PROCESS_POLL_INTERVAL) => {}
        }
    }
}

fn injection_support_message(error: &basement_isaac_injector::InjectorError) -> String {
    let message = error.to_string();
    if error.is_access_denied() {
        format!(
            "{message}; access denied usually means Bridge GUI, Steam, and Isaac need matching privilege levels or security software allowed the helper"
        )
    } else {
        message
    }
}

fn hook_startup_state(
    phase: HookStartupPhase,
    paths: &basement_isaac_injector::NativeHookPaths,
    process_name: Option<&str>,
    pid: Option<u32>,
    message: impl Into<String>,
) -> HookStartupState {
    HookStartupState {
        phase,
        process_name: process_name.map(ToOwned::to_owned),
        pid,
        injector_path: (!paths.injector.as_os_str().is_empty()).then(|| paths.injector.clone()),
        hook_path: (!paths.hook.as_os_str().is_empty()).then(|| paths.hook.clone()),
        endpoint: Some(format!("{HOOK_OUT}/UDP")),
        message: Some(message.into()),
        updated_at: unix_seconds(),
        ..HookStartupState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use basement_isaac_injector::{InjectionStep, InjectorError, NativeHookPaths};

    #[test]
    fn endpoint_wait_budget_covers_native_hook_startup_window() {
        assert!(HOOK_ENDPOINT_WAIT_TIMEOUT > Duration::from_secs(30));
        assert_eq!(INJECTOR_HELPER_TIMEOUT, Duration::from_secs(60));
    }

    #[test]
    fn access_denied_support_message_uses_injector_category() {
        let error =
            InjectorError::step_io(InjectionStep::OpenProcess, io::Error::from_raw_os_error(5));

        let message = injection_support_message(&error);

        assert!(message.contains("access denied"));
        assert!(message.contains("matching privilege levels"));
    }

    #[test]
    fn startup_state_carries_artifact_paths_and_endpoint() {
        let paths = NativeHookPaths {
            injector: PathBuf::from("bundle/basement-isaac-injector.exe"),
            hook: PathBuf::from("bundle/native-hook/basement_native_hook.dll"),
        };

        let state = hook_startup_state(
            HookStartupPhase::Injecting,
            &paths,
            Some("isaac-ng.exe"),
            Some(42),
            "starting helper",
        );

        assert_eq!(state.phase, HookStartupPhase::Injecting);
        assert_eq!(state.process_name.as_deref(), Some("isaac-ng.exe"));
        assert_eq!(state.pid, Some(42));
        assert_eq!(state.injector_path.as_ref(), Some(&paths.injector));
        assert_eq!(state.hook_path.as_ref(), Some(&paths.hook));
        assert_eq!(state.endpoint.as_deref(), Some("127.0.0.1:25901/UDP"));
    }
}
