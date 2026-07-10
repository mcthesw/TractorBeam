use std::{
    env, fs,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::client::{SessionHealthConfig, SessionHealthSnapshot, SessionQuality, TransportChoice};

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
        relay: RelayEndpoint::new("relay.example.com", 25_910),
        relay_name: None,
        transport: TransportChoice::Udp,
        session_credential: crate::SessionCredential::generate(),
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
        ..SessionHealthSnapshot::default()
    });

    let text = client.diagnostics_text();

    assert!(text.contains("session health:"));
    assert!(text.contains("quality=good"));
    assert!(text.contains("\"quality\": \"good\""));
}

#[test]
fn diagnostics_include_native_hook_startup_evidence() {
    let mut client = BridgeClient::new();
    client.state.hook_launch_parameters_path_written =
        Some(PathBuf::from("bundle/native-hook/isaac_bridge_config.txt"));
    client.state.hook_launch_parameters_cleanup = Some(
        "removed path=bundle/native-hook/isaac_bridge_config.txt reason=user stopped session"
            .to_owned(),
    );
    client.state.hook_startup = state::HookStartupState {
        phase: state::HookStartupPhase::WaitingForHookEndpoint,
        process_name: Some("isaac-ng.exe".to_owned()),
        pid: Some(42),
        injector_path: Some(PathBuf::from("bundle/tractor-beam-isaac-injector.exe")),
        hook_path: Some(PathBuf::from(
            "bundle/native-hook/tractor_beam_native_hook.dll",
        )),
        launch_parameters_path: Some(PathBuf::from("bundle/native-hook/isaac_bridge_config.txt")),
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
    assert!(text.contains("hook_path: bundle/native-hook/tractor_beam_native_hook.dll"));
    assert!(text.contains("launch_parameters_path: bundle/native-hook/isaac_bridge_config.txt"));
    assert!(text.contains("launch_parameters_cleanup: removed path="));
}

#[test]
fn cleanup_hook_launch_parameters_keeps_first_successful_result() {
    let directory = unique_test_dir("hook-launch-cleanup");
    let path = directory.join("isaac_bridge_config.txt");
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
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn cleanup_hook_launch_parameters_records_already_missing() {
    let directory = unique_test_dir("hook-launch-cleanup-missing");
    let path = directory.join("isaac_bridge_config.txt");
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
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn startup_failure_record_keeps_artifact_and_launch_parameter_paths() {
    let mut client = BridgeClient::new();
    let paths = tractor_beam_isaac_injector::NativeHookPaths {
        injector: PathBuf::from("bundle/tractor-beam-isaac-injector.exe"),
        hook: PathBuf::from("bundle/native-hook/tractor_beam_native_hook.dll"),
    };
    client.state.hook_launch_parameters_path_written =
        Some(PathBuf::from("bundle/native-hook/isaac_bridge_config.txt"));

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
        Some(PathBuf::from("bundle/native-hook/isaac_bridge_config.txt").as_path())
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

fn unique_test_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let path = env::temp_dir().join(format!("tractor-beam-{name}-{}-{nonce}", process::id()));
    fs::create_dir_all(&path).expect("create test directory");
    path
}
