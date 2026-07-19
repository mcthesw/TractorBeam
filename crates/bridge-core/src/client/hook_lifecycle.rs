use std::time::Duration;

#[cfg(test)]
use std::io;

use tokio::time;
use tokio_util::sync::CancellationToken;

use super::{
    LogLevel,
    state::{
        HookStartupPhase, HookStartupState, RuntimeEvent, RuntimeEventSender, SessionStopReason,
        error_counter, log_event, send_critical_event, send_event, unix_seconds,
    },
};

const INJECTOR_HELPER_TIMEOUT: Duration = Duration::from_secs(60);
const ADMIN_PERMISSION_REQUEST_MESSAGE: &str = "Requesting admin permission...";
const ADMIN_PERMISSION_CANCELLED_MESSAGE: &str = "Admin permission was cancelled";
const ELEVATED_INJECTOR_RETRY_SUCCEEDED_MESSAGE: &str = "Elevated Injector retry succeeded";
const STALE_NATIVE_HOOK_MESSAGE: &str =
    "Native Hook is already loaded from an earlier session. Fully exit Isaac, then start it again.";

pub(super) async fn inject_process(
    paths: tractor_beam_isaac_injector::NativeHookPaths,
    process: tractor_beam_isaac_injector::IsaacProcess,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) {
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
    let retry_event_tx = event_tx.clone();
    let retry_paths = paths.clone();
    let retry_process_name = process_name.clone();
    let injection = tokio::task::spawn_blocking(move || {
        tractor_beam_isaac_injector::run_injector_with_elevated_retry(
            &injection_paths,
            process_id,
            |event| {
                send_injector_launch_event(
                    &retry_event_tx,
                    &retry_paths,
                    &retry_process_name,
                    process_id,
                    event,
                );
            },
        )
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
                send_critical_event(
                    &event_tx,
                    RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded { message }),
                )
                .await;
                cancellation.cancel();
            }
            if injected {
                let mut state = hook_startup_state(
                    HookStartupPhase::WaitingForHookEndpoint,
                    &paths,
                    Some(&process_name),
                    Some(process_id),
                    "Injection succeeded; waiting for local IPC handshake",
                );
                state.injected = true;
                send_event(&event_tx, RuntimeEvent::HookStartup(Box::new(state)));
            }
        }
    }
}

pub(super) fn report_waiting_for_isaac(
    paths: &tractor_beam_isaac_injector::NativeHookPaths,
    event_tx: &RuntimeEventSender,
    message: impl Into<String>,
) {
    send_event(
        event_tx,
        RuntimeEvent::HookStartup(Box::new(hook_startup_state(
            HookStartupPhase::WaitingForIsaac,
            paths,
            None,
            None,
            message,
        ))),
    );
}

pub(super) fn report_isaac_wait_failure(
    paths: &tractor_beam_isaac_injector::NativeHookPaths,
    event_tx: &RuntimeEventSender,
    phase: HookStartupPhase,
    message: impl Into<String>,
) {
    send_event(
        event_tx,
        RuntimeEvent::HookStartup(Box::new(hook_startup_state(
            phase, paths, None, None, message,
        ))),
    );
}

fn injection_support_message(error: &tractor_beam_isaac_injector::InjectorError) -> String {
    if error.is_native_hook_already_loaded() {
        return STALE_NATIVE_HOOK_MESSAGE.to_owned();
    }
    if error.is_admin_permission_cancelled() {
        return ADMIN_PERMISSION_CANCELLED_MESSAGE.to_owned();
    }
    let message = error.to_string();
    if error.is_access_denied() {
        format!(
            "{message}; access denied usually means Bridge GUI, Steam, and Isaac need matching privilege levels or security software allowed the helper"
        )
    } else {
        message
    }
}

fn send_injector_launch_event(
    event_tx: &RuntimeEventSender,
    paths: &tractor_beam_isaac_injector::NativeHookPaths,
    process_name: &str,
    process_id: u32,
    event: tractor_beam_isaac_injector::InjectorLaunchEvent,
) {
    match event {
        tractor_beam_isaac_injector::InjectorLaunchEvent::ElevatedRetryStarting => {
            let mut state = hook_startup_state(
                HookStartupPhase::Injecting,
                paths,
                Some(process_name),
                Some(process_id),
                ADMIN_PERMISSION_REQUEST_MESSAGE,
            );
            state.access_denied = true;
            send_event(event_tx, RuntimeEvent::HookStartup(Box::new(state)));
            send_event(
                event_tx,
                log_event(LogLevel::Info, ADMIN_PERMISSION_REQUEST_MESSAGE),
            );
        }
        tractor_beam_isaac_injector::InjectorLaunchEvent::ElevatedRetrySucceeded => {
            send_event(
                event_tx,
                log_event(LogLevel::Info, ELEVATED_INJECTOR_RETRY_SUCCEEDED_MESSAGE),
            );
        }
    }
}

fn hook_startup_state(
    phase: HookStartupPhase,
    paths: &tractor_beam_isaac_injector::NativeHookPaths,
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
        endpoint: Some("local IPC".to_owned()),
        message: Some(message.into()),
        updated_at: unix_seconds(),
        ..HookStartupState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tractor_beam_isaac_injector::{InjectionStep, InjectorError, NativeHookPaths};

    #[test]
    fn injector_helper_has_bounded_startup_window() {
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
    fn admin_permission_cancelled_support_message_is_short() {
        let message = injection_support_message(&InjectorError::AdminPermissionCancelled);

        assert_eq!(message, ADMIN_PERMISSION_CANCELLED_MESSAGE);
    }

    #[test]
    fn stale_native_hook_support_message_requires_full_isaac_restart() {
        let message = injection_support_message(&InjectorError::NativeHookAlreadyLoaded);

        assert_eq!(message, STALE_NATIVE_HOOK_MESSAGE);
        assert!(message.contains("Fully exit Isaac"));
    }

    #[test]
    fn elevated_retry_start_event_marks_access_denied() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
        let paths = NativeHookPaths {
            injector: PathBuf::from("bundle/tractor-beam-isaac-injector.exe"),
            hook: PathBuf::from("bundle/tractor_beam_native_hook.dll"),
        };

        send_injector_launch_event(
            &event_tx,
            &paths,
            "isaac-ng.exe",
            42,
            tractor_beam_isaac_injector::InjectorLaunchEvent::ElevatedRetryStarting,
        );

        let Some(RuntimeEvent::HookStartup(startup)) = event_rx.blocking_recv() else {
            panic!("expected hook startup event");
        };
        assert_eq!(startup.phase, HookStartupPhase::Injecting);
        assert!(startup.access_denied);
        assert_eq!(
            startup.message.as_deref(),
            Some(ADMIN_PERMISSION_REQUEST_MESSAGE)
        );
        let Some(RuntimeEvent::Log(LogLevel::Info, message)) = event_rx.blocking_recv() else {
            panic!("expected log event");
        };
        assert_eq!(message, ADMIN_PERMISSION_REQUEST_MESSAGE);
    }

    #[test]
    fn startup_state_carries_artifact_paths_and_endpoint() {
        let paths = NativeHookPaths {
            injector: PathBuf::from("bundle/tractor-beam-isaac-injector.exe"),
            hook: PathBuf::from("bundle/tractor_beam_native_hook.dll"),
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
        assert_eq!(state.endpoint.as_deref(), Some("local IPC"));
    }
}
