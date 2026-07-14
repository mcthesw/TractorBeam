use super::*;

#[derive(Debug, Default)]
pub(super) struct PacketStageStats {
    packets: u64,
    bytes: u64,
    last_seen: Option<Instant>,
    gaps: LatencyAccumulator,
}

impl PacketStageStats {
    pub(super) fn observe(&mut self, bytes: usize, now: Instant) {
        self.packets = self.packets.saturating_add(1);
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        if let Some(previous) = self.last_seen.replace(now) {
            self.gaps.observe(now.duration_since(previous));
        }
    }

    pub(super) fn snapshot(&self) -> PacketStageSnapshot {
        PacketStageSnapshot {
            packets: self.packets,
            bytes: self.bytes,
            gap: self.gaps.summary(),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct QueueStats {
    outbound_enqueued: u64,
    outbound_full: u64,
    outbound_dropped: u64,
    inbound_enqueued: u64,
    inbound_full: u64,
    inbound_dropped: u64,
}

impl QueueStats {
    pub(super) fn observe_outbound(&mut self, accepted: bool) {
        if accepted {
            self.outbound_enqueued = self.outbound_enqueued.saturating_add(1);
        } else {
            self.outbound_full = self.outbound_full.saturating_add(1);
            self.outbound_dropped = self.outbound_dropped.saturating_add(1);
        }
    }

    pub(super) fn observe_inbound(&mut self, accepted: bool) {
        if accepted {
            self.inbound_enqueued = self.inbound_enqueued.saturating_add(1);
        } else {
            self.inbound_full = self.inbound_full.saturating_add(1);
            self.inbound_dropped = self.inbound_dropped.saturating_add(1);
        }
    }

    pub(super) fn snapshot(&self) -> QueueHealthSnapshot {
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
pub(super) struct SequenceStats {
    first_packets: u64,
    in_order: u64,
    gaps: u64,
    duplicate_or_reordered: u64,
    last_by_peer: HashMap<u64, u32>,
}

impl SequenceStats {
    pub(super) fn observe(&mut self, peer: u64, source_sequence: u32) {
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

    pub(super) fn snapshot(&self) -> SequenceHealthSnapshot {
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
pub(super) struct RuntimeRttStats {
    next_id: u64,
    sent: u64,
    received: u64,
    timed_out: u64,
    pending: HashMap<u64, Instant>,
    latency: LatencyAccumulator,
}

impl RuntimeRttStats {
    pub(super) fn next_ping(&mut self, now: Instant) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.sent = self.sent.saturating_add(1);
        self.pending.insert(id, now);
        id
    }

    pub(super) fn observe_pong(&mut self, id: u64, now: Instant) {
        if let Some(sent_at) = self.pending.remove(&id) {
            self.received = self.received.saturating_add(1);
            self.latency.observe(now.duration_since(sent_at));
        }
    }

    pub(super) fn expire(&mut self, now: Instant, timeout: Duration) {
        let before = self.pending.len();
        self.pending
            .retain(|_, sent_at| now.duration_since(*sent_at) <= timeout);
        let expired = before.saturating_sub(self.pending.len());
        self.timed_out = self
            .timed_out
            .saturating_add(u64::try_from(expired).unwrap_or(u64::MAX));
    }

    pub(super) fn snapshot(&self, enabled: bool) -> RuntimeRttSnapshot {
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
pub(super) struct LatencyAccumulator {
    count: u64,
    min_ms: Option<u64>,
    max_ms: Option<u64>,
    over_200_ms: u64,
    over_500_ms: u64,
    over_1000_ms: u64,
    samples: Vec<u64>,
}

impl LatencyAccumulator {
    pub(super) fn observe(&mut self, duration: Duration) {
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

    pub(super) fn summary(&self) -> LatencySummary {
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

pub(super) fn percentile(sorted_samples: &[u64], percentile: usize) -> Option<u64> {
    if sorted_samples.is_empty() {
        return None;
    }
    let numerator = sorted_samples.len().saturating_sub(1) * percentile;
    let index = (numerator + 50) / 100;
    sorted_samples.get(index).copied()
}
