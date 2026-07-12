use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash as _, Hasher as _},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use opentelemetry::{
    KeyValue,
    metrics::{Counter, Gauge, Histogram},
};

#[derive(Debug)]
pub(crate) struct RelayMetricsV2 {
    pub(crate) bootstrap_accepted: AtomicU64,
    pub(crate) bootstrap_rejected: AtomicU64,
    pub(crate) joins: AtomicU64,
    pub(crate) control_detached: AtomicU64,
    pub(crate) resumes_attempted: AtomicU64,
    pub(crate) resumes_succeeded: AtomicU64,
    pub(crate) resumes_rejected: AtomicU64,
    pub(crate) sessions_expired: AtomicU64,
    pub(crate) data_received: AtomicU64,
    pub(crate) data_forwarded: AtomicU64,
    pub(crate) data_duplicates: AtomicU64,
    pub(crate) data_rate_limited: AtomicU64,
    pub(crate) data_rejected: AtomicU64,
    control_operation: Counter<u64>,
    data_frame: Counter<u64>,
    data_io: Counter<u64>,
    control_duration: Histogram<f64>,
    dispatch_duration: Histogram<f64>,
    active_rooms: Gauge<u64>,
    active_peers: Gauge<u64>,
    tcp_queue_max_utilization: Gauge<f64>,
    trace_seed: String,
    data_trace_sample_ratio: f64,
}

impl RelayMetricsV2 {
    pub(crate) fn new(
        meter: &opentelemetry::metrics::Meter,
        trace_seed: impl Into<String>,
        data_trace_sample_ratio: f64,
    ) -> Self {
        Self {
            bootstrap_accepted: AtomicU64::new(0),
            bootstrap_rejected: AtomicU64::new(0),
            joins: AtomicU64::new(0),
            control_detached: AtomicU64::new(0),
            resumes_attempted: AtomicU64::new(0),
            resumes_succeeded: AtomicU64::new(0),
            resumes_rejected: AtomicU64::new(0),
            sessions_expired: AtomicU64::new(0),
            data_received: AtomicU64::new(0),
            data_forwarded: AtomicU64::new(0),
            data_duplicates: AtomicU64::new(0),
            data_rate_limited: AtomicU64::new(0),
            data_rejected: AtomicU64::new(0),
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
            control_duration: meter
                .f64_histogram("tractor_beam.relay.control.operation.duration")
                .with_unit("s")
                .with_description("Relay control operation duration")
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
            trace_seed: trace_seed.into(),
            data_trace_sample_ratio,
        }
    }

    pub(crate) fn control(
        &self,
        local: &AtomicU64,
        operation: &'static str,
        outcome: &'static str,
    ) {
        Self::increment_local(local);
        self.record_control(operation, outcome);
    }

    pub(crate) fn increment_local(local: &AtomicU64) {
        local.fetch_add(1, Ordering::Relaxed);
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

    pub(crate) fn data(
        &self,
        local: &AtomicU64,
        transport: &'static str,
        direction: &'static str,
        frame_type: &'static str,
        outcome: &'static str,
        bytes: usize,
    ) {
        local.fetch_add(1, Ordering::Relaxed);
        let attributes = [
            KeyValue::new("network.transport", transport),
            KeyValue::new("direction", direction),
            KeyValue::new("frame.type", frame_type),
            KeyValue::new("outcome", outcome),
        ];
        self.data_frame.add(1, &attributes);
        self.data_io.add(bytes as u64, &attributes);
    }

    pub(crate) fn value(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
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
        self.active_rooms.record(rooms as u64, &[]);
        for (count, transport, presence) in [
            (peer_counts[0], "tcp", "connected"),
            (peer_counts[1], "tcp", "reconnecting"),
            (peer_counts[2], "udp", "connected"),
            (peer_counts[3], "udp", "reconnecting"),
        ] {
            self.active_peers.record(
                count as u64,
                &[
                    KeyValue::new("network.transport", transport),
                    KeyValue::new("peer.presence", presence),
                ],
            );
        }
        self.tcp_queue_max_utilization
            .record(queue_utilization.clamp(0.0, 1.0), &[]);
    }

    pub(crate) fn should_trace_data(&self, room_metric_id: u64, correlation_id: u64) -> bool {
        if self.data_trace_sample_ratio <= 0.0 {
            return false;
        }
        let mut hasher = DefaultHasher::new();
        self.trace_seed.hash(&mut hasher);
        room_metric_id.hash(&mut hasher);
        correlation_id.hash(&mut hasher);
        let sample = hasher.finish() as f64 / u64::MAX as f64;
        sample < self.data_trace_sample_ratio
    }
}

pub(crate) struct ControlDurationGuard<'a> {
    metrics: &'a RelayMetricsV2,
    operation: &'static str,
    started: Instant,
}

impl Drop for ControlDurationGuard<'_> {
    fn drop(&mut self) {
        self.metrics
            .record_control_duration(self.operation, self.started.elapsed().as_secs_f64());
    }
}

#[cfg(test)]
mod tests {
    use super::RelayMetricsV2;

    #[test]
    fn data_sampling_is_deterministic_and_bounded() {
        let meter = opentelemetry::global::meter("relay-metrics-test");
        let disabled = RelayMetricsV2::new(&meter, "relay-a", 0.0);
        assert!(!disabled.should_trace_data(1, 1));

        let enabled = RelayMetricsV2::new(&meter, "relay-a", 1.0);
        assert!(enabled.should_trace_data(1, 1));
        assert_eq!(
            enabled.should_trace_data(7, 42),
            enabled.should_trace_data(7, 42)
        );
    }
}
