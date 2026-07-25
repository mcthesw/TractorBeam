use std::{
    collections::HashMap,
    fmt, io,
    time::{Duration, Instant},
};

use bytes::Bytes;
use rand::RngExt as _;
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
    pub(super) hook_sequence: u32,
    pub(super) delivery_sequence: u64,
    pub(super) channel: i32,
    pub(super) send_type: i32,
    pub(super) payload_bytes: usize,
    pub(super) wire_bytes: usize,
}

#[derive(Clone, Debug)]
pub(super) struct OutboundGamePacket {
    pub(super) to_steam_id64: u64,
    pub(super) hook_sequence: u32,
    pub(super) delivery_stream_id: DeliveryStreamId,
    pub(super) delivery_sequence: u64,
    pub(super) channel: i32,
    pub(super) send_type: i32,
    pub(super) payload: Bytes,
}

#[derive(Clone, Debug)]
pub(super) struct InboundGamePacket {
    pub(super) from_steam_id64: u64,
    pub(super) delivery_stream_id: DeliveryStreamId,
    pub(super) delivery_sequence: u64,
    pub(super) channel: i32,
    pub(super) send_type: i32,
    pub(super) payload: Bytes,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct DeliveryStreamId([u8; 16]);

impl DeliveryStreamId {
    pub(super) const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub(super) const fn as_bytes(self) -> [u8; 16] {
        self.0
    }

    fn random() -> Self {
        loop {
            let bytes = rand::rng().random::<[u8; 16]>();
            if bytes.iter().any(|byte| *byte != 0) {
                return Self(bytes);
            }
        }
    }
}

impl fmt::Debug for DeliveryStreamId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeliveryStreamId([REDACTED])")
    }
}

#[derive(Debug)]
struct DeliveryCursor {
    stream_id: DeliveryStreamId,
    next_sequence: u64,
}

#[derive(Debug, Default)]
pub(super) struct DeliveryStreamAllocator {
    by_target: HashMap<u64, DeliveryCursor>,
}

impl DeliveryStreamAllocator {
    pub(super) fn assign_hook_packet(&mut self, packet: HookGamePacket) -> OutboundGamePacket {
        let cursor = self
            .by_target
            .entry(packet.peer)
            .or_insert_with(|| DeliveryCursor {
                stream_id: DeliveryStreamId::random(),
                next_sequence: 1,
            });
        if cursor.next_sequence == u64::MAX {
            cursor.stream_id = DeliveryStreamId::random();
            cursor.next_sequence = 1;
        }
        let outbound = OutboundGamePacket {
            to_steam_id64: packet.peer,
            hook_sequence: packet.sequence,
            delivery_stream_id: cursor.stream_id,
            delivery_sequence: cursor.next_sequence,
            channel: packet.channel,
            send_type: packet.send_type,
            payload: Bytes::from(packet.payload),
        };
        cursor.next_sequence = cursor.next_sequence.saturating_add(1);
        outbound
    }
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
                        "Hook -> network packet #{}: to={} hook_sequence={} channel={} send_type={} payload_bytes={} wire_bytes={}",
                        self.hook_packets,
                        summary.peer,
                        summary.hook_sequence,
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
                        "Network -> Hook packet #{}: from={} delivery_sequence={} hook_sequence={} channel={} send_type={} payload_bytes={} local_bytes={}",
                        self.network_packets,
                        summary.peer,
                        summary.delivery_sequence,
                        summary.hook_sequence,
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

pub(super) fn decode_inbound_relay_datagram(
    bytes: Bytes,
) -> io::Result<Option<InboundRelayDatagram>> {
    match decode_frame(bytes).map_err(io::Error::other)? {
        Frame::Data(game) => Ok(Some(InboundRelayDatagram::Game(InboundGamePacket {
            from_steam_id64: game.from_steam_id64,
            delivery_stream_id: DeliveryStreamId::from_bytes(
                game.delivery_stream_id.as_bytes().to_owned(),
            ),
            delivery_sequence: game.delivery_sequence,
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
        hook_sequence: *local_sequence,
        delivery_sequence: inbound.delivery_sequence,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hook_packet(target: u64) -> HookGamePacket {
        HookGamePacket {
            peer: target,
            sequence: 1,
            channel: 0,
            send_type: 0,
            payload: Vec::new(),
        }
    }

    #[test]
    fn delivery_streams_are_independent_per_target() {
        let mut allocator = DeliveryStreamAllocator::default();
        let b1 = allocator.assign_hook_packet(hook_packet(2));
        let c1 = allocator.assign_hook_packet(hook_packet(3));
        let b2 = allocator.assign_hook_packet(hook_packet(2));
        let c2 = allocator.assign_hook_packet(hook_packet(3));

        assert_eq!((b1.delivery_sequence, b2.delivery_sequence), (1, 2));
        assert_eq!((c1.delivery_sequence, c2.delivery_sequence), (1, 2));
        assert_eq!(b1.delivery_stream_id, b2.delivery_stream_id);
        assert_eq!(c1.delivery_stream_id, c2.delivery_stream_id);
        assert_ne!(b1.delivery_stream_id, c1.delivery_stream_id);
    }

    #[test]
    fn sequence_exhaustion_rotates_the_delivery_stream() {
        let mut allocator = DeliveryStreamAllocator::default();
        let first = allocator.assign_hook_packet(hook_packet(2));
        let cursor = allocator.by_target.get_mut(&2).unwrap();
        cursor.next_sequence = u64::MAX;
        let rotated = allocator.assign_hook_packet(hook_packet(2));

        assert_eq!(rotated.delivery_sequence, 1);
        assert_ne!(rotated.delivery_stream_id, first.delivery_stream_id);
    }

    #[test]
    fn hook_packet_conversion_preserves_route_neutral_fields() {
        let packet = HookGamePacket {
            peer: 76_561_198_000_000_002,
            sequence: 42,
            channel: 3,
            send_type: 1,
            payload: vec![1, 2, 3],
        };

        let outbound = DeliveryStreamAllocator::default().assign_hook_packet(packet);

        assert_eq!(outbound.to_steam_id64, 76_561_198_000_000_002);
        assert_eq!(outbound.hook_sequence, 42);
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
            delivery_stream_id: crate::protocol::DeliveryStreamId::from_bytes([4; 16]),
            delivery_sequence: 42,
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
        assert_eq!(inbound.delivery_sequence, 42);
        assert_eq!(inbound.channel, 3);
        assert_eq!(inbound.send_type, 1);
        assert_eq!(inbound.payload, Bytes::from_static(&[1, 2, 3]));
    }
}
