use std::{
    collections::HashMap,
    fmt::{self, Display},
    time::{Duration, Instant},
};

use serde::Serialize;

const LATENCY_SAMPLE_CAPACITY: usize = 2_048;
const ACTIVE_TRAFFIC_STARTUP_GRACE_SECONDS: u64 = 15;
const WATCH_SEQUENCE_GAPS: u64 = 1;
const POOR_SEQUENCE_GAPS: u64 = 10;
const WATCH_DUPLICATE_OR_REORDERED: u64 = 10;

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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityConfidence {
    #[default]
    None,
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionQualityReason {
    LocalQueueDrop,
    SequenceGap,
    SequenceReordered,
    HookSendStall,
    RuntimeRttTimeout,
    StartupOrIdle,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SessionHealthWindow {
    pub duration_seconds: u64,
    pub hook_in_packets: u64,
    pub relay_recv_packets: u64,
    pub queue_drops: u64,
    pub sequence_gaps: u64,
    pub sequence_reordered: u64,
    pub runtime_rtt_sent: u64,
    pub runtime_rtt_timeouts: u64,
    pub hook_send_over_500_ms: u64,
    pub hook_send_over_1000_ms: u64,
    pub relay_gap_over_500_ms: u64,
    pub relay_gap_over_1000_ms: u64,
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
    pub confidence: QualityConfidence,
    pub reasons: Vec<SessionQualityReason>,
    pub window: SessionHealthWindow,
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
            "{label}: quality={} confidence={:?} reasons={:?} window={}s hook_in={} relay_recv={} rtt_p95={} queue_drops={} seq_gaps={} relay_gap_p95={} hook_out_p95={}",
            self.quality,
            self.confidence,
            self.reasons,
            self.window.duration_seconds,
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
    quality_baseline: QualityBaseline,
    last_snapshot: Instant,
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
            quality_baseline: QualityBaseline::default(),
            last_snapshot: now,
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
        let elapsed_seconds = now.duration_since(self.start).as_secs();
        let current = QualityBaseline::from_snapshots(
            hook_in_recv,
            relay_recv,
            queues,
            source_sequence,
            runtime_rtt,
            hook_out_send_duration,
        );
        let window = current.delta(
            self.quality_baseline,
            now.duration_since(self.last_snapshot),
        );
        self.quality_baseline = current;
        self.last_snapshot = now;
        let assessment = classify_quality(elapsed_seconds, window);

        SessionHealthSnapshot {
            elapsed_seconds,
            quality: assessment.quality,
            confidence: assessment.confidence,
            reasons: assessment.reasons,
            window,
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

mod stats;

use stats::*;

#[derive(Clone, Copy, Debug, Default)]
struct QualityBaseline {
    hook_in_packets: u64,
    relay_recv_packets: u64,
    queue_drops: u64,
    sequence_gaps: u64,
    sequence_reordered: u64,
    runtime_rtt_sent: u64,
    runtime_rtt_timeouts: u64,
    hook_send_over_500_ms: u64,
    hook_send_over_1000_ms: u64,
    relay_gap_over_500_ms: u64,
    relay_gap_over_1000_ms: u64,
}

impl QualityBaseline {
    fn from_snapshots(
        hook_in: PacketStageSnapshot,
        relay_recv: PacketStageSnapshot,
        queues: QueueHealthSnapshot,
        sequence: SequenceHealthSnapshot,
        runtime_rtt: RuntimeRttSnapshot,
        hook_send: LatencySummary,
    ) -> Self {
        Self {
            hook_in_packets: hook_in.packets,
            relay_recv_packets: relay_recv.packets,
            queue_drops: queues.total_dropped(),
            sequence_gaps: sequence.gaps,
            sequence_reordered: sequence.duplicate_or_reordered,
            runtime_rtt_sent: runtime_rtt.sent,
            runtime_rtt_timeouts: runtime_rtt.timed_out,
            hook_send_over_500_ms: hook_send.over_500_ms,
            hook_send_over_1000_ms: hook_send.over_1000_ms,
            relay_gap_over_500_ms: relay_recv.gap.over_500_ms,
            relay_gap_over_1000_ms: relay_recv.gap.over_1000_ms,
        }
    }

    fn delta(self, previous: Self, duration: Duration) -> SessionHealthWindow {
        SessionHealthWindow {
            duration_seconds: duration.as_secs(),
            hook_in_packets: self
                .hook_in_packets
                .saturating_sub(previous.hook_in_packets),
            relay_recv_packets: self
                .relay_recv_packets
                .saturating_sub(previous.relay_recv_packets),
            queue_drops: self.queue_drops.saturating_sub(previous.queue_drops),
            sequence_gaps: self.sequence_gaps.saturating_sub(previous.sequence_gaps),
            sequence_reordered: self
                .sequence_reordered
                .saturating_sub(previous.sequence_reordered),
            runtime_rtt_sent: self
                .runtime_rtt_sent
                .saturating_sub(previous.runtime_rtt_sent),
            runtime_rtt_timeouts: self
                .runtime_rtt_timeouts
                .saturating_sub(previous.runtime_rtt_timeouts),
            hook_send_over_500_ms: self
                .hook_send_over_500_ms
                .saturating_sub(previous.hook_send_over_500_ms),
            hook_send_over_1000_ms: self
                .hook_send_over_1000_ms
                .saturating_sub(previous.hook_send_over_1000_ms),
            relay_gap_over_500_ms: self
                .relay_gap_over_500_ms
                .saturating_sub(previous.relay_gap_over_500_ms),
            relay_gap_over_1000_ms: self
                .relay_gap_over_1000_ms
                .saturating_sub(previous.relay_gap_over_1000_ms),
        }
    }
}

struct QualityAssessment {
    quality: SessionQuality,
    confidence: QualityConfidence,
    reasons: Vec<SessionQualityReason>,
}

fn classify_quality(elapsed_seconds: u64, window: SessionHealthWindow) -> QualityAssessment {
    let has_evidence =
        window.hook_in_packets > 0 || window.relay_recv_packets > 0 || window.runtime_rtt_sent > 0;
    if elapsed_seconds < ACTIVE_TRAFFIC_STARTUP_GRACE_SECONDS || !has_evidence {
        return QualityAssessment {
            quality: SessionQuality::Unavailable,
            confidence: QualityConfidence::None,
            reasons: vec![SessionQualityReason::StartupOrIdle],
        };
    }

    let mut reasons = Vec::new();
    let mut poor = false;
    if window.queue_drops > 0 {
        reasons.push(SessionQualityReason::LocalQueueDrop);
        poor = true;
    }
    if window.sequence_gaps >= WATCH_SEQUENCE_GAPS {
        reasons.push(SessionQualityReason::SequenceGap);
        poor |= window.sequence_gaps >= POOR_SEQUENCE_GAPS;
    }
    if window.sequence_reordered >= WATCH_DUPLICATE_OR_REORDERED {
        reasons.push(SessionQualityReason::SequenceReordered);
    }
    if window.hook_send_over_500_ms > 0 {
        reasons.push(SessionQualityReason::HookSendStall);
        poor |= window.hook_send_over_1000_ms > 0;
    }
    if window.runtime_rtt_timeouts > 0 {
        reasons.push(SessionQualityReason::RuntimeRttTimeout);
        poor |= window.runtime_rtt_timeouts >= 3;
    }
    reasons.sort_unstable();
    let evidence_count = window
        .hook_in_packets
        .saturating_add(window.relay_recv_packets)
        .saturating_add(window.runtime_rtt_sent);
    QualityAssessment {
        quality: if poor {
            SessionQuality::Poor
        } else if reasons.is_empty() {
            SessionQuality::Good
        } else {
            SessionQuality::Watch
        },
        confidence: if evidence_count >= 40 {
            QualityConfidence::High
        } else if evidence_count >= 10 {
            QualityConfidence::Medium
        } else {
            QualityConfidence::Low
        },
        reasons,
    }
}

fn display_ms(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| format!("{value}ms"))
}

#[cfg(test)]
#[path = "session_health_tests.rs"]
mod tests;
