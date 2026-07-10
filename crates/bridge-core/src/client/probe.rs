mod light_ping;
mod readiness;

use std::{
    fmt::{self, Display},
    future::Future,
    io,
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::Duration,
};

use bytes::Bytes;
use serde::Serialize;
use tokio::{runtime::Builder, time};

use crate::protocol::v2::{DATA_FRAME_OVERHEAD, Frame, decode_frame};

use super::{
    BridgeClient, LogLevel, RelayEndpoint, SessionConfig, SessionMode, TransportChoice,
    packet_flow::encode_outbound_relay_packet,
    relay_transport::{RelayTransport, complete_relay_join},
    state::{HookIpcState, RuntimeEvent, log_event},
};

pub(super) use light_ping::spawn_light_ping_probes;
pub use light_ping::{LightPingHandle, LightPingReport, LightPingTarget};
pub use readiness::{
    READINESS_PROBE_CONNECTION_PROFILES, READINESS_PROBE_PAYLOAD_BYTES,
    READINESS_PROBE_SAMPLES_PER_CASE, ReadinessProbeCaseReport, ReadinessProbeReport,
};

pub(super) const PROBE_A_STEAM: &str = "76561198000000101";
pub(super) const PROBE_B_STEAM: &str = "76561198000000102";
pub const DEFAULT_RELAY_PROBE_PAYLOAD_BYTES: usize = 2_048 - DATA_FRAME_OVERHEAD;
pub(super) const MAX_RELAY_PROBE_PAYLOAD_BYTES: usize = 60_000;
const DATA_TIMEOUT: Duration = Duration::from_secs(3);

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
    pub connection: String,
    pub protocol_major: Option<u16>,
    pub protocol_minor: Option<u16>,
    pub reconnects: u32,
    pub hook_data_dropped: u64,
    pub client_data_dropped: u64,
    pub malformed_frames: u64,
    pub last_error: Option<String>,
}

impl HookReceiveProbeReport {
    #[must_use]
    pub fn short_summary(&self) -> String {
        format!(
            "Hook IPC health: connection={} version={}.{} reconnects={} drops={}/{} malformed={}",
            self.connection,
            self.protocol_major
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            self.protocol_minor
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            self.reconnects,
            self.hook_data_dropped,
            self.client_data_dropped,
            self.malformed_frames,
        )
    }
}

impl Display for HookReceiveProbeReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.short_summary())?;
        if let Some(error) = &self.last_error {
            write!(formatter, "; last_error={error}")?;
        }
        Ok(())
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
        drop(self.worker.take());
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
        let report = hook_ipc_report(&self.state.hook_ipc);
        self.log(LogLevel::Info, report.to_string());
        Ok(report)
    }
}

pub(super) fn spawn_readiness_probe(relay: RelayEndpoint) -> io::Result<ProbeHandle> {
    readiness::spawn_readiness_probe(relay)
}

pub(super) fn spawn_hook_receive_probe(ipc: HookIpcState) -> ProbeHandle {
    let (event_tx, events) = mpsc::channel();
    let worker = thread::spawn(move || {
        let report = hook_ipc_report(&ipc);
        let _ = event_tx.send(log_event(LogLevel::Info, report.to_string()));
        let _ = event_tx.send(RuntimeEvent::HookReceiveProbeFinished(Ok(report)));
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
    let session_credential = crate::SessionCredential::generate();
    let relay_display = relay.to_string();
    let mut peer_a = ProbePeer::join(
        &relay,
        transport,
        session_credential,
        PROBE_A_STEAM,
        "Probe A",
    )
    .await?;
    let mut peer_b = ProbePeer::join(
        &relay,
        transport,
        session_credential,
        PROBE_B_STEAM,
        "Probe B",
    )
    .await?;

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
        room: "ephemeral probe room".to_owned(),
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
}

impl ProbePeer {
    pub(super) async fn join(
        relay: &RelayEndpoint,
        transport: TransportChoice,
        session_credential: super::SessionCredential,
        steam_id64: &'static str,
        display_name: &str,
    ) -> io::Result<Self> {
        let config = SessionConfig {
            relay: relay.clone(),
            relay_name: None,
            transport,
            session_credential,
            mode: SessionMode::Pure,
            steam_id64: steam_id64.to_owned(),
            display_name: display_name.to_owned(),
            session_health: super::session_config::SessionHealthConfig::default(),
        };
        let build = crate::build_info::current();
        let steam = steam_id64.parse::<u64>().map_err(io::Error::other)?;
        let mut relay_transport =
            RelayTransport::connect(relay, transport, build.version, build.git_hash, steam).await?;
        complete_relay_join(
            &mut relay_transport.sender,
            &mut relay_transport.receiver,
            &config,
        )
        .await?;
        Ok(Self {
            transport: relay_transport,
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
        let packet = tractor_beam_hook_ipc::GamePacket {
            peer: to_steam_id64.parse().map_err(io::Error::other)?,
            sequence: source_sequence,
            channel: 0,
            send_type: 0,
            payload: payload.to_vec(),
        };
        self.transport
            .sender
            .send_data_datagram(encode_outbound_relay_packet(packet)?)
            .await
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
                let Frame::Data(packet) = decode_frame(raw).map_err(io::Error::other)? else {
                    continue;
                };
                if packet.from_steam_id64.to_string() != from_steam_id64 {
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
                let Frame::Data(packet) = decode_frame(raw).map_err(io::Error::other)? else {
                    continue;
                };
                if packet.from_steam_id64.to_string() == from_steam_id64
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

fn hook_ipc_report(ipc: &HookIpcState) -> HookReceiveProbeReport {
    HookReceiveProbeReport {
        connection: ipc.connection.to_string(),
        protocol_major: ipc.negotiated_major,
        protocol_minor: ipc.negotiated_minor,
        reconnects: ipc.reconnects,
        hook_data_dropped: ipc.hook_data_dropped,
        client_data_dropped: ipc.client_data_dropped,
        malformed_frames: ipc.malformed_frames,
        last_error: ipc.last_error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::client::test_relay::TestRelay;

    use super::*;

    #[test]
    #[ignore = "requires TRACTOR_BEAM_RELAY=host:port and a running Relay Server"]
    fn probes_configured_relay() {
        let relay = std::env::var("TRACTOR_BEAM_RELAY").expect("set TRACTOR_BEAM_RELAY=host:port");
        let (host, port) = relay
            .rsplit_once(':')
            .expect("TRACTOR_BEAM_RELAY must be host:port");
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
    fn probes_local_tcp_relay() {
        let relay = TestRelay::spawn();

        let report = run_relay_probe(
            RelayEndpoint::new("127.0.0.1", relay.address.port()),
            TransportChoice::Tcp,
            512,
        )
        .unwrap();

        assert_eq!(report.a_to_b_bytes, 512);
        assert_eq!(report.b_to_a_bytes, 512);
        relay.stop();
    }
}
