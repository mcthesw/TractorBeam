use std::{
    io,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::client::test_relay::TestRelay;

use super::*;

static SESSION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn session_start_reports_relay_join_timeout() {
    let _guard = SESSION_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let relay = TestRelay::spawn_silent();

    let error = spawn_bridge_worker(
        test_session_config(relay.address.port()),
        test_native_hook_paths(),
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    relay.stop();
}

#[test]
fn session_start_reports_initial_room_peers() {
    let _guard = SESSION_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let relay = TestRelay::spawn();
    let handle = spawn_bridge_worker(
        test_session_config(relay.address.port()),
        test_native_hook_paths(),
    )
    .unwrap();

    let event = recv_matching(&handle.events, |event| {
        matches!(
            event,
            RuntimeEvent::RoomPeersUpdated(peers)
                if peers.len() == 1 && peers[0].steam_id64 == 76_561_198_000_000_001
        )
    });

    assert!(event.is_some());
    handle.stop();
    relay.stop();
}

#[test]
fn runtime_rtt_timeout_is_nonfatal() {
    let _guard = SESSION_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let relay = TestRelay::spawn();
    let handle = spawn_bridge_worker(
        SessionConfig {
            route: super::super::SessionRouteConfig::ExternalRelay(
                super::super::ExternalRelayConfig {
                    relay: super::super::RelayEndpoint::new("127.0.0.1", relay.address.port()),
                    relay_name: None,
                    transport: super::super::TransportChoice::Tcp,
                    session_credential: super::super::SessionCredential::generate(),
                },
            ),
            mode: super::super::SessionMode::Pure,
            steam_id64: "76561198000000001".to_owned(),
            display_name: "Test".to_owned(),
            session_health: super::super::SessionHealthConfig {
                snapshot_interval_seconds: 1,
                runtime_rtt_interval_seconds: 1,
                runtime_rtt_timeout_seconds: 1,
                ..super::super::SessionHealthConfig::default()
            },
        },
        test_native_hook_paths(),
    )
    .unwrap();

    let event = recv_matching(&handle.events, |event| {
        matches!(
            event,
            RuntimeEvent::SessionHealthSnapshot(snapshot) if snapshot.runtime_rtt.sent > 0
        )
    });

    assert!(event.is_some());
    handle.stop();
    relay.stop();
}

#[tokio::test]
async fn official_mode_owns_a_cancellable_process_lifecycle_task() {
    let mut config = test_session_config(1);
    config.mode = super::super::SessionMode::Official;
    let cancellation = CancellationToken::new();
    let (event_tx, _event_rx) = tokio_mpsc::channel(EVENT_QUEUE_CAPACITY);

    let tasks = start_runtime_tasks_inner(&config, None, None, &cancellation, &event_tx)
        .await
        .expect("Official lifecycle should start without Hook or Relay sockets");

    assert!(tasks.route.is_empty());
    assert_eq!(tasks.support.len(), 1);
    assert!(tasks.health.is_none());
    cancellation.cancel();
    shutdown_tasks(tasks.support, &event_tx).await;
}

#[tokio::test]
async fn lan_mode_attaches_existing_room_without_relay() {
    let credential = super::super::SessionCredential::from_bytes([21; 16]);
    let room = Arc::new(
        super::super::LanControlPlane::create(
            tractor_beam_direct_protocol::PeerIdentity::new(
                76_561_198_000_000_001,
                tractor_beam_direct_protocol::InstanceId::from_bytes([1; 16]),
            ),
            "Test".to_owned(),
            credential,
            &[super::super::LanAdapterAddress {
                adapter_id: "test-loopback".to_owned(),
                name: "Loopback".to_owned(),
                address: "127.0.0.1".parse().unwrap(),
                interface_index: 1,
            }],
        )
        .await
        .unwrap(),
    );
    let config = SessionConfig {
        route: super::super::SessionRouteConfig::LanDirect(super::super::LanDirectConfig {
            session_credential: credential,
            room: Some(room.clone()),
        }),
        mode: super::super::SessionMode::Pure,
        steam_id64: "76561198000000001".to_owned(),
        display_name: "Test".to_owned(),
        session_health: super::super::SessionHealthConfig::default(),
    };
    let cancellation = CancellationToken::new();
    let (event_tx, _event_rx) = tokio_mpsc::channel(EVENT_QUEUE_CAPACITY);
    let (_control, control_rx) = hook_ipc::control_channel();
    let tasks = start_runtime_tasks_inner(
        &config,
        Some(SessionNativeHook::new(
            test_native_hook_paths(),
            HookIpcSession::test(),
        )),
        Some(control_rx),
        &cancellation,
        &event_tx,
    )
    .await
    .unwrap();

    assert!(!tasks.route.is_empty());
    cancellation.cancel();
    shutdown_tasks(tasks.route, &event_tx).await;
    shutdown_tasks(tasks.support, &event_tx).await;
    room.stop().await;
}

fn test_session_config(port: u16) -> SessionConfig {
    SessionConfig {
        route: super::super::SessionRouteConfig::ExternalRelay(super::super::ExternalRelayConfig {
            relay: super::super::RelayEndpoint::new("127.0.0.1", port),
            relay_name: None,
            transport: super::super::TransportChoice::Tcp,
            session_credential: super::super::SessionCredential::generate(),
        }),
        mode: super::super::SessionMode::Pure,
        steam_id64: "76561198000000001".to_owned(),
        display_name: "Test".to_owned(),
        session_health: super::super::SessionHealthConfig::default(),
    }
}

fn recv_matching(
    receiver: &Receiver<RuntimeEvent>,
    predicate: impl Fn(&RuntimeEvent) -> bool,
) -> Option<RuntimeEvent> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if let Ok(event) = receiver.recv_timeout(Duration::from_millis(50))
            && predicate(&event)
        {
            return Some(event);
        }
    }
    None
}
