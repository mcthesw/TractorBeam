use std::time::{Duration, Instant};

use super::{
    LanDataDirection, LanDataDropReason, LanDataOutcome, LanDataStage, LanIncidentTransition,
    LanIncidentTransitionKind, LedgerState, PeerDataKey, PeerLifecycleEpoch, TRANSITION_CAPACITY,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct IncidentKey {
    pub(super) peer_slot: u32,
    pub(super) epoch: PeerLifecycleEpoch,
    direction: LanDataDirection,
    stage: LanDataStage,
    reason: LanDataDropReason,
}

pub(super) struct ActiveIncident {
    started_at: Instant,
    dropped_packets: u64,
}

pub(super) fn update_incidents(
    state: &mut LedgerState,
    key: PeerDataKey,
    direction: LanDataDirection,
    stage: LanDataStage,
    outcome: LanDataOutcome,
    now: Instant,
) {
    match outcome {
        LanDataOutcome::Dropped(reason) => {
            update_incident_drop(state, key, direction, stage, reason, 1, now);
        }
        LanDataOutcome::Queued | LanDataOutcome::Succeeded => {
            recover_incidents(state, key, direction, stage, now);
        }
    }
}

pub(super) fn update_incident_drop(
    state: &mut LedgerState,
    key: PeerDataKey,
    direction: LanDataDirection,
    stage: LanDataStage,
    reason: LanDataDropReason,
    count: u64,
    now: Instant,
) {
    let incident_key = IncidentKey {
        peer_slot: key.peer_slot,
        epoch: key.epoch,
        direction,
        stage,
        reason,
    };
    if let Some(incident) = state.incidents.get_mut(&incident_key) {
        incident.dropped_packets = incident.dropped_packets.saturating_add(count);
        return;
    }
    state.incidents.insert(
        incident_key,
        ActiveIncident {
            started_at: now,
            dropped_packets: count,
        },
    );
    push_transition(
        state,
        transition(
            incident_key,
            LanIncidentTransitionKind::Started,
            Duration::ZERO,
            count,
        ),
    );
}

fn recover_incidents(
    state: &mut LedgerState,
    key: PeerDataKey,
    direction: LanDataDirection,
    stage: LanDataStage,
    now: Instant,
) {
    close_matching(
        state,
        |incident| {
            incident.peer_slot == key.peer_slot
                && incident.epoch == key.epoch
                && incident.direction == direction
                && incident.stage == stage
        },
        LanIncidentTransitionKind::Recovered,
        now,
    );
}

pub(super) fn close_epoch_incidents(state: &mut LedgerState, key: PeerDataKey, now: Instant) {
    close_matching(
        state,
        |incident| incident.peer_slot == key.peer_slot && incident.epoch == key.epoch,
        LanIncidentTransitionKind::Closed,
        now,
    );
}

pub(super) fn close_peer_incidents(state: &mut LedgerState, peer_slot: u32, now: Instant) {
    close_matching(
        state,
        |incident| incident.peer_slot == peer_slot,
        LanIncidentTransitionKind::Closed,
        now,
    );
}

pub(super) fn finish_incidents(state: &mut LedgerState, now: Instant) {
    close_matching(state, |_| true, LanIncidentTransitionKind::Closed, now);
}

fn close_matching(
    state: &mut LedgerState,
    matches: impl Fn(&IncidentKey) -> bool,
    kind: LanIncidentTransitionKind,
    now: Instant,
) {
    let keys = state
        .incidents
        .keys()
        .filter(|incident| matches(incident))
        .copied()
        .collect::<Vec<_>>();
    for incident_key in keys {
        let Some(incident) = state.incidents.remove(&incident_key) else {
            continue;
        };
        push_transition(
            state,
            transition(
                incident_key,
                kind,
                now.saturating_duration_since(incident.started_at),
                incident.dropped_packets,
            ),
        );
    }
}

fn transition(
    key: IncidentKey,
    kind: LanIncidentTransitionKind,
    duration: Duration,
    dropped_packets: u64,
) -> LanIncidentTransition {
    LanIncidentTransition {
        peer_slot: key.peer_slot,
        lifecycle_epoch: key.epoch.get(),
        direction: key.direction,
        stage: key.stage,
        reason: key.reason,
        kind,
        duration,
        dropped_packets,
    }
}

fn push_transition(state: &mut LedgerState, transition: LanIncidentTransition) {
    if state.transitions.len() == TRANSITION_CAPACITY {
        state.transitions.pop_front();
        state.transitions_dropped = state.transitions_dropped.saturating_add(1);
    }
    state.transitions.push_back(transition);
}
