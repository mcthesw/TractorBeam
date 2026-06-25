mod readiness;

use std::{
    fmt::{self, Display},
    fs,
    future::Future,
    io,
    net::UdpSocket,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::Duration,
};

use bytes::Bytes;
use serde::Serialize;
use tokio::{runtime::Builder, time};

use crate::protocol::{Envelope, GamePacket, LocalPacket, MessageType};

use super::{
    BridgeClient, LogLevel, RelayEndpoint, SessionConfig, SessionMode, TransportChoice,
    hook_config::HOOK_OUT,
    relay_transport::{RelayTransport, complete_relay_join},
    state::{RuntimeEvent, log_event, unix_seconds},
};

pub use readiness::{
    READINESS_PROBE_PAYLOAD_BYTES, READINESS_PROBE_SAMPLES_PER_CASE, ReadinessProbeCaseReport,
    ReadinessProbeReport,
};

pub(super) const PROBE_A_STEAM: &str = "76561198000000101";
pub(super) const PROBE_B_STEAM: &str = "76561198000000102";
pub const DEFAULT_RELAY_PROBE_PAYLOAD_BYTES: usize = 2_048;
pub(super) const MAX_RELAY_PROBE_PAYLOAD_BYTES: usize = 60_000;
const DATA_TIMEOUT: Duration = Duration::from_secs(3);
const HOOK_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RelayProbeReport {
    pub relay: String,
    pub transport: TransportChoice,
    pub room: String,
    pub a_to_b_bytes: usize,
    pub b_to_a_bytes: usize,
    pub payload_bytes: usize,
}

impl RelayProbeReport {
    #[must_use]
    pub fn short_summary(&self) -> String {
        format!(
            "Relay probe passed: {} byte payload, {} bytes A->B, {} bytes B->A via {} ({})",
            self.payload_bytes, self.a_to_b_bytes, self.b_to_a_bytes, self.relay, self.transport
        )
    }
}

impl Display for RelayProbeReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}; room={}", self.short_summary(), self.room)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HookReceiveProbeReport {
    pub peer: u64,
    pub sent_bytes: usize,
    pub local_in: bool,
    pub available_hit: bool,
    pub read_hit: bool,
}

impl HookReceiveProbeReport {
    #[must_use]
    pub fn short_summary(&self) -> String {
        format!(
            "Hook receive probe sent: local_in={}, available={}, read={}",
            self.local_in, self.available_hit, self.read_hit
        )
    }
}

impl Display for HookReceiveProbeReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}; peer={}, bytes={}",
            self.short_summary(),
            self.peer,
            self.sent_bytes
        )
    }
}

#[derive(Debug)]
pub(super) struct ProbeHandle {
    pub(super) events: Receiver<RuntimeEvent>,
    worker: Option<JoinHandle<()>>,
}

impl ProbeHandle {
    pub(super) fn finish(mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ProbeHandle {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl BridgeClient {
    pub fn run_relay_probe(&mut self, relay: RelayEndpoint) -> io::Result<RelayProbeReport> {
        self.run_relay_probe_with_payload(relay, DEFAULT_RELAY_PROBE_PAYLOAD_BYTES)
    }

    pub fn run_relay_probe_with_payload(
        &mut self,
        relay: RelayEndpoint,
        payload_bytes: usize,
    ) -> io::Result<RelayProbeReport> {
        self.run_relay_probe_with_transport_payload(relay, TransportChoice::Udp, payload_bytes)
    }

    pub fn run_relay_probe_with_transport_payload(
        &mut self,
        relay: RelayEndpoint,
        transport: TransportChoice,
        payload_bytes: usize,
    ) -> io::Result<RelayProbeReport> {
        relay.validate().map_err(io::Error::other)?;
        let report = run_relay_probe(relay, transport, payload_bytes)?;
        self.log(LogLevel::Info, report.to_string());
        Ok(report)
    }

    pub fn run_hook_receive_probe(&mut self) -> io::Result<HookReceiveProbeReport> {
        let report = run_hook_receive_probe(self.state.hook_log_path_written())?;
        self.log(LogLevel::Info, report.to_string());
        Ok(report)
    }
}

pub(super) fn spawn_readiness_probe(relay: RelayEndpoint) -> io::Result<ProbeHandle> {
    readiness::spawn_readiness_probe(relay)
}

pub(super) fn spawn_hook_receive_probe(hook_log_path: Option<PathBuf>) -> ProbeHandle {
    let (event_tx, events) = mpsc::channel();
    let worker = thread::spawn(move || match run_hook_receive_probe(hook_log_path) {
        Ok(report) => {
            let _ = event_tx.send(log_event(LogLevel::Info, report.to_string()));
            let _ = event_tx.send(RuntimeEvent::HookReceiveProbeFinished(Ok(report)));
        }
        Err(error) => {
            let message = format!("Hook receive probe failed: {error}");
            let _ = event_tx.send(log_event(LogLevel::Error, message.clone()));
            let _ = event_tx.send(RuntimeEvent::HookReceiveProbeFinished(Err(message)));
        }
    });
    ProbeHandle {
        events,
        worker: Some(worker),
    }
}

fn run_relay_probe(
    relay: RelayEndpoint,
    transport: TransportChoice,
    payload_bytes: usize,
) -> io::Result<RelayProbeReport> {
    validate_probe_payload(payload_bytes)?;
    block_on_probe(run_relay_probe_async(relay, transport, payload_bytes))
}

pub(super) fn validate_probe_payload(payload_bytes: usize) -> io::Result<()> {
    if (1..=MAX_RELAY_PROBE_PAYLOAD_BYTES).contains(&payload_bytes) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("relay probe payload must be 1..={MAX_RELAY_PROBE_PAYLOAD_BYTES} bytes"),
        ))
    }
}

async fn run_relay_probe_async(
    relay: RelayEndpoint,
    transport: TransportChoice,
    payload_bytes: usize,
) -> io::Result<RelayProbeReport> {
    let room = format!("bb-probe-{}-{}", std::process::id(), unix_seconds());
    let relay_display = relay.to_string();
    let mut peer_a = ProbePeer::join(&relay, transport, &room, PROBE_A_STEAM, "Probe A").await?;
    let mut peer_b = ProbePeer::join(&relay, transport, &room, PROBE_B_STEAM, "Probe B").await?;

    let payload = probe_payload(payload_bytes);
    peer_a.send_game(PROBE_B_STEAM, payload.clone()).await?;
    peer_b
        .expect_game(PROBE_A_STEAM, PROBE_B_STEAM, &payload)
        .await?;

    peer_b.send_game(PROBE_A_STEAM, payload.clone()).await?;
    peer_a
        .expect_game(PROBE_B_STEAM, PROBE_A_STEAM, &payload)
        .await?;

    Ok(RelayProbeReport {
        relay: relay_display,
        transport,
        room,
        a_to_b_bytes: payload.len(),
        b_to_a_bytes: payload.len(),
        payload_bytes: payload.len(),
    })
}

pub(super) fn probe_payload(payload_bytes: usize) -> Bytes {
    Bytes::from(
        (0..payload_bytes)
            .map(|index| (index.wrapping_mul(31).wrapping_add(17) & 0xff) as u8)
            .collect::<Vec<_>>(),
    )
}

pub(super) struct ProbePeer {
    transport: RelayTransport,
    steam_id64: &'static str,
}

impl ProbePeer {
    pub(super) async fn join(
        relay: &RelayEndpoint,
        transport: TransportChoice,
        room: &str,
        steam_id64: &'static str,
        display_name: &str,
    ) -> io::Result<Self> {
        let config = SessionConfig {
            relay: relay.clone(),
            relay_name: None,
            transport,
            room: room.to_owned(),
            mode: SessionMode::Pure,
            steam_id64: steam_id64.to_owned(),
            display_name: display_name.to_owned(),
            session_health: super::session_config::SessionHealthConfig::default(),
            udp_fec: Default::default(),
            #[cfg(feature = "internal-test")]
            test_run_id: None,
        };
        let mut relay_transport = RelayTransport::connect(relay, transport).await?;
        complete_relay_join(
            &mut relay_transport.sender,
            &mut relay_transport.receiver,
            &config,
        )
        .await?;
        Ok(Self {
            transport: relay_transport,
            steam_id64,
        })
    }

    async fn send_game(&mut self, to_steam_id64: &str, payload: Bytes) -> io::Result<()> {
        self.send_game_with_sequence(to_steam_id64, payload, 1)
            .await
    }

    pub(super) async fn send_game_with_sequence(
        &mut self,
        to_steam_id64: &str,
        payload: Bytes,
        source_sequence: u32,
    ) -> io::Result<()> {
        let packet = GamePacket {
            from_steam_id64: self.steam_id64.to_owned(),
            to_steam_id64: to_steam_id64.parse().map_err(io::Error::other)?,
            source_sequence,
            channel: 0,
            send_type: 0,
            payload,
        };
        let payload = packet.encode().map_err(io::Error::other)?;
        let bytes = Envelope::new(MessageType::Data, payload)
            .encode()
            .map_err(io::Error::other)?;
        self.transport.sender.send_datagram(bytes).await
    }

    async fn expect_game(
        &mut self,
        from_steam_id64: &str,
        to_steam_id64: &str,
        payload: &Bytes,
    ) -> io::Result<()> {
        let wait_for_data = async {
            loop {
                let raw = self.transport.receiver.recv_datagram().await?;
                let envelope = Envelope::decode(raw).map_err(io::Error::other)?;
                if envelope.message_type != MessageType::Data {
                    continue;
                }
                let packet = GamePacket::decode(&envelope.payload).map_err(io::Error::other)?;
                if packet.from_steam_id64 != from_steam_id64 {
                    return Err(io::Error::other(format!(
                        "unexpected probe sender {}",
                        packet.from_steam_id64
                    )));
                }
                if packet.to_steam_id64.to_string() != to_steam_id64 {
                    return Err(io::Error::other(format!(
                        "unexpected probe target {}",
                        packet.to_steam_id64
                    )));
                }
                if packet.payload != *payload {
                    return Err(io::Error::other("unexpected probe payload"));
                }
                return Ok(());
            }
        };
        time::timeout(DATA_TIMEOUT, wait_for_data)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "relay probe data timed out"))?
    }

    pub(super) async fn expect_game_with_timeout(
        &mut self,
        from_steam_id64: &str,
        to_steam_id64: &str,
        source_sequence: u32,
        payload: &Bytes,
        timeout: Duration,
    ) -> io::Result<()> {
        let wait_for_data = async {
            loop {
                let raw = self.transport.receiver.recv_datagram().await?;
                let envelope = Envelope::decode(raw).map_err(io::Error::other)?;
                if envelope.message_type != MessageType::Data {
                    continue;
                }
                let packet = GamePacket::decode(&envelope.payload).map_err(io::Error::other)?;
                if packet.from_steam_id64 == from_steam_id64
                    && packet.to_steam_id64.to_string() == to_steam_id64
                    && packet.source_sequence == source_sequence
                    && packet.payload == *payload
                {
                    return Ok(());
                }
            }
        };
        time::timeout(timeout, wait_for_data)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "readiness sample timed out"))?
    }
}

fn block_on_probe<T>(future: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(future)
}

pub(super) fn run_hook_receive_probe(
    hook_log_path: Option<PathBuf>,
) -> io::Result<HookReceiveProbeReport> {
    let hook_log_path = hook_log_path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Hook log path is unavailable; start a Native Hook session first",
        )
    })?;
    block_on_probe(run_hook_receive_probe_async(hook_log_path))
}

async fn run_hook_receive_probe_async(
    hook_log_path: PathBuf,
) -> io::Result<HookReceiveProbeReport> {
    let peer = hook_probe_peer();
    let payload = Bytes::from(format!("basement-bridge-hook-probe-{peer}"));
    let game = GamePacket {
        from_steam_id64: peer.to_string(),
        to_steam_id64: 0,
        source_sequence: 1,
        channel: 0,
        send_type: 0,
        payload,
    };
    let packet = LocalPacket::incoming(peer, 1, game);
    let bytes = packet.encode().map_err(io::Error::other)?;
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.send_to(&bytes, HOOK_OUT)?;

    let mut report = HookReceiveProbeReport {
        peer,
        sent_bytes: bytes.len(),
        local_in: false,
        available_hit: false,
        read_hit: false,
    };
    let probe_result = time::timeout(HOOK_PROBE_TIMEOUT, async {
        loop {
            update_hook_probe_report(&mut report, &hook_log_path);
            if report.local_in && report.read_hit {
                return;
            }
            time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    update_hook_probe_report(&mut report, &hook_log_path);
    if probe_result.is_err() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("hook receive probe timed out: {report}"),
        ));
    }
    Ok(report)
}

fn hook_probe_peer() -> u64 {
    4_000_000_000 + (u64::from(std::process::id()) % 1_000) * 100_000 + (unix_seconds() % 100_000)
}

fn update_hook_probe_report(report: &mut HookReceiveProbeReport, hook_log_path: &Path) {
    let Ok(contents) = fs::read_to_string(hook_log_path) else {
        return;
    };
    let peer_marker = format!("peer={}", report.peer);
    for line in contents.lines().rev().take(512) {
        if !line.contains(&peer_marker) {
            continue;
        }
        report.local_in |= line.contains("local_in");
        report.available_hit |= line.contains("steam_available_bridge_hit");
        report.read_hit |= line.contains("steam_read_bridge_hit");
        if report.local_in && report.available_hit && report.read_hit {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        net::{SocketAddr, UdpSocket},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    use crate::protocol::ControlMessage;

    use super::*;

    #[test]
    #[ignore = "requires BASEMENT_BRIDGE_RELAY=host:port and a running Relay Server"]
    fn probes_configured_relay() {
        let relay =
            std::env::var("BASEMENT_BRIDGE_RELAY").expect("set BASEMENT_BRIDGE_RELAY=host:port");
        let (host, port) = relay
            .rsplit_once(':')
            .expect("BASEMENT_BRIDGE_RELAY must be host:port");
        let port = port.parse().expect("relay port must be a u16");

        let report = run_relay_probe(
            RelayEndpoint::new(host, port),
            TransportChoice::Udp,
            DEFAULT_RELAY_PROBE_PAYLOAD_BYTES,
        )
        .unwrap();

        println!("{report}");
    }

    #[test]
    fn rejects_oversized_relay_probe_payload() {
        let result = run_relay_probe(
            RelayEndpoint::new("127.0.0.1", 1),
            TransportChoice::Udp,
            MAX_RELAY_PROBE_PAYLOAD_BYTES + 1,
        );

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn probes_local_udp_relay() {
        let relay = TestRelay::spawn();

        let report = run_relay_probe(
            RelayEndpoint::new("127.0.0.1", relay.address.port()),
            TransportChoice::Udp,
            512,
        )
        .unwrap();

        assert_eq!(report.a_to_b_bytes, 512);
        assert_eq!(report.b_to_a_bytes, 512);
        relay.stop();
    }

    struct TestRelay {
        address: SocketAddr,
        stop: Arc<AtomicBool>,
        worker: thread::JoinHandle<()>,
    }

    impl TestRelay {
        fn spawn() -> Self {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
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

    fn run_test_relay(socket: UdpSocket, stop: &AtomicBool) {
        let mut peers = HashMap::new();
        let mut buffer = [0_u8; 65_535];
        while !stop.load(Ordering::Relaxed) {
            let Ok((size, address)) = socket.recv_from(&mut buffer) else {
                continue;
            };
            let raw = Bytes::copy_from_slice(&buffer[..size]);
            let Ok(envelope) = Envelope::decode(raw.clone()) else {
                continue;
            };
            match envelope.message_type {
                MessageType::Join => handle_join(&socket, address, &envelope, &mut peers),
                MessageType::Data => forward_data(&socket, raw, &envelope, &peers),
                _ => {}
            }
        }
    }

    fn handle_join(
        socket: &UdpSocket,
        address: SocketAddr,
        envelope: &Envelope,
        peers: &mut HashMap<String, SocketAddr>,
    ) {
        let Ok(control) = ControlMessage::decode(&envelope.payload) else {
            return;
        };
        let response = match control {
            ControlMessage::Join {
                steam_id64,
                challenge: None,
                ..
            } => {
                peers.insert(steam_id64, address);
                (
                    MessageType::JoinChallenge,
                    ControlMessage::Challenge {
                        token: "token".to_owned(),
                    },
                )
            }
            ControlMessage::Join {
                steam_id64,
                challenge: Some(_),
                ..
            } => {
                peers.insert(steam_id64, address);
                (
                    MessageType::JoinReady,
                    ControlMessage::Ready {
                        peer_count: 1,
                        udp_fec: None,
                    },
                )
            }
            _ => return,
        };
        send_control(socket, address, response.0, &response.1);
    }

    fn forward_data(
        socket: &UdpSocket,
        raw: Bytes,
        envelope: &Envelope,
        peers: &HashMap<String, SocketAddr>,
    ) {
        let Ok(game) = GamePacket::decode(&envelope.payload) else {
            return;
        };
        let Some(address) = peers.get(&game.to_steam_id64.to_string()) else {
            return;
        };
        socket.send_to(&raw, address).unwrap();
    }

    fn send_control(
        socket: &UdpSocket,
        address: SocketAddr,
        message_type: MessageType,
        message: &ControlMessage,
    ) {
        let payload = message.encode().unwrap();
        let raw = Envelope::new(message_type, payload).encode().unwrap();
        socket.send_to(&raw, address).unwrap();
    }
}
