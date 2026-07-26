use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tractor_beam_direct_protocol::{InstanceId, PeerIdentity};

use super::PathManager;
use crate::client::{
    lan::membership::PeerLifecycleEpoch,
    packet_flow::{InboundGamePacket, OutboundGamePacket},
};

mod incidents;
mod model;
use incidents::{
    ActiveIncident, IncidentKey, close_epoch_incidents, close_peer_incidents, finish_incidents,
    update_incident_drop, update_incidents,
};
pub(super) use model::LanDataOutcome;
use model::fold_directions;
pub(in crate::client) use model::{
    LanDataDirection, LanDataDropReason, LanDataPlaneMonitor, LanDataPlaneSnapshot, LanDataStage,
    LanDirectionSnapshot, LanEpochDataSnapshot, LanIncidentTransition, LanIncidentTransitionKind,
    LanPeerDataSnapshot, LanRejectionSnapshot,
};

pub(super) const PER_PEER_PACKET_QUEUE_CAPACITY: usize = 256;
const CLOSED_EPOCH_DETAIL_CAPACITY: usize = 8;
const TRANSITION_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct PeerDataKey {
    pub(super) identity: PeerIdentity,
    pub(super) peer_slot: u32,
    pub(super) epoch: PeerLifecycleEpoch,
}

#[derive(Clone)]
pub(super) struct PeerGenerationGate {
    active: Arc<Mutex<bool>>,
    cancellation: CancellationToken,
}

impl PeerGenerationGate {
    fn new(parent_cancellation: &CancellationToken) -> Self {
        Self {
            active: Arc::new(Mutex::new(true)),
            cancellation: parent_cancellation.child_token(),
        }
    }

    pub(super) fn with_active<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        let active = self
            .active
            .lock()
            .expect("LAN peer generation gate lock poisoned");
        (*active).then(operation)
    }

    pub(super) fn close(&self) {
        let mut active = self
            .active
            .lock()
            .expect("LAN peer generation gate lock poisoned");
        *active = false;
        self.cancellation.cancel();
    }
}

pub(super) struct PeerPathDataPlane {
    pub(super) key: PeerDataKey,
    pub(super) gate: PeerGenerationGate,
    pub(super) inbound: VecDeque<InboundGamePacket>,
    pub(super) outbound: tokio::sync::mpsc::Sender<OutboundGamePacket>,
}

impl PeerPathDataPlane {
    pub(super) fn new(
        key: PeerDataKey,
        parent_cancellation: &CancellationToken,
        outbound: tokio::sync::mpsc::Sender<OutboundGamePacket>,
    ) -> Self {
        Self {
            key,
            gate: PeerGenerationGate::new(parent_cancellation),
            inbound: VecDeque::with_capacity(PER_PEER_PACKET_QUEUE_CAPACITY),
            outbound,
        }
    }
}

pub(in crate::client) struct LanInboundDelivery {
    pub packet: InboundGamePacket,
    pub receipt: LanInboundReceipt,
}

pub(in crate::client) struct LanInboundReceipt {
    key: PeerDataKey,
    gate: PeerGenerationGate,
    ledger: Arc<LanDataPlaneLedger>,
    completed: bool,
}

impl LanInboundReceipt {
    pub(super) fn new(
        key: PeerDataKey,
        gate: PeerGenerationGate,
        ledger: Arc<LanDataPlaneLedger>,
    ) -> Self {
        Self {
            key,
            gate,
            ledger,
            completed: false,
        }
    }

    pub(in crate::client) fn with_active<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        self.gate.with_active(operation)
    }

    pub(in crate::client) fn complete_accepted(mut self) {
        self.ledger.record(
            self.key,
            LanDataDirection::Receive,
            LanDataStage::HookQueue,
            LanDataOutcome::Succeeded,
            None,
        );
        self.completed = true;
    }

    pub(in crate::client) fn complete_dropped(mut self, reason: LanDataDropReason) {
        self.ledger.record(
            self.key,
            LanDataDirection::Receive,
            LanDataStage::HookQueue,
            LanDataOutcome::Dropped(reason),
            None,
        );
        if matches!(
            reason,
            LanDataDropReason::GenerationClosed | LanDataDropReason::SessionClosed
        ) {
            self.ledger.close_epoch(self.key, Instant::now());
        }
        self.completed = true;
    }
}

impl Drop for LanInboundReceipt {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.ledger.record(
            self.key,
            LanDataDirection::Receive,
            LanDataStage::HookQueue,
            LanDataOutcome::Dropped(LanDataDropReason::SessionClosed),
            None,
        );
    }
}

pub(in crate::client) struct LanInboundReceiver {
    manager: Arc<PathManager>,
    cursor: usize,
}

impl LanInboundReceiver {
    pub(super) fn new(manager: Arc<PathManager>) -> Self {
        Self { manager, cursor: 0 }
    }

    pub(in crate::client) async fn recv(&mut self) -> Option<LanInboundDelivery> {
        loop {
            let notified = self.manager.inbound_notify.notified();
            if let Some(delivery) = self.manager.pop_next_inbound(&mut self.cursor) {
                return Some(delivery);
            }
            tokio::select! {
                () = self.manager.cancellation.cancelled() => return None,
                () = notified => {}
            }
        }
    }

    #[cfg(test)]
    pub(in crate::client) fn try_recv(
        &mut self,
    ) -> Result<LanInboundDelivery, tokio::sync::mpsc::error::TryRecvError> {
        self.manager
            .pop_next_inbound(&mut self.cursor)
            .ok_or(tokio::sync::mpsc::error::TryRecvError::Empty)
    }
}

pub(super) struct LanDataPlaneLedger {
    inner: Mutex<LedgerState>,
    transition_notify: Notify,
}

#[derive(Default)]
struct LedgerState {
    peer_slots: HashMap<u64, u32>,
    next_peer_slot: u32,
    peers: BTreeMap<u32, PeerLedger>,
    incidents: BTreeMap<IncidentKey, ActiveIncident>,
    transitions: VecDeque<LanIncidentTransition>,
    transitions_dropped: u64,
}

struct PeerLedger {
    peer_steam_id64: u64,
    latest_epoch: PeerLifecycleEpoch,
    active_epoch: Option<PeerLifecycleEpoch>,
    send: DirectionTotals,
    receive: DirectionTotals,
    epochs: VecDeque<EpochLedger>,
}

struct EpochLedger {
    epoch: PeerLifecycleEpoch,
    active: bool,
    send: DirectionTotals,
    receive: DirectionTotals,
}

#[derive(Clone, Debug, Default)]
struct DirectionTotals {
    queued: u64,
    resolved_success: u64,
    dropped: u64,
    current_queue_depth: usize,
    max_queue_depth: usize,
    rejections: BTreeMap<(LanDataStage, LanDataDropReason), u64>,
}

impl LanDataPlaneLedger {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(LedgerState {
                next_peer_slot: 1,
                ..LedgerState::default()
            }),
            transition_notify: Notify::new(),
        })
    }

    pub(super) fn monitor(self: &Arc<Self>) -> LanDataPlaneMonitor {
        LanDataPlaneMonitor {
            ledger: self.clone(),
        }
    }

    pub(super) fn activate(
        &self,
        identity: PeerIdentity,
        epoch: PeerLifecycleEpoch,
    ) -> PeerDataKey {
        let mut state = self.inner.lock().expect("LAN data ledger lock poisoned");
        let peer_slot = peer_slot(&mut state, identity.steam_id64);
        close_peer_incidents(&mut state, peer_slot, Instant::now());
        let peer = state.peers.entry(peer_slot).or_insert_with(|| PeerLedger {
            peer_steam_id64: identity.steam_id64,
            latest_epoch: epoch,
            active_epoch: None,
            send: DirectionTotals::default(),
            receive: DirectionTotals::default(),
            epochs: VecDeque::new(),
        });
        peer.latest_epoch = epoch;
        peer.active_epoch = Some(epoch);
        peer.epochs.push_back(EpochLedger {
            epoch,
            active: true,
            send: DirectionTotals::default(),
            receive: DirectionTotals::default(),
        });
        trim_closed_epochs(peer);
        let key = PeerDataKey {
            identity,
            peer_slot,
            epoch,
        };
        drop(state);
        self.transition_notify.notify_one();
        key
    }

    pub(super) fn unattached(&self, steam_id64: u64) -> PeerDataKey {
        let mut state = self.inner.lock().expect("LAN data ledger lock poisoned");
        let peer_slot = peer_slot(&mut state, steam_id64);
        state.peers.entry(peer_slot).or_insert_with(|| PeerLedger {
            peer_steam_id64: steam_id64,
            latest_epoch: PeerLifecycleEpoch::UNATTACHED,
            active_epoch: None,
            send: DirectionTotals::default(),
            receive: DirectionTotals::default(),
            epochs: VecDeque::from([EpochLedger {
                epoch: PeerLifecycleEpoch::UNATTACHED,
                active: false,
                send: DirectionTotals::default(),
                receive: DirectionTotals::default(),
            }]),
        });
        PeerDataKey {
            identity: PeerIdentity::new(steam_id64, InstanceId::from_bytes([0; 16])),
            peer_slot,
            epoch: PeerLifecycleEpoch::UNATTACHED,
        }
    }

    pub(super) fn record(
        &self,
        key: PeerDataKey,
        direction: LanDataDirection,
        stage: LanDataStage,
        outcome: LanDataOutcome,
        queue_depth: Option<usize>,
    ) {
        let now = Instant::now();
        let mut state = self.inner.lock().expect("LAN data ledger lock poisoned");
        let Some(mut peer) = state.peers.remove(&key.peer_slot) else {
            return;
        };
        update_direction(
            direction_totals_mut(&mut peer, direction),
            stage,
            outcome,
            queue_depth,
        );
        if let Some(epoch) = peer
            .epochs
            .iter_mut()
            .find(|epoch| epoch.epoch == key.epoch)
        {
            update_direction(
                epoch_direction_totals_mut(epoch, direction),
                stage,
                outcome,
                queue_depth,
            );
        }
        state.peers.insert(key.peer_slot, peer);
        update_incidents(&mut state, key, direction, stage, outcome, now);
        drop(state);
        self.transition_notify.notify_one();
    }

    pub(super) fn record_dropped_batch(
        &self,
        key: PeerDataKey,
        direction: LanDataDirection,
        stage: LanDataStage,
        reason: LanDataDropReason,
        count: u64,
        queue_depth: usize,
    ) {
        let now = Instant::now();
        if count == 0 {
            self.set_queue_depth(key, direction, queue_depth);
            return;
        }
        let mut state = self.inner.lock().expect("LAN data ledger lock poisoned");
        let Some(mut peer) = state.peers.remove(&key.peer_slot) else {
            return;
        };
        update_dropped_batch(
            direction_totals_mut(&mut peer, direction),
            stage,
            reason,
            count,
            queue_depth,
        );
        if let Some(epoch) = peer
            .epochs
            .iter_mut()
            .find(|epoch| epoch.epoch == key.epoch)
        {
            update_dropped_batch(
                epoch_direction_totals_mut(epoch, direction),
                stage,
                reason,
                count,
                queue_depth,
            );
        }
        state.peers.insert(key.peer_slot, peer);
        update_incident_drop(&mut state, key, direction, stage, reason, count, now);
        drop(state);
        self.transition_notify.notify_one();
    }

    pub(super) fn set_queue_depth(
        &self,
        key: PeerDataKey,
        direction: LanDataDirection,
        queue_depth: usize,
    ) {
        let mut state = self.inner.lock().expect("LAN data ledger lock poisoned");
        let Some(mut peer) = state.peers.remove(&key.peer_slot) else {
            return;
        };
        set_direction_depth(direction_totals_mut(&mut peer, direction), queue_depth);
        if let Some(epoch) = peer
            .epochs
            .iter_mut()
            .find(|epoch| epoch.epoch == key.epoch)
        {
            set_direction_depth(epoch_direction_totals_mut(epoch, direction), queue_depth);
        }
        state.peers.insert(key.peer_slot, peer);
    }

    pub(super) fn close_epoch(&self, key: PeerDataKey, now: Instant) {
        let mut state = self.inner.lock().expect("LAN data ledger lock poisoned");
        if let Some(peer) = state.peers.get_mut(&key.peer_slot) {
            if peer.active_epoch == Some(key.epoch) {
                peer.active_epoch = None;
            }
            if let Some(epoch) = peer
                .epochs
                .iter_mut()
                .find(|epoch| epoch.epoch == key.epoch)
            {
                epoch.active = false;
                epoch.send.current_queue_depth = 0;
                epoch.receive.current_queue_depth = 0;
            }
            peer.send.current_queue_depth = 0;
            peer.receive.current_queue_depth = 0;
            trim_closed_epochs(peer);
        }
        close_epoch_incidents(&mut state, key, now);
        drop(state);
        self.transition_notify.notify_one();
    }

    fn snapshot(&self) -> LanDataPlaneSnapshot {
        let state = self.inner.lock().expect("LAN data ledger lock poisoned");
        let peers = state
            .peers
            .iter()
            .map(|(peer_slot, peer)| LanPeerDataSnapshot {
                peer_slot: *peer_slot,
                peer_steam_id64: peer.peer_steam_id64,
                latest_lifecycle_epoch: peer.latest_epoch.get(),
                active: peer.active_epoch.is_some(),
                send: peer.send.snapshot(),
                receive: peer.receive.snapshot(),
                epochs: peer
                    .epochs
                    .iter()
                    .map(|epoch| LanEpochDataSnapshot {
                        lifecycle_epoch: epoch.epoch.get(),
                        active: epoch.active,
                        send: epoch.send.snapshot(),
                        receive: epoch.receive.snapshot(),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        LanDataPlaneSnapshot {
            send: fold_directions(peers.iter().map(|peer| &peer.send)),
            receive: fold_directions(peers.iter().map(|peer| &peer.receive)),
            peers,
            transitions_dropped: state.transitions_dropped,
        }
    }

    fn drain_transitions(&self) -> Vec<LanIncidentTransition> {
        self.inner
            .lock()
            .expect("LAN data ledger lock poisoned")
            .transitions
            .drain(..)
            .collect()
    }

    pub(super) fn finish_all(&self, now: Instant) {
        let mut state = self.inner.lock().expect("LAN data ledger lock poisoned");
        finish_incidents(&mut state, now);
        drop(state);
        self.transition_notify.notify_one();
    }
}

fn direction_totals_mut(
    peer: &mut PeerLedger,
    direction: LanDataDirection,
) -> &mut DirectionTotals {
    match direction {
        LanDataDirection::Send => &mut peer.send,
        LanDataDirection::Receive => &mut peer.receive,
    }
}

fn peer_slot(state: &mut LedgerState, steam_id64: u64) -> u32 {
    if let Some(slot) = state.peer_slots.get(&steam_id64) {
        return *slot;
    }
    let slot = state.next_peer_slot;
    state.next_peer_slot = state.next_peer_slot.saturating_add(1);
    state.peer_slots.insert(steam_id64, slot);
    slot
}

fn epoch_direction_totals_mut(
    epoch: &mut EpochLedger,
    direction: LanDataDirection,
) -> &mut DirectionTotals {
    match direction {
        LanDataDirection::Send => &mut epoch.send,
        LanDataDirection::Receive => &mut epoch.receive,
    }
}

fn update_direction(
    totals: &mut DirectionTotals,
    stage: LanDataStage,
    outcome: LanDataOutcome,
    queue_depth: Option<usize>,
) {
    match outcome {
        LanDataOutcome::Queued => totals.queued = totals.queued.saturating_add(1),
        LanDataOutcome::Succeeded => {
            totals.resolved_success = totals.resolved_success.saturating_add(1);
        }
        LanDataOutcome::Dropped(reason) => {
            totals.dropped = totals.dropped.saturating_add(1);
            let count = totals.rejections.entry((stage, reason)).or_default();
            *count = count.saturating_add(1);
        }
    }
    if let Some(depth) = queue_depth {
        totals.current_queue_depth = depth;
        totals.max_queue_depth = totals.max_queue_depth.max(depth);
    }
}

fn update_dropped_batch(
    totals: &mut DirectionTotals,
    stage: LanDataStage,
    reason: LanDataDropReason,
    count: u64,
    queue_depth: usize,
) {
    totals.dropped = totals.dropped.saturating_add(count);
    let rejection_count = totals.rejections.entry((stage, reason)).or_default();
    *rejection_count = rejection_count.saturating_add(count);
    totals.current_queue_depth = queue_depth;
    totals.max_queue_depth = totals.max_queue_depth.max(queue_depth);
}

fn set_direction_depth(totals: &mut DirectionTotals, queue_depth: usize) {
    totals.current_queue_depth = queue_depth;
    totals.max_queue_depth = totals.max_queue_depth.max(queue_depth);
}

fn trim_closed_epochs(peer: &mut PeerLedger) {
    while peer.epochs.iter().filter(|epoch| !epoch.active).count() > CLOSED_EPOCH_DETAIL_CAPACITY {
        let Some(index) = peer.epochs.iter().position(|epoch| !epoch.active) else {
            break;
        };
        peer.epochs.remove(index);
    }
}

#[cfg(test)]
#[path = "data_plane_tests.rs"]
mod tests;
