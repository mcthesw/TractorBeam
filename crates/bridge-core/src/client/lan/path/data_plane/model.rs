use std::{sync::Arc, time::Duration};

use super::{DirectionTotals, LanDataPlaneLedger};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(in crate::client) enum LanDataDirection {
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(in crate::client) enum LanDataStage {
    OutboundQueue,
    UdpSend,
    InboundQueue,
    HookQueue,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(in crate::client) enum LanDataDropReason {
    QueueFull,
    QueueClosed,
    PeerUnavailable,
    GenerationClosed,
    PayloadTooLarge,
    EncodeFailed,
    SendFailed,
    HookDisconnected,
    SessionClosed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::client::lan::path) enum LanDataOutcome {
    Queued,
    Succeeded,
    Dropped(LanDataDropReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::client) struct LanRejectionSnapshot {
    pub stage: LanDataStage,
    pub reason: LanDataDropReason,
    pub count: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::client) struct LanDirectionSnapshot {
    pub queued: u64,
    pub resolved_success: u64,
    pub dropped: u64,
    pub current_queue_depth: usize,
    pub max_queue_depth: usize,
    pub rejections: Vec<LanRejectionSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::client) struct LanEpochDataSnapshot {
    pub lifecycle_epoch: u64,
    pub active: bool,
    pub send: LanDirectionSnapshot,
    pub receive: LanDirectionSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::client) struct LanPeerDataSnapshot {
    pub peer_slot: u32,
    pub peer_steam_id64: u64,
    pub latest_lifecycle_epoch: u64,
    pub active: bool,
    pub send: LanDirectionSnapshot,
    pub receive: LanDirectionSnapshot,
    pub epochs: Vec<LanEpochDataSnapshot>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::client) struct LanDataPlaneSnapshot {
    pub send: LanDirectionSnapshot,
    pub receive: LanDirectionSnapshot,
    pub peers: Vec<LanPeerDataSnapshot>,
    pub transitions_dropped: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::client) enum LanIncidentTransitionKind {
    Started,
    Recovered,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::client) struct LanIncidentTransition {
    pub peer_slot: u32,
    pub lifecycle_epoch: u64,
    pub direction: LanDataDirection,
    pub stage: LanDataStage,
    pub reason: LanDataDropReason,
    pub kind: LanIncidentTransitionKind,
    pub duration: Duration,
    pub dropped_packets: u64,
}

#[derive(Clone)]
pub(in crate::client) struct LanDataPlaneMonitor {
    pub(super) ledger: Arc<LanDataPlaneLedger>,
}

impl LanDataPlaneMonitor {
    #[must_use]
    pub(in crate::client) fn snapshot(&self) -> LanDataPlaneSnapshot {
        self.ledger.snapshot()
    }

    pub(in crate::client) fn drain_transitions(&self) -> Vec<LanIncidentTransition> {
        self.ledger.drain_transitions()
    }

    pub(in crate::client) async fn changed(&self) {
        self.ledger.transition_notify.notified().await;
    }
}

impl DirectionTotals {
    pub(super) fn snapshot(&self) -> LanDirectionSnapshot {
        LanDirectionSnapshot {
            queued: self.queued,
            resolved_success: self.resolved_success,
            dropped: self.dropped,
            current_queue_depth: self.current_queue_depth,
            max_queue_depth: self.max_queue_depth,
            rejections: self
                .rejections
                .iter()
                .map(|((stage, reason), count)| LanRejectionSnapshot {
                    stage: *stage,
                    reason: *reason,
                    count: *count,
                })
                .collect(),
        }
    }
}

pub(super) fn fold_directions<'a>(
    directions: impl Iterator<Item = &'a LanDirectionSnapshot>,
) -> LanDirectionSnapshot {
    let mut totals = DirectionTotals::default();
    for direction in directions {
        totals.queued = totals.queued.saturating_add(direction.queued);
        totals.resolved_success = totals
            .resolved_success
            .saturating_add(direction.resolved_success);
        totals.dropped = totals.dropped.saturating_add(direction.dropped);
        totals.current_queue_depth = totals
            .current_queue_depth
            .saturating_add(direction.current_queue_depth);
        totals.max_queue_depth = totals
            .max_queue_depth
            .saturating_add(direction.max_queue_depth);
        for rejection in &direction.rejections {
            let count = totals
                .rejections
                .entry((rejection.stage, rejection.reason))
                .or_default();
            *count = count.saturating_add(rejection.count);
        }
    }
    totals.snapshot()
}
