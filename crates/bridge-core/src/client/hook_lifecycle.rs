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
        RuntimeEvent, RuntimeEventSender, SessionStopReason, error_counter, log_event, send_event,
    },
};

const INJECTOR_WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const PROCESS_WATCH_INTERVAL: Duration = Duration::from_secs(1);
const HOOK_ENDPOINT_WAIT_TIMEOUT: Duration = Duration::from_secs(3);
const HOOK_ENDPOINT_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(super) async fn injector_task(
    paths: basement_isaac_injector::NativeHookPaths,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) {
    send_event(
        &event_tx,
        log_event(LogLevel::Info, "Waiting for Isaac process"),
    );

    let Some(process) = wait_for_isaac_process(&cancellation).await else {
        send_event(
            &event_tx,
            log_event(LogLevel::Info, "Native Hook injection cancelled"),
        );
        return;
    };

    let process_name = process.name.clone();
    let process_id = process.pid;
    let hook_path = paths.hook.clone();
    let injection = tokio::task::spawn_blocking(move || {
        basement_isaac_injector::run_injector(&paths, process_id)
    });

    tokio::select! {
        () = cancellation.cancelled() => {
            send_event(&event_tx, log_event(LogLevel::Info, "Native Hook injection cancelled"));
        }
        result = time::timeout(INJECTOR_WAIT_TIMEOUT, injection) => {
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
                    let message = "Native Hook injection timed out".to_owned();
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
                run_hook_startup_preflight(
                    hook_path.with_file_name(crate::diagnostics::BRIDGE_HOOK_LOG),
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
    event_tx: &RuntimeEventSender,
    cancellation: &CancellationToken,
) {
    match wait_for_hook_receive_endpoint(cancellation).await {
        Ok(()) => send_event(
            event_tx,
            log_event(
                LogLevel::Info,
                format!("Hook receive endpoint is ready at {HOOK_OUT}/UDP"),
            ),
        ),
        Err(error) => {
            let message = format!("Hook receive endpoint preflight failed: {error}");
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
                send_event(event_tx, log_event(LogLevel::Info, report.to_string()));
                send_event(event_tx, RuntimeEvent::HookReceiveProbeFinished(Ok(report)));
            }
            Ok(Err(error)) => {
                let message = format!(
                    "Hook receive probe warning: {error}; {HOOK_OUT}/UDP is bound, continuing because this probe may be log-sampling sensitive"
                );
                send_event(event_tx, log_event(LogLevel::Warn, message.clone()));
                send_event(event_tx, RuntimeEvent::HookReceiveProbeFinished(Err(message)));
            }
            Err(error) => {
                let message = format!(
                    "Hook receive probe warning: task failed: {error}; {HOOK_OUT}/UDP is bound"
                );
                send_event(event_tx, log_event(LogLevel::Warn, message.clone()));
                send_event(event_tx, RuntimeEvent::HookReceiveProbeFinished(Err(message)));
            }
        }
    }
}

async fn wait_for_hook_receive_endpoint(cancellation: &CancellationToken) -> io::Result<()> {
    let started = Instant::now();
    loop {
        if let Err(error) = verify_hook_receive_endpoint_bound() {
            if started.elapsed() >= HOOK_ENDPOINT_WAIT_TIMEOUT {
                return Err(error);
            }
        } else {
            return Ok(());
        }
        tokio::select! {
            () = cancellation.cancelled() => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Hook receive endpoint preflight cancelled",
                ));
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
    cancellation: &CancellationToken,
) -> Option<basement_isaac_injector::IsaacProcess> {
    let wait = async {
        loop {
            if let Some(process) = basement_isaac_injector::find_isaac_process() {
                return Some(process);
            }
            time::sleep(Duration::from_millis(250)).await;
        }
    };

    tokio::select! {
        () = cancellation.cancelled() => None,
        result = time::timeout(INJECTOR_WAIT_TIMEOUT, wait) => result.unwrap_or(None),
    }
}

fn injection_support_message(error: &basement_isaac_injector::InjectorError) -> String {
    let message = error.to_string();
    if is_access_denied(&message) {
        format!(
            "{message}; access denied usually means Bridge GUI, Steam, and Isaac need matching privilege levels or security software allowed the helper"
        )
    } else {
        message
    }
}

fn is_access_denied(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("access is denied")
        || lower.contains("access denied")
        || lower.contains("os error 5")
}
