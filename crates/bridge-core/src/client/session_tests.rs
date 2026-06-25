use std::{
    io,
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use bytes::Bytes;

use crate::protocol::{ControlMessage, Envelope, MessageType};

use super::*;

static SESSION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn session_reports_malformed_hook_packet_and_stops() {
    let _guard = SESSION_TEST_LOCK.lock().unwrap();
    let relay = TestRelay::spawn();
    let handle = spawn_bridge_worker(
        test_session_config(relay.address.port()),
        test_native_hook_paths(),
    )
    .unwrap();

    let sender = StdUdpSocket::bind("127.0.0.1:0").unwrap();
    sender.send_to(b"not-a-local-packet", HOOK_IN).unwrap();

    let event = recv_matching(&handle.events, |event| {
        matches!(
            event,
            RuntimeEvent::Log(level, message) if *level == LogLevel::Warn && message.contains("Bad hook packet")
        )
    });
    assert!(event.is_some());

    handle.stop();
    relay.stop();
}

#[test]
fn session_start_reports_relay_join_timeout() {
    let _guard = SESSION_TEST_LOCK.lock().unwrap();
    let relay = SilentRelay::spawn();

    let error = spawn_bridge_worker(
        test_session_config(relay.address.port()),
        test_native_hook_paths(),
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    relay.stop();
}

#[test]
fn runtime_rtt_timeout_is_nonfatal() {
    let _guard = SESSION_TEST_LOCK.lock().unwrap();
    let relay = TestRelay::spawn();
    let handle = spawn_bridge_worker(
        SessionConfig {
            relay: super::super::RelayEndpoint::new("127.0.0.1", relay.address.port()),
            relay_name: None,
            transport: super::super::TransportChoice::Udp,
            room: "test-room".to_owned(),
            mode: super::super::SessionMode::Pure,
            steam_id64: "76561198000000001".to_owned(),
            display_name: "Test".to_owned(),
            session_health: super::super::SessionHealthConfig {
                snapshot_interval_seconds: 1,
                runtime_rtt_interval_seconds: 1,
                runtime_rtt_timeout_seconds: 1,
                ..super::super::SessionHealthConfig::default()
            },
            #[cfg(feature = "internal-test")]
            test_run_id: None,
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

fn test_session_config(port: u16) -> SessionConfig {
    SessionConfig {
        relay: super::super::RelayEndpoint::new("127.0.0.1", port),
        relay_name: None,
        transport: super::super::TransportChoice::Udp,
        room: "test-room".to_owned(),
        mode: super::super::SessionMode::Pure,
        steam_id64: "76561198000000001".to_owned(),
        display_name: "Test".to_owned(),
        session_health: super::super::SessionHealthConfig::default(),
        #[cfg(feature = "internal-test")]
        test_run_id: None,
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

struct TestRelay {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: thread::JoinHandle<()>,
}

impl TestRelay {
    fn spawn() -> Self {
        let socket = StdUdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let address = socket.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || run_test_relay(socket, &worker_stop));
        Self {
            address,
            stop,
            worker,
        }
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.worker.join();
    }
}

struct SilentRelay {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: thread::JoinHandle<()>,
}

impl SilentRelay {
    fn spawn() -> Self {
        let socket = StdUdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let address = socket.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while !worker_stop.load(Ordering::Relaxed) {
                let _ = socket.recv_from(&mut buffer);
            }
        });
        Self {
            address,
            stop,
            worker,
        }
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.worker.join();
    }
}

fn run_test_relay(socket: StdUdpSocket, stop: &AtomicBool) {
    let mut buffer = [0_u8; 4096];
    while !stop.load(Ordering::Relaxed) {
        let Ok((size, address)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let Ok(envelope) = Envelope::decode(Bytes::copy_from_slice(&buffer[..size])) else {
            continue;
        };
        if envelope.message_type != MessageType::Join {
            continue;
        }
        let Ok(control) = ControlMessage::decode(&envelope.payload) else {
            continue;
        };
        let response = match control {
            ControlMessage::Join {
                challenge: None, ..
            } => (
                MessageType::JoinChallenge,
                ControlMessage::Challenge {
                    token: "token".to_owned(),
                },
            ),
            ControlMessage::Join {
                challenge: Some(_), ..
            } => (
                MessageType::JoinReady,
                ControlMessage::Ready { peer_count: 1 },
            ),
            _ => continue,
        };
        let payload = response.1.encode().unwrap();
        let raw = Envelope::new(response.0, payload).encode().unwrap();
        socket.send_to(&raw, address).unwrap();
    }
}
