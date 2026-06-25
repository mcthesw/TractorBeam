use std::collections::HashMap;

use tracing::info;

use crate::state::RelayState;

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
        *self = Self::new(tcp_egress_queue_capacity);
    }
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
}

impl PacketOutcome {
    pub(crate) fn for_room(room: String) -> Self {
        Self {
            room: Some(room),
            room_packets_in: 1,
            ..Self::default()
        }
    }
}
