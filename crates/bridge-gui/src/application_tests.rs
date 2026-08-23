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
fn room_activity_covers_relay_and_lan_rooms() {
    let mut snapshot = ApplicationSnapshot::default();
    assert!(!snapshot.room_active());

    snapshot.relay_room_active = true;
    assert!(snapshot.room_active());

    snapshot.relay_room_active = false;
    snapshot.lan_room = Some(LanRoomSnapshot {
        invitation_code: String::new(),
        peers: Vec::new(),
        paths: Vec::new(),
    });
    assert!(snapshot.room_active());
}

#[test]
fn relay_catalog_changes_require_idle_without_any_room() {
    assert!(commands::relay_catalog_change_allowed(
        SessionStatus::Idle,
        false,
        false
    ));
    assert!(!commands::relay_catalog_change_allowed(
        SessionStatus::Running,
        false,
        false
    ));
    assert!(!commands::relay_catalog_change_allowed(
        SessionStatus::Idle,
        true,
        false
    ));
    assert!(!commands::relay_catalog_change_allowed(
        SessionStatus::Idle,
        false,
        true
    ));
}

#[test]
fn shutdown_control_takes_priority_over_leave_room() {
    let control = AtomicU8::new(CONTROL_NONE);
    control.fetch_max(CONTROL_LEAVE_ROOM, Ordering::Release);
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
        selected_steam_id64: None,
    };
    let latest = ClientConfigSelection {
        selected_relay: Some("latest".to_owned()),
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
            command_generation: 7,
            ..ApplicationSnapshot::default()
        }),
        wake: Arc::new(|| {}),
    });
    let queued = QueuedCommand {
        command_generation: 7,
        command: ApplicationCommand::ClearLogs,
    };
    assert!(command_is_current(&snapshot, &queued));

    update_snapshot(&snapshot, |snapshot| {
        snapshot.operation = Some(ApplicationOperation::Starting);
        snapshot.command_generation = snapshot.command_generation.saturating_add(1);
    });
    update_snapshot(&snapshot, |snapshot| {
        snapshot.operation = None;
        snapshot.command_generation = snapshot.command_generation.saturating_add(1);
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
        None,
        None,
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

#[test]
fn available_update_is_published_after_bootstrap() {
    let update = AvailableUpdate {
        version: "0.6.0".to_owned(),
        url: "https://github.com/mcthesw/TractorBeam/releases/tag/v0.6.0".to_owned(),
    };
    let expected = update.clone();
    let application = ApplicationHandle::spawn_with(
        || {},
        Box::new(|| {
            let loaded = LoadedClientConfig::default();
            Ok((BridgeClient::with_config(loaded.clone()), loaded))
        }),
        None,
        Some(Box::new(move || Ok(Some(update)))),
    );

    wait_for_snapshot(&application, |snapshot| {
        snapshot.bootstrap == BootstrapState::Ready
    });
    let published = wait_for_event(&application, |event| match event {
        ApplicationEvent::UpdateAvailable(update) => Some(update),
        _ => None,
    });
    assert_eq!(published, expected);

    application.request_shutdown();
    wait_for_snapshot(&application, |snapshot| snapshot.shutdown_complete);
}

#[test]
fn update_check_is_skipped_when_bootstrap_fails() {
    let (update_started_tx, update_started_rx) = std::sync::mpsc::channel();
    let application = ApplicationHandle::spawn_with(
        || {},
        Box::new(|| {
            Err(io::Error::other(ClientLogInitError::from(
                io::Error::other("Client logging failed"),
            )))
        }),
        None,
        Some(Box::new(move || {
            update_started_tx.send(()).unwrap();
            Ok(None)
        })),
    );

    wait_for_snapshot(&application, |snapshot| {
        snapshot.bootstrap == BootstrapState::Failed
    });
    assert_eq!(
        application.snapshot().bootstrap_failure,
        Some(BootstrapFailure::LoggingUnavailable)
    );
    assert!(update_started_rx.try_recv().is_err());

    application.request_shutdown();
    wait_for_snapshot(&application, |snapshot| snapshot.shutdown_complete);
}

#[test]
fn update_check_does_not_block_application_shutdown() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let application = ApplicationHandle::spawn_with(
        || {},
        Box::new(|| {
            let loaded = LoadedClientConfig::default();
            Ok((BridgeClient::with_config(loaded.clone()), loaded))
        }),
        None,
        Some(Box::new(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(None)
        })),
    );

    wait_for_snapshot(&application, |snapshot| {
        snapshot.bootstrap == BootstrapState::Ready
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    application.request_shutdown();
    wait_for_snapshot(&application, |snapshot| snapshot.shutdown_complete);
    release_tx.send(()).unwrap();
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

fn wait_for_event<T>(
    application: &ApplicationHandle,
    mut select: impl FnMut(ApplicationEvent) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        for event in application.drain_events() {
            if let Some(value) = select(event) {
                return value;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("application event did not arrive");
}
