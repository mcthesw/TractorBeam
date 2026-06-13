use std::{
    collections::HashMap,
    fmt::{self, Display},
    time::{Duration, Instant},
};

use serde::Serialize;

const LATENCY_SAMPLE_CAPACITY: usize = 2_048;
const WATCH_RTT_P95_MS: u64 = 120;
const POOR_RTT_P95_MS: u64 = 250;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LatencySummary {
    pub count: u64,
    pub min_ms: Option<u64>,
    pub median_ms: Option<u64>,
    pub p95_ms: Option<u64>,
    pub max_ms: Option<u64>,
    pub over_200_ms: u64,
    pub over_500_ms: u64,
    pub over_1000_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PacketStageSnapshot {
    pub packets: u64,
    pub bytes: u64,
    pub gap: LatencySummary,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct QueueHealthSnapshot {
    pub outbound_enqueued: u64,
    pub outbound_full: u64,
    pub outbound_dropped: u64,
    pub inbound_enqueued: u64,
    pub inbound_full: u64,
    pub inbound_dropped: u64,
}

impl QueueHealthSnapshot {
    #[must_use]
    pub fn total_dropped(self) -> u64 {
        self.outbound_dropped.saturating_add(self.inbound_dropped)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SequenceHealthSnapshot {
    pub first_packets: u64,
    pub in_order: u64,
    pub gaps: u64,
    pub duplicate_or_reordered: u64,
    pub tracked_peers: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeRttSnapshot {
    pub enabled: bool,
    pub sent: u64,
    pub received: u64,
    pub timed_out: u64,
    pub pending: usize,
    pub latency: LatencySummary,
}

impl Default for RuntimeRttSnapshot {
    fn default() -> Self {
        Self {
            enabled: true,
            sent: 0,
            received: 0,
            timed_out: 0,
            pending: 0,
            latency: LatencySummary::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionQuality {
    #[default]
    Unavailable,
    Good,
    Watch,
    Poor,
}

impl Display for SessionQuality {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("unavailable"),
            Self::Good => formatter.write_str("good"),
            Self::Watch => formatter.write_str("watch"),
            Self::Poor => formatter.write_str("poor"),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SessionHealthSnapshot {
    pub elapsed_seconds: u64,
    pub quality: SessionQuality,
    pub hook_in_recv: PacketStageSnapshot,
    pub relay_recv: PacketStageSnapshot,
    pub relay_send_duration: LatencySummary,
    pub hook_out_send_duration: LatencySummary,
    pub queues: QueueHealthSnapshot,
    pub source_sequence: SequenceHealthSnapshot,
    pub runtime_rtt: RuntimeRttSnapshot,
}

pub type SessionHealthSummary = SessionHealthSnapshot;

impl SessionHealthSnapshot {
    #[must_use]
    pub fn compact_log_line(&self, label: &str) -> String {
        format!(
            "{label}: quality={} hook_in={} relay_recv={} rtt_p95={} queue_drops={} seq_gaps={} relay_gap_p95={} hook_out_p95={}",
            self.quality,
            self.hook_in_recv.packets,
            self.relay_recv.packets,
            display_ms(self.runtime_rtt.latency.p95_ms),
            self.queues.total_dropped(),
            self.source_sequence.gaps,
            display_ms(self.relay_recv.gap.p95_ms),
            display_ms(self.hook_out_send_duration.p95_ms),
        )
    }
}

#[derive(Debug)]
pub(super) struct SessionHealth {
    start: Instant,
    runtime_rtt_enabled: bool,
    runtime_rtt_timeout: Duration,
    hook_in_recv: PacketStageStats,
    relay_recv: PacketStageStats,
    relay_send_duration: LatencyAccumulator,
    hook_out_send_duration: LatencyAccumulator,
    queues: QueueStats,
    sequences: SequenceStats,
    runtime_rtt: RuntimeRttStats,
}

impl SessionHealth {
    #[must_use]
    pub(super) fn new(
        runtime_rtt_enabled: bool,
        runtime_rtt_timeout: Duration,
        now: Instant,
    ) -> Self {
        Self {
            start: now,
            runtime_rtt_enabled,
            runtime_rtt_timeout,
            hook_in_recv: PacketStageStats::default(),
            relay_recv: PacketStageStats::default(),
            relay_send_duration: LatencyAccumulator::default(),
            hook_out_send_duration: LatencyAccumulator::default(),
            queues: QueueStats::default(),
            sequences: SequenceStats::default(),
            runtime_rtt: RuntimeRttStats::default(),
        }
    }

    pub(super) fn observe_hook_in_recv(&mut self, bytes: usize, now: Instant) {
        self.hook_in_recv.observe(bytes, now);
    }

    pub(super) fn observe_outbound_enqueue(&mut self, accepted: bool) {
        self.queues.observe_outbound(accepted);
    }

    pub(super) fn observe_relay_send_duration(&mut self, duration: Duration) {
        self.relay_send_duration.observe(duration);
    }

    pub(super) fn observe_relay_recv(&mut self, bytes: usize, now: Instant) {
        self.relay_recv.observe(bytes, now);
    }

    pub(super) fn observe_inbound_enqueue(&mut self, accepted: bool) {
        self.queues.observe_inbound(accepted);
    }

    pub(super) fn observe_hook_out_send_duration(&mut self, duration: Duration) {
        self.hook_out_send_duration.observe(duration);
    }

    pub(super) fn observe_source_sequence(&mut self, peer: u64, source_sequence: u32) {
        self.sequences.observe(peer, source_sequence);
    }

    pub(super) fn next_health_ping(&mut self, now: Instant) -> Option<u64> {
        if !self.runtime_rtt_enabled {
            return None;
        }
        self.expire_runtime_rtt(now);
        Some(self.runtime_rtt.next_ping(now))
    }

    pub(super) fn observe_health_pong(&mut self, id: u64, now: Instant) {
        if self.runtime_rtt_enabled {
            self.runtime_rtt.observe_pong(id, now);
        }
    }

    pub(super) fn snapshot(&mut self, now: Instant) -> SessionHealthSnapshot {
        self.expire_runtime_rtt(now);
        let hook_in_recv = self.hook_in_recv.snapshot();
        let relay_recv = self.relay_recv.snapshot();
        let queues = self.queues.snapshot();
        let source_sequence = self.sequences.snapshot();
        let runtime_rtt = self.runtime_rtt.snapshot(self.runtime_rtt_enabled);
        let relay_send_duration = self.relay_send_duration.summary();
        let hook_out_send_duration = self.hook_out_send_duration.summary();
        let quality = classify_quality(
            hook_in_recv,
            relay_recv,
            queues,
            source_sequence,
            runtime_rtt,
            hook_out_send_duration,
        );

        SessionHealthSnapshot {
            elapsed_seconds: now.duration_since(self.start).as_secs(),
            quality,
            hook_in_recv,
            relay_recv,
            relay_send_duration,
            hook_out_send_duration,
            queues,
            source_sequence,
            runtime_rtt,
        }
    }

    fn expire_runtime_rtt(&mut self, now: Instant) {
        if self.runtime_rtt_enabled {
            self.runtime_rtt.expire(now, self.runtime_rtt_timeout);
        }
    }
}

#[derive(Debug, Default)]
struct PacketStageStats {
    packets: u64,
    bytes: u64,
    last_seen: Option<Instant>,
    gaps: LatencyAccumulator,
}

impl PacketStageStats {
    fn observe(&mut self, bytes: usize, now: Instant) {
        self.packets = self.packets.saturating_add(1);
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        if let Some(previous) = self.last_seen.replace(now) {
            self.gaps.observe(now.duration_since(previous));
        }
    }

    fn snapshot(&self) -> PacketStageSnapshot {
        PacketStageSnapshot {
            packets: self.packets,
            bytes: self.bytes,
            gap: self.gaps.summary(),
        }
    }
}

#[derive(Debug, Default)]
struct QueueStats {
    outbound_enqueued: u64,
    outbound_full: u64,
    outbound_dropped: u64,
    inbound_enqueued: u64,
    inbound_full: u64,
    inbound_dropped: u64,
}

impl QueueStats {
    fn observe_outbound(&mut self, accepted: bool) {
        if accepted {
            self.outbound_enqueued = self.outbound_enqueued.saturating_add(1);
        } else {
            self.outbound_full = self.outbound_full.saturating_add(1);
            self.outbound_dropped = self.outbound_dropped.saturating_add(1);
        }
    }

    fn observe_inbound(&mut self, accepted: bool) {
        if accepted {
            self.inbound_enqueued = self.inbound_enqueued.saturating_add(1);
        } else {
            self.inbound_full = self.inbound_full.saturating_add(1);
            self.inbound_dropped = self.inbound_dropped.saturating_add(1);
        }
    }

    fn snapshot(&self) -> QueueHealthSnapshot {
        QueueHealthSnapshot {
            outbound_enqueued: self.outbound_enqueued,
            outbound_full: self.outbound_full,
            outbound_dropped: self.outbound_dropped,
            inbound_enqueued: self.inbound_enqueued,
            inbound_full: self.inbound_full,
            inbound_dropped: self.inbound_dropped,
        }
    }
}

#[derive(Debug, Default)]
struct SequenceStats {
    first_packets: u64,
    in_order: u64,
    gaps: u64,
    duplicate_or_reordered: u64,
    last_by_peer: HashMap<u64, u32>,
}

impl SequenceStats {
    fn observe(&mut self, peer: u64, source_sequence: u32) {
        if source_sequence == 0 {
            return;
        }
        let Some(previous) = self.last_by_peer.get_mut(&peer) else {
            self.first_packets = self.first_packets.saturating_add(1);
            self.last_by_peer.insert(peer, source_sequence);
            return;
        };
        let expected = previous.saturating_add(1);
        if source_sequence == expected {
            self.in_order = self.in_order.saturating_add(1);
            *previous = source_sequence;
        } else if source_sequence > expected {
            self.gaps = self.gaps.saturating_add(1);
            *previous = source_sequence;
        } else {
            self.duplicate_or_reordered = self.duplicate_or_reordered.saturating_add(1);
        }
    }

    fn snapshot(&self) -> SequenceHealthSnapshot {
        SequenceHealthSnapshot {
            first_packets: self.first_packets,
            in_order: self.in_order,
            gaps: self.gaps,
            duplicate_or_reordered: self.duplicate_or_reordered,
            tracked_peers: self.last_by_peer.len(),
        }
    }
}

#[derive(Debug, Default)]
struct RuntimeRttStats {
    next_id: u64,
    sent: u64,
    received: u64,
    timed_out: u64,
    pending: HashMap<u64, Instant>,
    latency: LatencyAccumulator,
}

impl RuntimeRttStats {
    fn next_ping(&mut self, now: Instant) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.sent = self.sent.saturating_add(1);
        self.pending.insert(id, now);
        id
    }

    fn observe_pong(&mut self, id: u64, now: Instant) {
        if let Some(sent_at) = self.pending.remove(&id) {
            self.received = self.received.saturating_add(1);
            self.latency.observe(now.duration_since(sent_at));
        }
    }

    fn expire(&mut self, now: Instant, timeout: Duration) {
        let before = self.pending.len();
        self.pending
            .retain(|_, sent_at| now.duration_since(*sent_at) <= timeout);
        let expired = before.saturating_sub(self.pending.len());
        self.timed_out = self
            .timed_out
            .saturating_add(u64::try_from(expired).unwrap_or(u64::MAX));
    }

    fn snapshot(&self, enabled: bool) -> RuntimeRttSnapshot {
        RuntimeRttSnapshot {
            enabled,
            sent: self.sent,
            received: self.received,
            timed_out: self.timed_out,
            pending: self.pending.len(),
            latency: self.latency.summary(),
        }
    }
}

#[derive(Debug, Default)]
struct LatencyAccumulator {
    count: u64,
    min_ms: Option<u64>,
    max_ms: Option<u64>,
    over_200_ms: u64,
    over_500_ms: u64,
    over_1000_ms: u64,
    samples: Vec<u64>,
}

impl LatencyAccumulator {
    fn observe(&mut self, duration: Duration) {
        let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.count = self.count.saturating_add(1);
        self.min_ms = Some(self.min_ms.map_or(millis, |current| current.min(millis)));
        self.max_ms = Some(self.max_ms.map_or(millis, |current| current.max(millis)));
        if millis > 200 {
            self.over_200_ms = self.over_200_ms.saturating_add(1);
        }
        if millis > 500 {
            self.over_500_ms = self.over_500_ms.saturating_add(1);
        }
        if millis > 1_000 {
            self.over_1000_ms = self.over_1000_ms.saturating_add(1);
        }
        if self.samples.len() < LATENCY_SAMPLE_CAPACITY {
            self.samples.push(millis);
        } else {
            let index = usize::try_from(self.count).unwrap_or(0) % LATENCY_SAMPLE_CAPACITY;
            self.samples[index] = millis;
        }
    }

    fn summary(&self) -> LatencySummary {
        let mut samples = self.samples.clone();
        samples.sort_unstable();
        LatencySummary {
            count: self.count,
            min_ms: self.min_ms,
            median_ms: percentile(&samples, 50),
            p95_ms: percentile(&samples, 95),
            max_ms: self.max_ms,
            over_200_ms: self.over_200_ms,
            over_500_ms: self.over_500_ms,
            over_1000_ms: self.over_1000_ms,
        }
    }
}

fn percentile(sorted_samples: &[u64], percentile: usize) -> Option<u64> {
    if sorted_samples.is_empty() {
        return None;
    }
    let numerator = sorted_samples.len().saturating_sub(1) * percentile;
    let index = (numerator + 50) / 100;
    sorted_samples.get(index).copied()
}

fn classify_quality(
    hook_in_recv: PacketStageSnapshot,
    relay_recv: PacketStageSnapshot,
    queues: QueueHealthSnapshot,
    source_sequence: SequenceHealthSnapshot,
    runtime_rtt: RuntimeRttSnapshot,
    hook_out_send_duration: LatencySummary,
) -> SessionQuality {
    let has_evidence = hook_in_recv.packets > 0 || relay_recv.packets > 0 || runtime_rtt.sent > 0;
    if !has_evidence {
        return SessionQuality::Unavailable;
    }
    if queues.total_dropped() > 0
        || source_sequence.gaps > 0
        || relay_recv.gap.over_1000_ms > 0
        || hook_out_send_duration.over_1000_ms > 0
        || runtime_rtt.timed_out > 0
        || runtime_rtt
            .latency
            .p95_ms
            .is_some_and(|p95| p95 >= POOR_RTT_P95_MS)
    {
        return SessionQuality::Poor;
    }
    if source_sequence.duplicate_or_reordered > 0
        || relay_recv.gap.over_500_ms > 0
        || hook_out_send_duration.over_500_ms > 0
        || runtime_rtt
            .latency
            .p95_ms
            .is_some_and(|p95| p95 >= WATCH_RTT_P95_MS)
    {
        return SessionQuality::Watch;
    }
    SessionQuality::Good
}

fn display_ms(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| format!("{value}ms"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_summary_reports_percentiles_and_thresholds() {
        let mut accumulator = LatencyAccumulator::default();

        for value in [10, 50, 210, 510, 1_100] {
            accumulator.observe(Duration::from_millis(value));
        }

        let summary = accumulator.summary();

        assert_eq!(summary.count, 5);
        assert_eq!(summary.min_ms, Some(10));
        assert_eq!(summary.median_ms, Some(210));
        assert_eq!(summary.p95_ms, Some(1_100));
        assert_eq!(summary.max_ms, Some(1_100));
        assert_eq!(summary.over_200_ms, 3);
        assert_eq!(summary.over_500_ms, 2);
        assert_eq!(summary.over_1000_ms, 1);
    }

    #[test]
    fn sequence_stats_classify_gaps_and_reordered_packets() {
        let mut stats = SequenceStats::default();

        stats.observe(7, 1);
        stats.observe(7, 2);
        stats.observe(7, 5);
        stats.observe(7, 4);

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.first_packets, 1);
        assert_eq!(snapshot.in_order, 1);
        assert_eq!(snapshot.gaps, 1);
        assert_eq!(snapshot.duplicate_or_reordered, 1);
    }

    #[test]
    fn queue_stats_count_enqueued_and_dropped_packets() {
        let mut stats = QueueStats::default();

        stats.observe_outbound(true);
        stats.observe_outbound(false);
        stats.observe_inbound(true);
        stats.observe_inbound(false);

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.outbound_enqueued, 1);
        assert_eq!(snapshot.outbound_full, 1);
        assert_eq!(snapshot.inbound_enqueued, 1);
        assert_eq!(snapshot.inbound_full, 1);
        assert_eq!(snapshot.total_dropped(), 2);
    }

    #[test]
    fn runtime_rtt_times_out_without_marking_session_failed() {
        let start = Instant::now();
        let mut health = SessionHealth::new(true, Duration::from_millis(10), start);

        let id = health.next_health_ping(start).unwrap();
        health.snapshot(start + Duration::from_millis(20));
        health.observe_health_pong(id, start + Duration::from_millis(30));
        let snapshot = health.snapshot(start + Duration::from_millis(30));

        assert_eq!(snapshot.runtime_rtt.sent, 1);
        assert_eq!(snapshot.runtime_rtt.received, 0);
        assert_eq!(snapshot.runtime_rtt.timed_out, 1);
        assert_eq!(snapshot.quality, SessionQuality::Poor);
    }
}
