use std::{
    collections::HashMap,
    io,
    time::{Duration, Instant},
};

use bytes::Bytes;
use tractor_beam_hook_ipc::GamePacket as HookGamePacket;

use crate::protocol::{
    Frame, PeerPresenceInfo, ProbeFrame, ServerControl, decode_frame, decode_server_control,
};

use super::{
    Counters, LogLevel,
    state::{RuntimeEvent, RuntimeEventSender, error_counter, log_event, send_event},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PacketSummary {
    pub(super) peer: u64,
    pub(super) sequence: u32,
    pub(super) source_sequence: u32,
    pub(super) channel: i32,
    pub(super) send_type: i32,
    pub(super) payload_bytes: usize,
    pub(super) wire_bytes: usize,
}

#[derive(Clone, Debug)]
pub(super) struct OutboundGamePacket {
    pub(super) to_steam_id64: u64,
    pub(super) source_sequence: u32,
    pub(super) channel: i32,
    pub(super) send_type: i32,
    pub(super) payload: Bytes,
}

#[derive(Clone, Debug)]
pub(super) struct InboundGamePacket {
    pub(super) from_steam_id64: u64,
    pub(super) source_sequence: u32,
    pub(super) channel: i32,
    pub(super) send_type: i32,
    pub(super) payload: Bytes,
}

#[derive(Clone, Debug)]
pub(super) enum InboundRelayDatagram {
    Game(InboundGamePacket),
    HealthPong { id: u64 },
    PeerPresence { peers: Vec<PeerPresenceInfo> },
    Probe(ProbeFrame),
}

#[derive(Debug, Default)]
pub(super) struct PacketObserver {
    hook_packets: u64,
    network_packets: u64,
    last_hook_packet_at: Option<Instant>,
    last_network_packet_at: Option<Instant>,
    last_remote_sequences: HashMap<u64, u32>,
}

impl PacketObserver {
    pub(super) fn observe_hook_packet(
        &mut self,
        event_tx: &RuntimeEventSender,
        summary: &PacketSummary,
    ) {
        observe_packet_gap(event_tx, "Hook -> network", &mut self.last_hook_packet_at);
        self.hook_packets = self.hook_packets.saturating_add(1);
        if self.hook_packets == 1 {
            send_event(
                event_tx,
                log_event(LogLevel::Info, "First hook packet received"),
            );
        }
        if should_sample_packet(self.hook_packets) {
            send_event(
                event_tx,
                log_event(
                    LogLevel::Debug,
                    format!(
                        "Hook -> network packet #{}: to={} sequence={} channel={} send_type={} payload_bytes={} wire_bytes={}",
                        self.hook_packets,
                        summary.peer,
                        summary.sequence,
                        summary.channel,
                        summary.send_type,
                        summary.payload_bytes,
                        summary.wire_bytes
                    ),
                ),
            );
        }
    }

    pub(super) fn observe_network_packet(
        &mut self,
        event_tx: &RuntimeEventSender,
        summary: &PacketSummary,
    ) {
        observe_packet_gap(
            event_tx,
            "Network -> Hook",
            &mut self.last_network_packet_at,
        );
        observe_source_sequence(event_tx, &mut self.last_remote_sequences, summary);
        self.network_packets = self.network_packets.saturating_add(1);
        if self.network_packets == 1 {
            send_event(
                event_tx,
                log_event(LogLevel::Info, "First network packet received"),
            );
        }
        if should_sample_packet(self.network_packets) {
            send_event(
                event_tx,
                log_event(
                    LogLevel::Debug,
                    format!(
                        "Network -> Hook packet #{}: from={} source_sequence={} local_sequence={} channel={} send_type={} payload_bytes={} local_bytes={}",
                        self.network_packets,
                        summary.peer,
                        summary.source_sequence,
                        summary.sequence,
                        summary.channel,
                        summary.send_type,
                        summary.payload_bytes,
                        summary.wire_bytes
                    ),
                ),
            );
        }
    }
}

pub(super) fn decode_outbound_hook_packet(packet: HookGamePacket) -> OutboundGamePacket {
    OutboundGamePacket {
        to_steam_id64: packet.peer,
        source_sequence: packet.sequence,
        channel: packet.channel,
        send_type: packet.send_type,
        payload: Bytes::from(packet.payload),
    }
}

pub(super) fn decode_inbound_relay_datagram(
    bytes: Bytes,
) -> io::Result<Option<InboundRelayDatagram>> {
    match decode_frame(bytes).map_err(io::Error::other)? {
        Frame::Data(game) => Ok(Some(InboundRelayDatagram::Game(InboundGamePacket {
            from_steam_id64: game.from_steam_id64,
            source_sequence: game.source_sequence,
            channel: game.channel,
            send_type: game.send_type,
            payload: game.payload,
        }))),
        Frame::Probe(probe) => Ok(Some(InboundRelayDatagram::Probe(probe))),
        Frame::ServerControl(payload) => match decode_server_control(&payload)
            .map_err(io::Error::other)?
        {
            ServerControl::ControlPong { id } => Ok(Some(InboundRelayDatagram::HealthPong { id })),
            ServerControl::PeerPresenceUpdate { peers } => {
                Ok(Some(InboundRelayDatagram::PeerPresence { peers }))
            }
            ServerControl::Error { code, message, .. } => {
                Err(io::Error::other(format!("{code:?}: {message}")))
            }
            _ => Ok(None),
        },
        Frame::ClientControl(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "relay sent a client control frame",
        )),
    }
}

pub(super) fn encode_inbound_hook_packet(
    inbound: InboundGamePacket,
    local_sequence: &mut u32,
) -> (HookGamePacket, PacketSummary, u64) {
    let received_bytes = u64::try_from(inbound.payload.len()).unwrap_or(u64::MAX);
    let summary = PacketSummary {
        peer: inbound.from_steam_id64,
        sequence: *local_sequence,
        source_sequence: inbound.source_sequence,
        channel: inbound.channel,
        send_type: inbound.send_type,
        payload_bytes: inbound.payload.len(),
        wire_bytes: 0,
    };
    let packet = HookGamePacket {
        peer: inbound.from_steam_id64,
        sequence: *local_sequence,
        channel: inbound.channel,
        send_type: inbound.send_type,
        payload: inbound.payload.to_vec(),
    };
    *local_sequence = local_sequence.saturating_add(1);
    (
        packet,
        PacketSummary {
            wire_bytes: summary.payload_bytes,
            ..summary
        },
        received_bytes,
    )
}

pub(super) fn send_error(event_tx: &RuntimeEventSender, message: impl Into<String>) {
    send_event(event_tx, log_event(LogLevel::Warn, message.into()));
    send_event(event_tx, RuntimeEvent::CounterDelta(error_counter()));
}

pub(super) fn network_out_counter(sent_bytes: u64) -> Counters {
    Counters {
        hook_to_relay: 1,
        sent_bytes,
        ..Counters::default()
    }
}

pub(super) fn network_in_counter(received_bytes: u64) -> Counters {
    Counters {
        relay_to_hook: 1,
        received_bytes,
        ..Counters::default()
    }
}

fn should_sample_packet(count: u64) -> bool {
    count <= 64 || count.is_multiple_of(1_000)
}

fn observe_packet_gap(
    event_tx: &RuntimeEventSender,
    direction: &str,
    last_packet_at: &mut Option<Instant>,
) {
    let now = Instant::now();
    if let Some(previous) = last_packet_at.replace(now) {
        let gap = now.duration_since(previous);
        if gap >= Duration::from_millis(1_000) {
            send_event(
                event_tx,
                log_event(
                    LogLevel::Debug,
                    format!("{direction} packet gap: {} ms", gap.as_millis()),
                ),
            );
        }
    }
}

fn observe_source_sequence(
    event_tx: &RuntimeEventSender,
    last_remote_sequences: &mut HashMap<u64, u32>,
    summary: &PacketSummary,
) {
    if summary.source_sequence == 0 {
        return;
    }
    let Some(previous) = last_remote_sequences.get_mut(&summary.peer) else {
        last_remote_sequences.insert(summary.peer, summary.source_sequence);
        return;
    };
    let expected = previous.saturating_add(1);
    if summary.source_sequence == expected {
        *previous = summary.source_sequence;
        return;
    }
    send_event(
        event_tx,
        log_event(
            LogLevel::Debug,
            format!(
                "Network source sequence gap: from={} previous={} expected={} current={}",
                summary.peer, *previous, expected, summary.source_sequence
            ),
        ),
    );
    if summary.source_sequence > *previous {
        *previous = summary.source_sequence;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_packet_conversion_preserves_route_neutral_fields() {
        let packet = HookGamePacket {
            peer: 76_561_198_000_000_002,
            sequence: 42,
            channel: 3,
            send_type: 1,
            payload: vec![1, 2, 3],
        };

        let outbound = decode_outbound_hook_packet(packet);

        assert_eq!(outbound.to_steam_id64, 76_561_198_000_000_002);
        assert_eq!(outbound.source_sequence, 42);
        assert_eq!(outbound.channel, 3);
        assert_eq!(outbound.send_type, 1);
        assert_eq!(outbound.payload, Bytes::from_static(&[1, 2, 3]));
    }

    #[test]
    fn relay_adapter_decodes_to_route_neutral_inbound_packet() {
        let frame = crate::protocol::DataFrame {
            connection_id: 7,
            frame_id: 8,
            from_steam_id64: 76_561_198_000_000_002,
            to_steam_id64: 76_561_198_000_000_001,
            source_sequence: 42,
            channel: 3,
            send_type: 1,
            payload: Bytes::from_static(&[1, 2, 3]),
        }
        .encode()
        .unwrap();

        let decoded = decode_inbound_relay_datagram(frame).unwrap().unwrap();
        let InboundRelayDatagram::Game(inbound) = decoded else {
            panic!("expected game packet");
        };

        assert_eq!(inbound.from_steam_id64, 76_561_198_000_000_002);
        assert_eq!(inbound.source_sequence, 42);
        assert_eq!(inbound.channel, 3);
        assert_eq!(inbound.send_type, 1);
        assert_eq!(inbound.payload, Bytes::from_static(&[1, 2, 3]));
    }
}
