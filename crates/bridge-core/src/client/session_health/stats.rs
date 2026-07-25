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

const DELIVERY_WINDOW_BITS: u32 = 128;
const RETIRED_STREAM_CAPACITY: usize = 8;

#[derive(Debug)]
struct DeliveryWindow {
    stream_id: DeliveryStreamId,
    highest: u64,
    seen: u128,
    tracked_len: u32,
}

impl DeliveryWindow {
    fn new(stream_id: DeliveryStreamId, sequence: u64) -> Self {
        Self {
            stream_id,
            highest: sequence,
            seen: 1,
            tracked_len: 1,
        }
    }

    fn observe(&mut self, sequence: u64) -> DeliveryOutcome {
        if sequence > self.highest {
            let advance = sequence - self.highest;
            let confirmed = self.confirmed_by_advance(advance);
            if advance >= u64::from(DELIVERY_WINDOW_BITS) {
                self.seen = 1;
                self.tracked_len = DELIVERY_WINDOW_BITS;
            } else {
                let shift = u32::try_from(advance).unwrap_or(DELIVERY_WINDOW_BITS);
                self.seen = (self.seen << shift) | 1;
                self.tracked_len = self
                    .tracked_len
                    .saturating_add(shift)
                    .min(DELIVERY_WINDOW_BITS);
            }
            self.highest = sequence;
            return if confirmed != 0 {
                DeliveryOutcome::ConfirmedGap(confirmed)
            } else if advance == 1 {
                DeliveryOutcome::InOrder
            } else {
                DeliveryOutcome::OpenGap
            };
        }

        let age = self.highest - sequence;
        if age >= u64::from(DELIVERY_WINDOW_BITS) {
            return DeliveryOutcome::TooOld;
        }
        let bit = 1_u128 << u32::try_from(age).unwrap_or(DELIVERY_WINDOW_BITS - 1);
        if self.seen & bit != 0 {
            DeliveryOutcome::Duplicate
        } else {
            self.seen |= bit;
            DeliveryOutcome::Reordered
        }
    }

    fn confirmed_by_advance(&self, advance: u64) -> u64 {
        if advance >= u64::from(DELIVERY_WINDOW_BITS) {
            let existing_missing = u64::from(self.tracked_len)
                .saturating_sub(u64::from((self.seen & self.valid_mask()).count_ones()));
            return existing_missing
                .saturating_add(advance.saturating_sub(u64::from(DELIVERY_WINDOW_BITS)));
        }
        let shift = u32::try_from(advance).unwrap_or_default();
        let retained_bits = DELIVERY_WINDOW_BITS - shift;
        let retained_mask = if retained_bits == DELIVERY_WINDOW_BITS {
            u128::MAX
        } else {
            (1_u128 << retained_bits) - 1
        };
        let aged_valid = self.valid_mask() & !retained_mask;
        u64::from(
            aged_valid
                .count_ones()
                .saturating_sub((self.seen & aged_valid).count_ones()),
        )
    }

    fn valid_mask(&self) -> u128 {
        if self.tracked_len == DELIVERY_WINDOW_BITS {
            u128::MAX
        } else {
            (1_u128 << self.tracked_len) - 1
        }
    }

    fn open_gaps(&self) -> u64 {
        u64::from(self.tracked_len)
            .saturating_sub(u64::from((self.seen & self.valid_mask()).count_ones()))
    }
}

#[derive(Debug)]
struct PeerDeliveryState {
    active: DeliveryWindow,
    retired: VecDeque<DeliveryStreamId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeliveryOutcome {
    InOrder,
    OpenGap,
    ConfirmedGap(u64),
    Duplicate,
    Reordered,
    TooOld,
}

#[derive(Debug, Default)]
pub(super) struct DeliveryStats {
    first_packets: u64,
    in_order: u64,
    confirmed_gaps: u64,
    duplicates: u64,
    reordered: u64,
    by_peer: HashMap<u64, PeerDeliveryState>,
}

impl DeliveryStats {
    pub(super) fn observe(&mut self, peer: u64, stream_id: DeliveryStreamId, sequence: u64) {
        let Some(state) = self.by_peer.get_mut(&peer) else {
            self.first_packets = self.first_packets.saturating_add(1);
            self.by_peer.insert(
                peer,
                PeerDeliveryState {
                    active: DeliveryWindow::new(stream_id, sequence),
                    retired: VecDeque::new(),
                },
            );
            return;
        };
        if state.active.stream_id != stream_id {
            if state.retired.contains(&stream_id) {
                return;
            }
            state.retired.push_back(state.active.stream_id);
            if state.retired.len() > RETIRED_STREAM_CAPACITY {
                state.retired.pop_front();
            }
            state.active = DeliveryWindow::new(stream_id, sequence);
            self.first_packets = self.first_packets.saturating_add(1);
            return;
        }
        match state.active.observe(sequence) {
            DeliveryOutcome::InOrder => {
                self.in_order = self.in_order.saturating_add(1);
            }
            DeliveryOutcome::OpenGap => {}
            DeliveryOutcome::ConfirmedGap(count) => {
                self.confirmed_gaps = self.confirmed_gaps.saturating_add(count);
            }
            DeliveryOutcome::Duplicate | DeliveryOutcome::TooOld => {
                self.duplicates = self.duplicates.saturating_add(1);
            }
            DeliveryOutcome::Reordered => {
                self.reordered = self.reordered.saturating_add(1);
            }
        }
    }

    pub(super) fn snapshot(&self) -> DeliveryHealthSnapshot {
        DeliveryHealthSnapshot {
            first_packets: self.first_packets,
            in_order: self.in_order,
            open_gaps: self
                .by_peer
                .values()
                .map(|state| state.active.open_gaps())
                .fold(0_u64, u64::saturating_add),
            confirmed_gaps: self.confirmed_gaps,
            duplicates: self.duplicates,
            reordered: self.reordered,
            tracked_peers: self.by_peer.len(),
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
