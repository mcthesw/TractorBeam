use std::{sync::Arc, time::Instant};

use opentelemetry::{
    KeyValue,
    metrics::{Counter, Gauge, Histogram, UpDownCounter},
};

#[derive(Debug)]
pub(crate) struct RelayMetrics {
    connection_operation: Counter<u64>,
    active_connections: UpDownCounter<i64>,
    control_operation: Counter<u64>,
    data_frame: Counter<u64>,
    data_io: Counter<u64>,
    tcp_egress_queue_full: Counter<u64>,
    control_duration: Histogram<f64>,
    establishment_duration: Histogram<f64>,
    dispatch_duration: Histogram<f64>,
    active_rooms: Gauge<u64>,
    active_peers: Gauge<u64>,
    tcp_queue_max_utilization: Gauge<f64>,
}

impl RelayMetrics {
    pub(crate) fn new(meter: &opentelemetry::metrics::Meter) -> Self {
        Self {
            connection_operation: meter
                .u64_counter("tractor_beam.relay.connection.operation")
                .with_unit("{connection}")
                .with_description("Relay TCP connection outcomes")
                .build(),
            active_connections: meter
                .i64_up_down_counter("tractor_beam.relay.connection.active")
                .with_unit("{connection}")
                .with_description("Active accepted Relay TCP connections")
                .build(),
            control_operation: meter
                .u64_counter("tractor_beam.relay.control.operation")
                .with_unit("{operation}")
                .with_description("Relay control-plane operation outcomes")
                .build(),
            data_frame: meter
                .u64_counter("tractor_beam.relay.data.frame")
                .with_unit("{frame}")
                .with_description("Relay data-plane frame outcomes")
                .build(),
            data_io: meter
                .u64_counter("tractor_beam.relay.data.io")
                .with_unit("By")
                .with_description("Relay data-plane bytes accepted or forwarded")
                .build(),
            tcp_egress_queue_full: meter
                .u64_counter("tractor_beam.relay.tcp.egress.queue.full")
                .with_unit("{frame}")
                .with_description("Frames rejected by a full TCP egress queue")
                .build(),
            control_duration: meter
                .f64_histogram("tractor_beam.relay.control.operation.duration")
                .with_unit("s")
                .with_description("Relay control operation duration")
                .build(),
            establishment_duration: meter
                .f64_histogram("tractor_beam.relay.session.establishment.duration")
                .with_unit("s")
                .with_description("Relay session establishment attempt duration")
                .build(),
            dispatch_duration: meter
                .f64_histogram("tractor_beam.relay.data.dispatch.duration")
                .with_unit("s")
                .with_description("Relay frame routing and dispatch duration")
                .build(),
            active_rooms: meter
                .u64_gauge("tractor_beam.relay.room.active")
                .with_unit("{room}")
                .with_description("Active Relay rooms")
                .build(),
            active_peers: meter
                .u64_gauge("tractor_beam.relay.peer.active")
                .with_unit("{peer}")
                .with_description("Active Relay peers")
                .build(),
            tcp_queue_max_utilization: meter
                .f64_gauge("tractor_beam.relay.tcp.egress.queue.max_utilization")
                .with_unit("1")
                .with_description("Maximum current TCP egress queue utilization")
                .build(),
        }
    }

    pub(crate) fn start_connection(self: &Arc<Self>) -> ConnectionGuard {
        self.connection_operation
            .add(1, &[KeyValue::new("outcome", "accepted")]);
        self.active_connections.add(1, &[]);
        ConnectionGuard {
            metrics: Arc::clone(self),
        }
    }

    pub(crate) fn record_blocked_connection(&self) {
        self.connection_operation
            .add(1, &[KeyValue::new("outcome", "blocked")]);
    }

    pub(crate) fn record_control(&self, operation: &'static str, outcome: &'static str) {
        self.control_operation.add(
            1,
            &[
                KeyValue::new("operation", operation),
                KeyValue::new("outcome", outcome),
            ],
        );
    }

    pub(crate) fn record_data(
        &self,
        transport: &'static str,
        direction: &'static str,
        frame_type: &'static str,
        outcome: &'static str,
        bytes: usize,
    ) {
        let attributes = [
            KeyValue::new("network.transport", transport),
            KeyValue::new("direction", direction),
            KeyValue::new("frame.type", frame_type),
            KeyValue::new("outcome", outcome),
        ];
        self.data_frame.add(1, &attributes);
        self.data_io
            .add(u64::try_from(bytes).unwrap_or(u64::MAX), &attributes);
    }

    pub(crate) fn record_tcp_egress_queue_full(&self, frame_type: &'static str) {
        self.tcp_egress_queue_full
            .add(1, &[KeyValue::new("frame.type", frame_type)]);
    }

    pub(crate) fn start_control_duration(
        &self,
        operation: &'static str,
    ) -> ControlDurationGuard<'_> {
        ControlDurationGuard {
            metrics: self,
            operation,
            started: Instant::now(),
        }
    }

    fn record_control_duration(&self, operation: &'static str, seconds: f64) {
        self.control_duration
            .record(seconds, &[KeyValue::new("operation", operation)]);
    }

    pub(crate) fn record_establishment_duration(
        &self,
        operation: &'static str,
        profile: &'static str,
        outcome: &'static str,
        seconds: f64,
    ) {
        self.establishment_duration.record(
            seconds,
            &[
                KeyValue::new("operation", operation),
                KeyValue::new("network.transport", profile),
                KeyValue::new("outcome", outcome),
            ],
        );
    }

    pub(crate) fn record_dispatch_duration(
        &self,
        transport: &'static str,
        frame_type: &'static str,
        seconds: f64,
    ) {
        self.dispatch_duration.record(
            seconds,
            &[
                KeyValue::new("network.transport", transport),
                KeyValue::new("frame.type", frame_type),
            ],
        );
    }

    pub(crate) fn record_snapshot(
        &self,
        rooms: usize,
        peer_counts: [usize; 4],
        queue_utilization: f64,
    ) {
        self.active_rooms
            .record(u64::try_from(rooms).unwrap_or(u64::MAX), &[]);
        for (count, transport, presence) in [
            (peer_counts[0], "tcp", "connected"),
            (peer_counts[1], "tcp", "reconnecting"),
            (peer_counts[2], "udp", "connected"),
            (peer_counts[3], "udp", "reconnecting"),
        ] {
            self.active_peers.record(
                u64::try_from(count).unwrap_or(u64::MAX),
                &[
                    KeyValue::new("network.transport", transport),
                    KeyValue::new("peer.presence", presence),
                ],
            );
        }
        self.tcp_queue_max_utilization
            .record(queue_utilization.clamp(0.0, 1.0), &[]);
    }
}

pub(crate) struct ConnectionGuard {
    metrics: Arc<RelayMetrics>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.metrics.active_connections.add(-1, &[]);
        self.metrics
            .connection_operation
            .add(1, &[KeyValue::new("outcome", "closed")]);
    }
}

pub(crate) struct ControlDurationGuard<'a> {
    metrics: &'a RelayMetrics,
    operation: &'static str,
    started: Instant,
}

impl Drop for ControlDurationGuard<'_> {
    fn drop(&mut self) {
        self.metrics
            .record_control_duration(self.operation, self.started.elapsed().as_secs_f64());
    }
}
