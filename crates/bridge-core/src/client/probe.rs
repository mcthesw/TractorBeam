use std::{
    fmt::{self, Display},
    fs, io,
    net::UdpSocket,
    time::{Duration, Instant},
};

use bytes::Bytes;

use crate::protocol::{ControlMessage, Envelope, GamePacket, LocalPacket, MessageType};

use super::{BridgeClient, LogLevel, RelayEndpoint, hook_config::HOOK_OUT, state::unix_seconds};

const PROBE_A_STEAM: &str = "76561198000000101";
const PROBE_B_STEAM: &str = "76561198000000102";
pub const DEFAULT_RELAY_PROBE_PAYLOAD_BYTES: usize = 2_048;
const MAX_RELAY_PROBE_PAYLOAD_BYTES: usize = 60_000;
const SOCKET_TIMEOUT: Duration = Duration::from_millis(500);
const JOIN_TIMEOUT: Duration = Duration::from_secs(3);
const DATA_TIMEOUT: Duration = Duration::from_secs(3);
const HOOK_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayProbeReport {
    pub relay: String,
    pub room: String,
    pub a_to_b_bytes: usize,
    pub b_to_a_bytes: usize,
    pub payload_bytes: usize,
}

impl RelayProbeReport {
    #[must_use]
    pub fn short_summary(&self) -> String {
        format!(
            "Relay probe passed: {} byte payload, {} bytes A->B, {} bytes B->A via {}",
            self.payload_bytes, self.a_to_b_bytes, self.b_to_a_bytes, self.relay
        )
    }
}

impl Display for RelayProbeReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}; room={}", self.short_summary(), self.room)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

impl BridgeClient {
    pub fn run_relay_probe(&mut self, relay: RelayEndpoint) -> io::Result<RelayProbeReport> {
        self.run_relay_probe_with_payload(relay, DEFAULT_RELAY_PROBE_PAYLOAD_BYTES)
    }

    pub fn run_relay_probe_with_payload(
        &mut self,
        relay: RelayEndpoint,
        payload_bytes: usize,
    ) -> io::Result<RelayProbeReport> {
        relay.validate().map_err(io::Error::other)?;
        let report = run_relay_probe(relay, payload_bytes)?;
        self.log(LogLevel::Info, report.to_string());
        Ok(report)
    }

    pub fn run_hook_receive_probe(&mut self) -> io::Result<HookReceiveProbeReport> {
        let report = run_hook_receive_probe()?;
        self.log(LogLevel::Info, report.to_string());
        Ok(report)
    }
}

fn run_relay_probe(relay: RelayEndpoint, payload_bytes: usize) -> io::Result<RelayProbeReport> {
    if !(1..=MAX_RELAY_PROBE_PAYLOAD_BYTES).contains(&payload_bytes) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("relay probe payload must be 1..={MAX_RELAY_PROBE_PAYLOAD_BYTES} bytes"),
        ));
    }
    let room = format!("bb-probe-{}-{}", std::process::id(), unix_seconds());
    let relay_display = relay.to_string();
    let peer_a = ProbePeer::join(&relay_display, &room, PROBE_A_STEAM, "Probe A")?;
    let peer_b = ProbePeer::join(&relay_display, &room, PROBE_B_STEAM, "Probe B")?;

    let payload = probe_payload(payload_bytes);
    peer_a.send_game(PROBE_B_STEAM, payload.clone())?;
    peer_b.expect_game(PROBE_A_STEAM, PROBE_B_STEAM, &payload)?;

    peer_b.send_game(PROBE_A_STEAM, payload.clone())?;
    peer_a.expect_game(PROBE_B_STEAM, PROBE_A_STEAM, &payload)?;

    Ok(RelayProbeReport {
        relay: relay_display,
        room,
        a_to_b_bytes: payload.len(),
        b_to_a_bytes: payload.len(),
        payload_bytes: payload.len(),
    })
}

fn probe_payload(payload_bytes: usize) -> Bytes {
    Bytes::from(
        (0..payload_bytes)
            .map(|index| (index.wrapping_mul(31).wrapping_add(17) & 0xff) as u8)
            .collect::<Vec<_>>(),
    )
}

struct ProbePeer {
    socket: UdpSocket,
    steam_id64: &'static str,
}

impl ProbePeer {
    fn join(
        relay: &str,
        room: &str,
        steam_id64: &'static str,
        display_name: &str,
    ) -> io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.connect(relay)?;
        socket.set_read_timeout(Some(SOCKET_TIMEOUT))?;
        complete_join(&socket, room, steam_id64, display_name)?;
        Ok(Self { socket, steam_id64 })
    }

    fn send_game(&self, to_steam_id64: &str, payload: Bytes) -> io::Result<()> {
        let packet = GamePacket {
            from_steam_id64: self.steam_id64.to_owned(),
            to_steam_id64: to_steam_id64.parse().map_err(io::Error::other)?,
            source_sequence: 1,
            channel: 0,
            send_type: 0,
            payload,
        };
        let payload = packet.encode().map_err(io::Error::other)?;
        let bytes = Envelope::new(MessageType::Data, payload)
            .encode()
            .map_err(io::Error::other)?;
        self.socket.send(&bytes)?;
        Ok(())
    }

    fn expect_game(
        &self,
        from_steam_id64: &str,
        to_steam_id64: &str,
        payload: &Bytes,
    ) -> io::Result<()> {
        let deadline = Instant::now() + DATA_TIMEOUT;
        let mut buffer = vec![0_u8; 65_535];
        while Instant::now() < deadline {
            match self.socket.recv(&mut buffer) {
                Ok(size) => {
                    let envelope = Envelope::decode(Bytes::copy_from_slice(&buffer[..size]))
                        .map_err(io::Error::other)?;
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
                Err(error) if would_wait(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "relay probe data timed out",
        ))
    }
}

fn complete_join(
    socket: &UdpSocket,
    room: &str,
    steam_id64: &str,
    display_name: &str,
) -> io::Result<()> {
    send_join(socket, room, steam_id64, display_name, None)?;
    let deadline = Instant::now() + JOIN_TIMEOUT;
    let mut buffer = [0_u8; 4096];
    while Instant::now() < deadline {
        match socket.recv(&mut buffer) {
            Ok(size) => {
                let envelope = Envelope::decode(Bytes::copy_from_slice(&buffer[..size]))
                    .map_err(io::Error::other)?;
                let control =
                    ControlMessage::decode(&envelope.payload).map_err(io::Error::other)?;
                match control {
                    ControlMessage::Challenge { token } => {
                        send_join(socket, room, steam_id64, display_name, Some(token))?;
                    }
                    ControlMessage::Ready { .. } => return Ok(()),
                    ControlMessage::Error { code, message } => {
                        return Err(io::Error::other(format!("{code}: {message}")));
                    }
                    _ => {}
                }
            }
            Err(error) if would_wait(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "relay probe join timed out",
    ))
}

fn send_join(
    socket: &UdpSocket,
    room: &str,
    steam_id64: &str,
    display_name: &str,
    challenge: Option<String>,
) -> io::Result<()> {
    let message = ControlMessage::Join {
        room: room.to_owned(),
        steam_id64: steam_id64.to_owned(),
        display_name: Some(display_name.to_owned()),
        challenge,
    };
    let payload = message.encode().map_err(io::Error::other)?;
    let bytes = Envelope::new(MessageType::Join, payload)
        .encode()
        .map_err(io::Error::other)?;
    socket.send(&bytes)?;
    Ok(())
}

fn would_wait(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn run_hook_receive_probe() -> io::Result<HookReceiveProbeReport> {
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
    let deadline = Instant::now() + HOOK_PROBE_TIMEOUT;
    while Instant::now() < deadline {
        update_hook_probe_report(&mut report);
        if report.local_in && report.read_hit {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    update_hook_probe_report(&mut report);
    Ok(report)
}

fn hook_probe_peer() -> u64 {
    4_000_000_000 + (u64::from(std::process::id()) % 1_000) * 100_000 + (unix_seconds() % 100_000)
}

fn update_hook_probe_report(report: &mut HookReceiveProbeReport) {
    let path =
        crate::diagnostics::isaac_online_logs_directory().join(crate::diagnostics::BRIDGE_HOOK_LOG);
    let Ok(contents) = fs::read_to_string(path) else {
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
            DEFAULT_RELAY_PROBE_PAYLOAD_BYTES,
        )
        .unwrap();

        println!("{report}");
    }

    #[test]
    fn rejects_oversized_relay_probe_payload() {
        let result = run_relay_probe(
            RelayEndpoint::new("127.0.0.1", 1),
            MAX_RELAY_PROBE_PAYLOAD_BYTES + 1,
        );

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
