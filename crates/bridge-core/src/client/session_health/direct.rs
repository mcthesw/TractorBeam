use std::collections::BTreeMap;

use serde::Serialize;

use crate::client::lan::{
    LanDataDropReason, LanDataPlaneSnapshot, LanDataStage, LanDirectionSnapshot,
    LanEpochDataSnapshot, LanPeerDataSnapshot, LanRejectionSnapshot,
};

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectFlowDirection {
    #[default]
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectFlowStage {
    OutboundQueue,
    UdpSend,
    InboundQueue,
    HookQueue,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectDropReason {
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DirectRejectionHealthSnapshot {
    pub direction: DirectFlowDirection,
    pub stage: DirectFlowStage,
    pub reason: DirectDropReason,
    pub count: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DirectDirectionHealthSnapshot {
    pub direction: DirectFlowDirection,
    pub queued: u64,
    pub resolved_success: u64,
    pub dropped: u64,
    pub current_queue_depth: usize,
    pub max_queue_depth: usize,
    pub rejections: Vec<DirectRejectionHealthSnapshot>,
}

impl DirectDirectionHealthSnapshot {
    #[must_use]
    pub fn resolved_outcomes(&self) -> u128 {
        u128::from(self.resolved_success).saturating_add(u128::from(self.dropped))
    }

    #[must_use]
    pub fn loss_percent(&self) -> Option<u8> {
        rounded_loss_percent(self.resolved_success, self.dropped)
    }

    fn from_lan(direction: DirectFlowDirection, snapshot: LanDirectionSnapshot) -> Self {
        Self {
            direction,
            queued: snapshot.queued,
            resolved_success: snapshot.resolved_success,
            dropped: snapshot.dropped,
            current_queue_depth: snapshot.current_queue_depth,
            max_queue_depth: snapshot.max_queue_depth,
            rejections: snapshot
                .rejections
                .into_iter()
                .map(|rejection| DirectRejectionHealthSnapshot::from_lan(direction, rejection))
                .collect(),
        }
    }

    fn fold<'a>(direction: DirectFlowDirection, snapshots: impl Iterator<Item = &'a Self>) -> Self {
        let mut folded = Self {
            direction,
            ..Self::default()
        };
        let mut rejections = BTreeMap::new();
        for snapshot in snapshots {
            folded.queued = folded.queued.saturating_add(snapshot.queued);
            folded.resolved_success = folded
                .resolved_success
                .saturating_add(snapshot.resolved_success);
            folded.dropped = folded.dropped.saturating_add(snapshot.dropped);
            folded.current_queue_depth = folded
                .current_queue_depth
                .saturating_add(snapshot.current_queue_depth);
            folded.max_queue_depth = folded
                .max_queue_depth
                .saturating_add(snapshot.max_queue_depth);
            for rejection in &snapshot.rejections {
                let count = rejections
                    .entry((rejection.stage, rejection.reason))
                    .or_insert(0_u64);
                *count = count.saturating_add(rejection.count);
            }
        }
        folded.rejections = rejections
            .into_iter()
            .map(|((stage, reason), count)| DirectRejectionHealthSnapshot {
                direction,
                stage,
                reason,
                count,
            })
            .collect();
        folded
    }
}

impl DirectRejectionHealthSnapshot {
    fn from_lan(direction: DirectFlowDirection, snapshot: LanRejectionSnapshot) -> Self {
        Self {
            direction,
            stage: snapshot.stage.into(),
            reason: snapshot.reason.into(),
            count: snapshot.count,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DirectEpochHealthSnapshot {
    pub lifecycle_epoch: u64,
    pub active: bool,
    pub send: DirectDirectionHealthSnapshot,
    pub receive: DirectDirectionHealthSnapshot,
}

impl DirectEpochHealthSnapshot {
    fn from_lan(snapshot: LanEpochDataSnapshot) -> Self {
        Self {
            lifecycle_epoch: snapshot.lifecycle_epoch,
            active: snapshot.active,
            send: DirectDirectionHealthSnapshot::from_lan(DirectFlowDirection::Send, snapshot.send),
            receive: DirectDirectionHealthSnapshot::from_lan(
                DirectFlowDirection::Receive,
                snapshot.receive,
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DirectPeerHealthSnapshot {
    pub peer_slot: u32,
    pub peer_steam_id64: u64,
    pub latest_lifecycle_epoch: u64,
    pub active: bool,
    pub send: DirectDirectionHealthSnapshot,
    pub receive: DirectDirectionHealthSnapshot,
    pub epochs: Vec<DirectEpochHealthSnapshot>,
}

impl DirectPeerHealthSnapshot {
    fn from_lan(snapshot: LanPeerDataSnapshot) -> Self {
        let mut epochs = snapshot
            .epochs
            .into_iter()
            .map(DirectEpochHealthSnapshot::from_lan)
            .collect::<Vec<_>>();
        epochs.sort_by_key(|epoch| epoch.lifecycle_epoch);
        Self {
            peer_slot: snapshot.peer_slot,
            peer_steam_id64: snapshot.peer_steam_id64,
            latest_lifecycle_epoch: snapshot.latest_lifecycle_epoch,
            active: snapshot.active,
            send: DirectDirectionHealthSnapshot::from_lan(DirectFlowDirection::Send, snapshot.send),
            receive: DirectDirectionHealthSnapshot::from_lan(
                DirectFlowDirection::Receive,
                snapshot.receive,
            ),
            epochs,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DirectFlowHealthSnapshot {
    pub enabled: bool,
    pub send: DirectDirectionHealthSnapshot,
    pub receive: DirectDirectionHealthSnapshot,
    pub peers: Vec<DirectPeerHealthSnapshot>,
    pub transitions_dropped: u64,
}

impl Default for DirectFlowHealthSnapshot {
    fn default() -> Self {
        Self {
            enabled: false,
            send: DirectDirectionHealthSnapshot {
                direction: DirectFlowDirection::Send,
                ..DirectDirectionHealthSnapshot::default()
            },
            receive: DirectDirectionHealthSnapshot {
                direction: DirectFlowDirection::Receive,
                ..DirectDirectionHealthSnapshot::default()
            },
            peers: Vec::new(),
            transitions_dropped: 0,
        }
    }
}

impl DirectFlowHealthSnapshot {
    pub(super) fn from_lan(snapshot: LanDataPlaneSnapshot) -> Self {
        let mut peers = snapshot
            .peers
            .into_iter()
            .map(DirectPeerHealthSnapshot::from_lan)
            .collect::<Vec<_>>();
        peers.sort_by_key(|peer| peer.peer_slot);
        let send = DirectDirectionHealthSnapshot::fold(
            DirectFlowDirection::Send,
            peers.iter().map(|peer| &peer.send),
        );
        let receive = DirectDirectionHealthSnapshot::fold(
            DirectFlowDirection::Receive,
            peers.iter().map(|peer| &peer.receive),
        );
        Self {
            enabled: true,
            send,
            receive,
            peers,
            transitions_dropped: snapshot.transitions_dropped,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DirectOutcomeWindow {
    pub resolved_success: u64,
    pub dropped: u64,
}

impl DirectOutcomeWindow {
    #[must_use]
    pub fn resolved_outcomes(self) -> u128 {
        u128::from(self.resolved_success).saturating_add(u128::from(self.dropped))
    }

    #[must_use]
    pub fn loss_percent(self) -> Option<u8> {
        rounded_loss_percent(self.resolved_success, self.dropped)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DirectFlowWindow {
    pub send: DirectOutcomeWindow,
    pub receive: DirectOutcomeWindow,
}

impl From<LanDataStage> for DirectFlowStage {
    fn from(value: LanDataStage) -> Self {
        match value {
            LanDataStage::OutboundQueue => Self::OutboundQueue,
            LanDataStage::UdpSend => Self::UdpSend,
            LanDataStage::InboundQueue => Self::InboundQueue,
            LanDataStage::HookQueue => Self::HookQueue,
        }
    }
}

impl From<LanDataDropReason> for DirectDropReason {
    fn from(value: LanDataDropReason) -> Self {
        match value {
            LanDataDropReason::QueueFull => Self::QueueFull,
            LanDataDropReason::QueueClosed => Self::QueueClosed,
            LanDataDropReason::PeerUnavailable => Self::PeerUnavailable,
            LanDataDropReason::GenerationClosed => Self::GenerationClosed,
            LanDataDropReason::PayloadTooLarge => Self::PayloadTooLarge,
            LanDataDropReason::EncodeFailed => Self::EncodeFailed,
            LanDataDropReason::SendFailed => Self::SendFailed,
            LanDataDropReason::HookDisconnected => Self::HookDisconnected,
            LanDataDropReason::SessionClosed => Self::SessionClosed,
        }
    }
}

fn rounded_loss_percent(resolved_success: u64, dropped: u64) -> Option<u8> {
    let resolved = u128::from(resolved_success).saturating_add(u128::from(dropped));
    if resolved == 0 {
        return None;
    }
    let rounded = u128::from(dropped)
        .saturating_mul(100)
        .saturating_add(resolved / 2)
        / resolved;
    Some(u8::try_from(rounded.min(100)).unwrap_or(100))
}
