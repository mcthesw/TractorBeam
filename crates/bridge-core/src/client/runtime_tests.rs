use std::{fs, path::PathBuf};

use crate::client::{
    DirectDirectionHealthSnapshot, DirectDropReason, DirectEpochHealthSnapshot,
    DirectFlowDirection, DirectFlowHealthSnapshot, DirectFlowStage, DirectPeerHealthSnapshot,
    DirectRejectionHealthSnapshot, ExternalRelayConfig, LanDirectConfig, QualityConfidence,
    SessionHealthConfig, SessionHealthSnapshot, SessionQuality, SmoothnessReason, TransportChoice,
};

use super::*;

#[test]
fn exposes_runtime_name() {
    assert_eq!(runtime_name(), "bridge-core");
}

#[test]
fn validates_relay_endpoint() {
    assert!(
        RelayEndpoint::new("relay.example.com", 25_910)
            .validate()
            .is_ok()
    );
    assert_eq!(
        RelayEndpoint::new("", 25_910).validate(),
        Err(ConfigError::MissingRelayHost)
    );
}

#[test]
fn validates_session_config() {
    let config = SessionConfig {
        route: SessionRouteConfig::ExternalRelay(ExternalRelayConfig {
            relay: RelayEndpoint::new("relay.example.com", 25_910),
            relay_name: None,
            transport: TransportChoice::Udp,
            session_credential: crate::SessionCredential::generate(),
        }),
        mode: SessionMode::Pure,
        steam_id64: "76561198000000001".to_owned(),
        display_name: "Alice".to_owned(),
        session_health: SessionHealthConfig::default(),
    };

    assert!(config.validate().is_ok());
}

#[test]
fn validates_lan_session_without_relay_fields() {
    let config = SessionConfig {
        route: SessionRouteConfig::LanDirect(LanDirectConfig {
            session_credential: crate::SessionCredential::generate(),
            room: None,
        }),
        mode: SessionMode::Pure,
        steam_id64: "76561198000000001".to_owned(),
        display_name: "Alice".to_owned(),
        session_health: SessionHealthConfig::default(),
    };

    assert!(config.validate().is_ok());
}

#[test]
fn redacts_exported_diagnostics_text() {
    let mut client = BridgeClient::new();
    client.log(LogLevel::Info, "Relay endpoint: 203.0.113.10:25910");
    client.log(LogLevel::Info, "Starting Pure session in room 123");
    client.log(LogLevel::Info, "SteamID64 76561198000000001");

    let text = client.redacted_diagnostics_text();

    assert!(!text.contains("203.0.113.10"));
    assert!(!text.contains("76561198000000001"));
    assert!(!text.contains("room 123"));
}

#[test]
fn diagnostics_include_session_health_evidence() {
    let mut client = BridgeClient::new();
    client.state.latest_session_health = Some(SessionHealthSnapshot {
        quality: SessionQuality::Good,
        direct: DirectFlowHealthSnapshot {
            enabled: true,
            send: direct_direction(DirectFlowDirection::Send, 14, 11, 3),
            receive: direct_direction(DirectFlowDirection::Receive, 12, 11, 1),
            peers: vec![
                direct_peer(1, 76561198000000001, 2, (12, 9, 3), (8, 7, 1)),
                direct_peer(2, 76561198000000002, 4, (2, 2, 0), (4, 4, 0)),
            ],
            transitions_dropped: 4,
        },
        ..SessionHealthSnapshot::default()
    });
    client.state.smoothness.level = SessionQuality::Watch;
    client.state.smoothness.confidence = QualityConfidence::Medium;
    client.state.smoothness.reasons = vec![SmoothnessReason::PathJitterElevated];

    let text = client.diagnostics_text();

    assert!(text.contains("session health:"));
    assert!(text.contains("quality=good"));
    assert!(text.contains("direct_send_succeeded=11"));
    assert!(text.contains("direct_receive_dropped=1"));
    assert!(text.contains("\"peer_slot\": 1"));
    assert!(text.contains("\"lifecycle_epoch\": 2"));
    assert!(text.contains("\"stage\": \"outbound_queue\""));
    assert!(text.contains("\"reason\": \"queue_full\""));
    assert!(text.contains("\"current_queue_depth\": 0"));
    assert!(text.contains("\"transitions_dropped\": 4"));
    assert!(text.contains("\"dropped\": 3"));
    assert!(text.contains("\"quality\": \"good\""));
    assert!(text.contains("current smoothness:"));
    assert!(text.contains("\"level\": \"watch\""));
    assert!(text.contains("path_jitter_elevated"));
    assert!(text.contains("input_delay_evidence:"));

    let redacted = client.redacted_diagnostics_text();
    assert!(!redacted.contains("76561198000000001"));
    assert!(!redacted.contains("76561198000000002"));
    assert!(redacted.contains("\"peer_slot\": 1"));
    assert!(redacted.contains("\"peer_slot\": 2"));
    assert!(redacted.contains("\"lifecycle_epoch\": 2"));
}

fn direct_peer(
    peer_slot: u32,
    peer_steam_id64: u64,
    lifecycle_epoch: u64,
    send_outcomes: (u64, u64, u64),
    receive_outcomes: (u64, u64, u64),
) -> DirectPeerHealthSnapshot {
    let (send_queued, send_success, send_dropped) = send_outcomes;
    let (receive_queued, receive_success, receive_dropped) = receive_outcomes;
    let send = direct_direction(
        DirectFlowDirection::Send,
        send_queued,
        send_success,
        send_dropped,
    );
    let receive = direct_direction(
        DirectFlowDirection::Receive,
        receive_queued,
        receive_success,
        receive_dropped,
    );
    DirectPeerHealthSnapshot {
        peer_slot,
        peer_steam_id64,
        latest_lifecycle_epoch: lifecycle_epoch,
        active: true,
        send: send.clone(),
        receive: receive.clone(),
        epochs: vec![DirectEpochHealthSnapshot {
            lifecycle_epoch,
            active: true,
            send,
            receive,
        }],
    }
}

fn direct_direction(
    direction: DirectFlowDirection,
    queued: u64,
    resolved_success: u64,
    dropped: u64,
) -> DirectDirectionHealthSnapshot {
    DirectDirectionHealthSnapshot {
        direction,
        queued,
        resolved_success,
        dropped,
        current_queue_depth: 0,
        max_queue_depth: 12,
        rejections: if dropped == 0 {
            Vec::new()
        } else {
            vec![DirectRejectionHealthSnapshot {
                direction,
                stage: DirectFlowStage::OutboundQueue,
                reason: DirectDropReason::QueueFull,
                count: dropped,
            }]
        },
    }
}

#[test]
fn diagnostics_include_native_hook_startup_evidence() {
    let mut client = BridgeClient::new();
    client.state.hook_launch_parameters_path_written =
        Some(PathBuf::from("bundle/logs/hook/hook-runtime.txt"));
    client.state.hook_launch_parameters_cleanup = Some(
        "removed path=bundle/logs/hook/hook-runtime.txt reason=user stopped session".to_owned(),
    );
    client.state.hook_startup = state::HookStartupState {
        phase: state::HookStartupPhase::WaitingForHookEndpoint,
        process_name: Some("isaac-ng.exe".to_owned()),
        pid: Some(42),
        injector_path: Some(PathBuf::from("bundle/tractor-beam-isaac-injector.exe")),
        hook_path: Some(PathBuf::from("bundle/tractor_beam_native_hook.dll")),
        launch_parameters_path: Some(PathBuf::from("bundle/logs/hook/hook-runtime.txt")),
        endpoint: Some("local IPC".to_owned()),
        injected: true,
        endpoint_ready: false,
        access_denied: false,
        message: Some("waiting for endpoint".to_owned()),
        updated_at: 123,
    };

    let text = client.diagnostics_text();

    assert!(text.contains("native hook startup:"));
    assert!(text.contains("phase: waiting_for_hook_endpoint"));
    assert!(text.contains("process_name: isaac-ng.exe"));
    assert!(text.contains("injector_path: bundle/tractor-beam-isaac-injector.exe"));
    assert!(text.contains("hook_path: bundle/tractor_beam_native_hook.dll"));
    assert!(text.contains("launch_parameters_path: bundle/logs/hook/hook-runtime.txt"));
    assert!(text.contains("launch_parameters_cleanup: removed path="));
}

#[test]
fn cleanup_hook_launch_parameters_keeps_first_successful_result() {
    let directory = tempfile::tempdir().expect("create test directory");
    let path = directory.path().join("hook-runtime.txt");
    fs::write(&path, "sidecar=127.0.0.1:25900\n").expect("write launch parameters");
    let mut client = BridgeClient::new();
    client.state.hook_launch_parameters_path_written = Some(path.clone());

    client.cleanup_hook_launch_parameters("user stopped session");

    assert!(!path.exists());
    let cleanup = client
        .state
        .hook_launch_parameters_cleanup
        .clone()
        .expect("cleanup result should be recorded");
    assert!(cleanup.starts_with("removed "));
    assert!(cleanup.contains("reason=user stopped session"));

    client.cleanup_hook_launch_parameters("session ended");

    assert_eq!(
        client.state.hook_launch_parameters_cleanup.as_deref(),
        Some(cleanup.as_str())
    );
}

#[test]
fn cleanup_hook_launch_parameters_records_already_missing() {
    let directory = tempfile::tempdir().expect("create test directory");
    let path = directory.path().join("hook-runtime.txt");
    let mut client = BridgeClient::new();
    client.state.hook_launch_parameters_path_written = Some(path);

    client.cleanup_hook_launch_parameters("session ended");

    let cleanup = client
        .state
        .hook_launch_parameters_cleanup
        .as_deref()
        .expect("cleanup result should be recorded");
    assert!(cleanup.starts_with("already_missing "));
    assert!(cleanup.contains("reason=session ended"));
}

#[test]
fn startup_failure_record_keeps_artifact_and_launch_parameter_paths() {
    let mut client = BridgeClient::new();
    let paths = tractor_beam_isaac_injector::NativeHookPaths {
        injector: PathBuf::from("bundle/tractor-beam-isaac-injector.exe"),
        hook: PathBuf::from("bundle/tractor_beam_native_hook.dll"),
    };
    client.state.hook_launch_parameters_path_written =
        Some(PathBuf::from("bundle/logs/hook/hook-runtime.txt"));

    client.record_hook_startup_failure(Some(&paths), "Bridge worker startup failed");

    assert_eq!(
        client.state.hook_startup.phase,
        state::HookStartupPhase::Failed
    );
    assert_eq!(
        client.state.hook_startup.injector_path.as_ref(),
        Some(&paths.injector)
    );
    assert_eq!(
        client.state.hook_startup.hook_path.as_ref(),
        Some(&paths.hook)
    );
    assert_eq!(
        client.state.hook_startup.launch_parameters_path.as_deref(),
        Some(PathBuf::from("bundle/logs/hook/hook-runtime.txt").as_path())
    );
}

#[test]
fn reliable_game_exit_completion_returns_client_to_idle() {
    let mut client = BridgeClient::new();
    client.state.status = state::SessionStatus::Running;
    client.state.active_session_mode = Some(SessionMode::Pure);
    client.session = Some(session::SessionHandle::with_test_events(vec![
        state::RuntimeEvent::SessionEnded(state::SessionStopReason::GameExited {
            process_name: "isaac-ng.exe".to_owned(),
            pid: 42,
        }),
        state::RuntimeEvent::SessionEnded(state::SessionStopReason::RuntimeEnded {
            message: "later task exit".to_owned(),
        }),
        state::RuntimeEvent::Stopped,
    ]));

    assert!(client.poll_events());
    assert_eq!(client.state.status, state::SessionStatus::Idle);
    assert_eq!(client.state.active_session_mode, None);
    assert!(client.session.is_none());
    assert_eq!(
        client.state.last_stop_reason,
        Some(state::SessionStopReason::GameExited {
            process_name: "isaac-ng.exe".to_owned(),
            pid: 42,
        })
    );
}

#[test]
fn stop_does_not_overwrite_a_terminal_reason_that_already_arrived() {
    let mut client = BridgeClient::new();
    client.state.status = state::SessionStatus::Running;
    client.state.active_session_mode = Some(SessionMode::Official);
    client.session = Some(session::SessionHandle::with_test_events(vec![
        state::RuntimeEvent::SessionEnded(state::SessionStopReason::GameExited {
            process_name: "isaac-ng.exe".to_owned(),
            pid: 42,
        }),
        state::RuntimeEvent::Stopped,
    ]));

    client.stop_session();
    client.stop_session();

    assert_eq!(client.state.status, state::SessionStatus::Idle);
    assert_eq!(
        client.state.last_stop_reason,
        Some(state::SessionStopReason::GameExited {
            process_name: "isaac-ng.exe".to_owned(),
            pid: 42,
        })
    );
}
