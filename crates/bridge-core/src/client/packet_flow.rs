use std::{
    collections::HashMap,
    io,
    time::{Duration, Instant},
};

use bytes::Bytes;

use crate::protocol::{
    ControlMessage, Envelope, GamePacket, LocalPacket, LocalPacketType, MessageType,
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
pub(super) struct OutboundRelayPacket {
    pub(super) raw: Bytes,
    pub(super) summary: PacketSummary,
    pub(super) sent_bytes: u64,
}

#[derive(Clone, Debug)]
pub(super) struct InboundGamePacket {
    pub(super) game: GamePacket,
}

#[derive(Clone, Debug)]
pub(super) enum InboundRelayDatagram {
    Game(InboundGamePacket),
    HealthPong { id: u64 },
}

#[derive(Debug, Default)]
pub(super) struct PacketObserver {
    hook_packets: u64,
    relay_packets: u64,
    last_hook_packet_at: Option<Instant>,
    last_relay_packet_at: Option<Instant>,
    last_remote_sequences: HashMap<u64, u32>,
}

impl PacketObserver {
    pub(super) fn observe_hook_packet(
        &mut self,
        event_tx: &RuntimeEventSender,
        summary: &PacketSummary,
    ) {
        observe_packet_gap(event_tx, "Hook -> Relay", &mut self.last_hook_packet_at);
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
                        "Hook -> Relay packet #{}: to={} sequence={} channel={} send_type={} payload_bytes={} wire_bytes={}",
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

    pub(super) fn observe_relay_packet(
        &mut self,
        event_tx: &RuntimeEventSender,
        summary: &PacketSummary,
    ) {
        observe_packet_gap(event_tx, "Relay -> Hook", &mut self.last_relay_packet_at);
        observe_source_sequence(event_tx, &mut self.last_remote_sequences, summary);
        self.relay_packets = self.relay_packets.saturating_add(1);
        if self.relay_packets == 1 {
            send_event(
                event_tx,
                log_event(LogLevel::Info, "First relay packet received"),
            );
        }
        if should_sample_packet(self.relay_packets) {
            send_event(
                event_tx,
                log_event(
                    LogLevel::Debug,
                    format!(
                        "Relay -> Hook packet #{}: from={} source_sequence={} local_sequence={} channel={} send_type={} payload_bytes={} local_bytes={}",
                        self.relay_packets,
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

pub(super) fn encode_outbound_relay_packet(
    steam_id64: &str,
    bytes: Bytes,
) -> io::Result<OutboundRelayPacket> {
    let packet = LocalPacket::decode(bytes).map_err(io::Error::other)?;
    if packet.packet_type != LocalPacketType::Outgoing {
        return Err(io::Error::other("expected outgoing local packet"));
    }
    let summary = PacketSummary {
        peer: packet.peer,
        sequence: packet.sequence,
        source_sequence: packet.sequence,
        channel: packet.channel,
        send_type: packet.send_type,
        payload_bytes: packet.payload.len(),
        wire_bytes: 0,
    };
    let sent_bytes = u64::try_from(packet.payload.len()).unwrap_or(u64::MAX);
    let game = GamePacket::from_local(steam_id64.to_owned(), packet);
    let payload = game.encode().map_err(io::Error::other)?;
    let raw = Envelope::new(MessageType::Data, payload)
        .encode()
        .map_err(io::Error::other)?;
    Ok(OutboundRelayPacket {
        summary: PacketSummary {
            wire_bytes: raw.len(),
            ..summary
        },
        raw,
        sent_bytes,
    })
}

pub(super) fn decode_inbound_relay_datagram(
    bytes: Bytes,
) -> io::Result<Option<InboundRelayDatagram>> {
    let envelope = Envelope::decode(bytes).map_err(io::Error::other)?;
    match envelope.message_type {
        MessageType::Data => {
            let game = GamePacket::decode(&envelope.payload).map_err(io::Error::other)?;
            Ok(Some(InboundRelayDatagram::Game(InboundGamePacket { game })))
        }
        MessageType::Heartbeat => match ControlMessage::decode(&envelope.payload) {
            Ok(ControlMessage::HealthPong { id }) => {
                Ok(Some(InboundRelayDatagram::HealthPong { id }))
            }
            Ok(_) => Ok(None),
            Err(error) => Err(io::Error::other(error)),
        },
        _ => Ok(None),
    }
}

pub(super) fn encode_inbound_local_packet(
    inbound: InboundGamePacket,
    local_sequence: &mut u32,
) -> io::Result<(Bytes, PacketSummary, u64)> {
    let game = inbound.game;
    let peer = game.from_steam_id64.parse::<u64>().unwrap_or_default();
    let received_bytes = u64::try_from(game.payload.len()).unwrap_or(u64::MAX);
    let summary = PacketSummary {
        peer,
        sequence: *local_sequence,
        source_sequence: game.source_sequence,
        channel: game.channel,
        send_type: game.send_type,
        payload_bytes: game.payload.len(),
        wire_bytes: 0,
    };
    let packet = LocalPacket::incoming(peer, *local_sequence, game);
    *local_sequence = local_sequence.saturating_add(1);
    let bytes = packet.encode().map_err(io::Error::other)?;
    let wire_bytes = bytes.len();
    Ok((
        bytes,
        PacketSummary {
            wire_bytes,
            ..summary
        },
        received_bytes,
    ))
}

pub(super) fn send_error(event_tx: &RuntimeEventSender, message: impl Into<String>) {
    send_event(event_tx, log_event(LogLevel::Warn, message.into()));
    send_event(event_tx, RuntimeEvent::CounterDelta(error_counter()));
}

pub(super) fn hook_counter(sent_bytes: u64) -> Counters {
    Counters {
        hook_to_relay: 1,
        sent_bytes,
        ..Counters::default()
    }
}

pub(super) fn relay_counter(received_bytes: u64) -> Counters {
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
                "Relay source sequence gap: from={} previous={} expected={} current={}",
                summary.peer, *previous, expected, summary.source_sequence
            ),
        ),
    );
    if summary.source_sequence > *previous {
        *previous = summary.source_sequence;
    }
}
