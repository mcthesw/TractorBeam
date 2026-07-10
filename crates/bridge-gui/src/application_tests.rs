use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use super::*;

#[test]
fn snapshot_only_accepts_mutations_when_ready_and_idle() {
    let mut snapshot = ApplicationSnapshot {
        bootstrap: BootstrapState::Ready,
        ..ApplicationSnapshot::default()
    };
    assert!(snapshot.accepts_mutation());

    snapshot.operation = Some(ApplicationOperation::Starting);
    assert!(!snapshot.accepts_mutation());

    snapshot.operation = None;
    snapshot.shutdown_complete = true;
    assert!(!snapshot.accepts_mutation());
}

#[test]
fn shutdown_control_takes_priority_over_stop() {
    let control = AtomicU8::new(CONTROL_NONE);
    control.fetch_max(CONTROL_STOP, Ordering::Release);
    control.store(CONTROL_SHUTDOWN, Ordering::Release);

    assert_eq!(
        control.swap(CONTROL_NONE, Ordering::AcqRel),
        CONTROL_SHUTDOWN
    );
}

#[test]
fn pending_selection_keeps_only_latest_value() {
    let pending = Mutex::new(None);
    let first = ClientConfigSelection {
        selected_relay: Some("first".to_owned()),
        room: Some("room-a".to_owned()),
        selected_steam_id64: None,
    };
    let latest = ClientConfigSelection {
        selected_relay: Some("latest".to_owned()),
        room: Some("room-b".to_owned()),
        selected_steam_id64: Some("76561198000000001".to_owned()),
    };

    *lock(&pending) = Some(first);
    *lock(&pending) = Some(latest.clone());

    assert_eq!(lock(&pending).take(), Some(latest));
}

#[test]
fn command_submitted_before_another_operation_finishes_is_rejected() {
    let snapshot = Arc::new(SnapshotStore {
        value: Mutex::new(ApplicationSnapshot {
            bootstrap: BootstrapState::Ready,
            admission_generation: 7,
            ..ApplicationSnapshot::default()
        }),
        wake: Arc::new(|| {}),
    });
    let queued = QueuedCommand {
        admission_generation: 7,
        command: ApplicationCommand::ClearLogs,
    };
    assert!(command_is_current(&snapshot, &queued));

    update_snapshot(&snapshot, |snapshot| {
        snapshot.operation = Some(ApplicationOperation::Starting);
        snapshot.admission_generation = snapshot.admission_generation.saturating_add(1);
    });
    update_snapshot(&snapshot, |snapshot| {
        snapshot.operation = None;
        snapshot.admission_generation = snapshot.admission_generation.saturating_add(1);
    });

    assert!(!command_is_current(&snapshot, &queued));
}

#[test]
fn failed_bootstrap_stays_open_and_can_retry() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let factory_attempts = Arc::clone(&attempts);
    let application = ApplicationHandle::spawn_with(
        || {},
        Box::new(move || {
            if factory_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(io::Error::other("test bootstrap failed"));
            }
            let loaded = LoadedClientConfig::default();
            Ok((BridgeClient::with_config(loaded.clone()), loaded))
        }),
    );

    wait_for_snapshot(&application, |snapshot| {
        snapshot.bootstrap == BootstrapState::Failed
    });
    assert!(application.retry_bootstrap());
    wait_for_snapshot(&application, |snapshot| {
        snapshot.bootstrap == BootstrapState::Ready
    });
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    application.request_shutdown();
    wait_for_snapshot(&application, |snapshot| snapshot.shutdown_complete);
}

fn wait_for_snapshot(
    application: &ApplicationHandle,
    predicate: impl Fn(&ApplicationSnapshot) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if predicate(&application.snapshot()) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("application snapshot did not reach expected state");
}
