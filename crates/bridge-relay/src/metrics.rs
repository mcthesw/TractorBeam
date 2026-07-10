use std::collections::HashMap;

use tracing::info;
use tractor_beam_relay_protocol::{IPV4_SAFE_GAME_PAYLOAD, IPV4_UDP_DATAGRAM_BUDGET};

use crate::state::RelayState;

pub(crate) const PACKET_SIZE_BUCKET_UPPER_BOUNDS: [usize; 8] =
    [64, 256, 512, 1_024, 1_100, 1_182, 1_390, 1_472];
const PACKET_SIZE_BUCKET_COUNT: usize = PACKET_SIZE_BUCKET_UPPER_BOUNDS.len() + 1;

#[derive(Debug, Default)]
pub(crate) struct RelayMetrics {
    pub(crate) tcp_egress_queue_capacity: usize,
    pub(crate) packets_in: u64,
    pub(crate) data_in: u64,
    pub(crate) forwarded_packets: u64,
    pub(crate) forwarded_bytes: u64,
    pub(crate) tcp_egress_queue_full: u64,
    pub(crate) tcp_egress_dropped_packets: u64,
    pub(crate) decode_errors: u64,
    pub(crate) unjoined_data: u64,
    pub(crate) missing_target: u64,
    pub(crate) blocked: u64,
    pub(crate) rate_limited: u64,
    pub(crate) packet_handling_errors: u64,
    pub(crate) packet_sizes: PacketSizeMetrics,
    pub(crate) room_metrics: HashMap<String, RoomMetrics>,
}

impl RelayMetrics {
    pub(crate) fn new(tcp_egress_queue_capacity: usize) -> Self {
        Self {
            tcp_egress_queue_capacity,
            ..Self::default()
        }
    }

    pub(crate) fn record_packet_in(&mut self) {
        self.packets_in = self.packets_in.saturating_add(1);
    }

    pub(crate) fn record_blocked(&mut self, room: Option<&str>) {
        self.blocked = self.blocked.saturating_add(1);
        if let Some(room) = room {
            let metrics = self.room_entry(room);
            metrics.packets_in = metrics.packets_in.saturating_add(1);
            metrics.blocked = metrics.blocked.saturating_add(1);
        }
    }

    pub(crate) fn record_rate_limited(&mut self, room: Option<&str>) {
        self.rate_limited = self.rate_limited.saturating_add(1);
        if let Some(room) = room {
            let metrics = self.room_entry(room);
            metrics.packets_in = metrics.packets_in.saturating_add(1);
            metrics.rate_limited = metrics.rate_limited.saturating_add(1);
        }
    }

    pub(crate) fn record_packet_handling_error(&mut self, room: Option<&str>) {
        self.packet_handling_errors = self.packet_handling_errors.saturating_add(1);
        if let Some(room) = room {
            let metrics = self.room_entry(room);
            metrics.packets_in = metrics.packets_in.saturating_add(1);
            metrics.packet_handling_errors = metrics.packet_handling_errors.saturating_add(1);
        }
    }

    pub(crate) fn add(&mut self, outcome: PacketOutcome) {
        self.data_in = self.data_in.saturating_add(outcome.data_in);
        self.forwarded_packets = self
            .forwarded_packets
            .saturating_add(outcome.forwarded_packets);
        self.forwarded_bytes = self.forwarded_bytes.saturating_add(outcome.forwarded_bytes);
        self.tcp_egress_queue_full = self
            .tcp_egress_queue_full
            .saturating_add(outcome.tcp_egress_queue_full);
        self.tcp_egress_dropped_packets = self
            .tcp_egress_dropped_packets
            .saturating_add(outcome.tcp_egress_dropped_packets);
        self.decode_errors = self.decode_errors.saturating_add(outcome.decode_errors);
        self.unjoined_data = self.unjoined_data.saturating_add(outcome.unjoined_data);
        self.missing_target = self.missing_target.saturating_add(outcome.missing_target);
        if let Some((payload_bytes, wire_bytes)) = outcome.game_size {
            self.packet_sizes.observe(payload_bytes, wire_bytes);
        }
        if let Some(room) = outcome.room.as_deref() {
            self.room_entry(room).add(&outcome);
        }
    }

    fn room_entry(&mut self, room: &str) -> &mut RoomMetrics {
        self.room_metrics.entry(room.to_owned()).or_default()
    }

    pub(crate) fn log_and_reset(&mut self, state: &RelayState) {
        info!(
            rooms = state.room_count(),
            peers = state.peer_count(),
            packets_in = self.packets_in,
            data_in = self.data_in,
            forwarded_packets = self.forwarded_packets,
            forwarded_bytes = self.forwarded_bytes,
            tcp_egress_queue_capacity = self.tcp_egress_queue_capacity,
            tcp_egress_queue_full = self.tcp_egress_queue_full,
            tcp_egress_dropped_packets = self.tcp_egress_dropped_packets,
            decode_errors = self.decode_errors,
            unjoined_data = self.unjoined_data,
            missing_target = self.missing_target,
            blocked = self.blocked,
            rate_limited = self.rate_limited,
            packet_handling_errors = self.packet_handling_errors,
            packet_size_bucket_upper_bounds = ?PACKET_SIZE_BUCKET_UPPER_BOUNDS,
            payload_size_buckets = ?self.packet_sizes.payload_buckets,
            wire_size_buckets = ?self.packet_sizes.wire_buckets,
            max_payload_bytes = self.packet_sizes.max_payload_bytes,
            max_wire_bytes = self.packet_sizes.max_wire_bytes,
            payload_over_ipv4_safe = self.packet_sizes.payload_over_ipv4_safe,
            wire_over_ipv4_udp = self.packet_sizes.wire_over_ipv4_udp,
            "relay stats"
        );
        let tcp_egress_queue_capacity = self.tcp_egress_queue_capacity;
        let mut room_metrics = std::mem::take(&mut self.room_metrics);
        for summary in state.room_summaries() {
            let metrics = room_metrics.remove(&summary.name).unwrap_or_default();
            if summary.peers == 0 && metrics.is_empty() {
                continue;
            }
            log_room_metrics(
                &summary.name,
                summary.peers,
                summary.tcp_peers,
                summary.udp_peers,
                &metrics,
            );
        }
        for (room, metrics) in room_metrics {
            if metrics.is_empty() {
                continue;
            }
            log_room_metrics(&room, 0, 0, 0, &metrics);
        }
        self.reset_interval(tcp_egress_queue_capacity);
    }

    fn reset_interval(&mut self, tcp_egress_queue_capacity: usize) {
        *self = Self::new(tcp_egress_queue_capacity);
    }
}

#[derive(Debug)]
pub(crate) struct PacketSizeMetrics {
    pub(crate) payload_buckets: [u64; PACKET_SIZE_BUCKET_COUNT],
    pub(crate) wire_buckets: [u64; PACKET_SIZE_BUCKET_COUNT],
    pub(crate) max_payload_bytes: usize,
    pub(crate) max_wire_bytes: usize,
    pub(crate) payload_over_ipv4_safe: u64,
    pub(crate) wire_over_ipv4_udp: u64,
}

impl Default for PacketSizeMetrics {
    fn default() -> Self {
        Self {
            payload_buckets: [0; PACKET_SIZE_BUCKET_COUNT],
            wire_buckets: [0; PACKET_SIZE_BUCKET_COUNT],
            max_payload_bytes: 0,
            max_wire_bytes: 0,
            payload_over_ipv4_safe: 0,
            wire_over_ipv4_udp: 0,
        }
    }
}

impl PacketSizeMetrics {
    fn observe(&mut self, payload_bytes: usize, wire_bytes: usize) {
        increment_bucket(&mut self.payload_buckets, payload_bytes);
        increment_bucket(&mut self.wire_buckets, wire_bytes);
        self.max_payload_bytes = self.max_payload_bytes.max(payload_bytes);
        self.max_wire_bytes = self.max_wire_bytes.max(wire_bytes);
        if payload_bytes > IPV4_SAFE_GAME_PAYLOAD {
            self.payload_over_ipv4_safe = self.payload_over_ipv4_safe.saturating_add(1);
        }
        if wire_bytes > IPV4_UDP_DATAGRAM_BUDGET {
            self.wire_over_ipv4_udp = self.wire_over_ipv4_udp.saturating_add(1);
        }
    }
}

fn increment_bucket(buckets: &mut [u64; PACKET_SIZE_BUCKET_COUNT], size: usize) {
    let index = PACKET_SIZE_BUCKET_UPPER_BOUNDS
        .iter()
        .position(|upper_bound| size <= *upper_bound)
        .unwrap_or(PACKET_SIZE_BUCKET_UPPER_BOUNDS.len());
    buckets[index] = buckets[index].saturating_add(1);
}

#[derive(Debug, Default)]
pub(crate) struct RoomMetrics {
    pub(crate) packets_in: u64,
    pub(crate) data_in: u64,
    pub(crate) forwarded_packets: u64,
    pub(crate) forwarded_bytes: u64,
    pub(crate) tcp_egress_queue_full: u64,
    pub(crate) tcp_egress_dropped_packets: u64,
    pub(crate) decode_errors: u64,
    pub(crate) missing_target: u64,
    pub(crate) blocked: u64,
    pub(crate) rate_limited: u64,
    pub(crate) packet_handling_errors: u64,
}

impl RoomMetrics {
    fn add(&mut self, outcome: &PacketOutcome) {
        self.packets_in = self.packets_in.saturating_add(outcome.room_packets_in);
        self.data_in = self.data_in.saturating_add(outcome.data_in);
        self.forwarded_packets = self
            .forwarded_packets
            .saturating_add(outcome.forwarded_packets);
        self.forwarded_bytes = self.forwarded_bytes.saturating_add(outcome.forwarded_bytes);
        self.tcp_egress_queue_full = self
            .tcp_egress_queue_full
            .saturating_add(outcome.tcp_egress_queue_full);
        self.tcp_egress_dropped_packets = self
            .tcp_egress_dropped_packets
            .saturating_add(outcome.tcp_egress_dropped_packets);
        self.decode_errors = self.decode_errors.saturating_add(outcome.decode_errors);
        self.missing_target = self.missing_target.saturating_add(outcome.missing_target);
    }

    fn is_empty(&self) -> bool {
        self.packets_in == 0
            && self.data_in == 0
            && self.forwarded_packets == 0
            && self.forwarded_bytes == 0
            && self.tcp_egress_queue_full == 0
            && self.tcp_egress_dropped_packets == 0
            && self.decode_errors == 0
            && self.missing_target == 0
            && self.blocked == 0
            && self.rate_limited == 0
            && self.packet_handling_errors == 0
    }
}

fn log_room_metrics(
    room: &str,
    peers: usize,
    tcp_peers: usize,
    udp_peers: usize,
    metrics: &RoomMetrics,
) {
    info!(
        room = %room,
        peers,
        tcp_peers,
        udp_peers,
        packets_in = metrics.packets_in,
        data_in = metrics.data_in,
        forwarded_packets = metrics.forwarded_packets,
        forwarded_bytes = metrics.forwarded_bytes,
        tcp_egress_queue_full = metrics.tcp_egress_queue_full,
        tcp_egress_dropped_packets = metrics.tcp_egress_dropped_packets,
        decode_errors = metrics.decode_errors,
        missing_target = metrics.missing_target,
        blocked = metrics.blocked,
        rate_limited = metrics.rate_limited,
        packet_handling_errors = metrics.packet_handling_errors,
        "relay room stats"
    );
}

#[derive(Debug, Default)]
pub(crate) struct PacketOutcome {
    pub(crate) room: Option<String>,
    pub(crate) room_packets_in: u64,
    pub(crate) data_in: u64,
    pub(crate) forwarded_packets: u64,
    pub(crate) forwarded_bytes: u64,
    pub(crate) tcp_egress_queue_full: u64,
    pub(crate) tcp_egress_dropped_packets: u64,
    pub(crate) decode_errors: u64,
    pub(crate) unjoined_data: u64,
    pub(crate) missing_target: u64,
    pub(crate) game_size: Option<(usize, usize)>,
}

impl PacketOutcome {
    pub(crate) fn for_room(room: String) -> Self {
        Self {
            room: Some(room),
            room_packets_in: 1,
            ..Self::default()
        }
    }

    pub(crate) fn record_game_size(&mut self, payload_bytes: usize, wire_bytes: usize) {
        self.game_size = Some((payload_bytes, wire_bytes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_size_metrics_track_boundaries_maxima_and_oversize_counts() {
        let mut metrics = RelayMetrics::new(512);
        for (payload, wire) in [(64, 64), (1_390, 1_472), (1_391, 1_473)] {
            let mut outcome = PacketOutcome::default();
            outcome.record_game_size(payload, wire);
            metrics.add(outcome);
        }

        assert_eq!(metrics.packet_sizes.payload_buckets[0], 1);
        assert_eq!(metrics.packet_sizes.payload_buckets[6], 1);
        assert_eq!(metrics.packet_sizes.payload_buckets[7], 1);
        assert_eq!(metrics.packet_sizes.wire_buckets[0], 1);
        assert_eq!(metrics.packet_sizes.wire_buckets[7], 1);
        assert_eq!(metrics.packet_sizes.wire_buckets[8], 1);
        assert_eq!(metrics.packet_sizes.max_payload_bytes, 1_391);
        assert_eq!(metrics.packet_sizes.max_wire_bytes, 1_473);
        assert_eq!(metrics.packet_sizes.payload_over_ipv4_safe, 1);
        assert_eq!(metrics.packet_sizes.wire_over_ipv4_udp, 1);
    }

    #[test]
    fn interval_reset_preserves_queue_capacity_and_clears_packet_sizes() {
        let mut metrics = RelayMetrics::new(512);
        metrics.packet_sizes.observe(1_391, 1_473);

        metrics.reset_interval(512);

        assert_eq!(metrics.tcp_egress_queue_capacity, 512);
        assert_eq!(metrics.packet_sizes.max_payload_bytes, 0);
        assert_eq!(metrics.packet_sizes.max_wire_bytes, 0);
        assert_eq!(metrics.packet_sizes.payload_buckets, [0; 9]);
        assert_eq!(metrics.packet_sizes.wire_buckets, [0; 9]);
    }

    #[test]
    fn packet_size_counters_saturate() {
        let mut sizes = PacketSizeMetrics::default();
        sizes.payload_buckets[8] = u64::MAX;
        sizes.wire_buckets[8] = u64::MAX;
        sizes.payload_over_ipv4_safe = u64::MAX;
        sizes.wire_over_ipv4_udp = u64::MAX;

        sizes.observe(1_473, 1_473);

        assert_eq!(sizes.payload_buckets[8], u64::MAX);
        assert_eq!(sizes.wire_buckets[8], u64::MAX);
        assert_eq!(sizes.payload_over_ipv4_safe, u64::MAX);
        assert_eq!(sizes.wire_over_ipv4_udp, u64::MAX);
    }
}
