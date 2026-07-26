use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tractor_beam_direct_protocol::{InstanceId, PeerIdentity};

use super::*;

fn identity(id: u8) -> PeerIdentity {
    PeerIdentity::new(u64::from(id), InstanceId::from_bytes([id; 16]))
}

#[test]
fn monitor_keeps_rejections_recorded_before_attachment() {
    let ledger = LanDataPlaneLedger::new();
    let key = ledger.activate(identity(2), PeerLifecycleEpoch::test(1));
    ledger.record(
        key,
        LanDataDirection::Receive,
        LanDataStage::InboundQueue,
        LanDataOutcome::Dropped(LanDataDropReason::QueueFull),
        Some(PER_PEER_PACKET_QUEUE_CAPACITY),
    );

    let monitor = ledger.monitor();
    let snapshot = monitor.snapshot();
    assert_eq!(snapshot.receive.dropped, 1);
    assert_eq!(snapshot.peers[0].receive.dropped, 1);
    assert_eq!(monitor.drain_transitions().len(), 1);
}

#[test]
fn unattached_target_rejection_is_closed_when_the_first_epoch_activates() {
    let ledger = LanDataPlaneLedger::new();
    let key = ledger.unattached(2);
    ledger.record(
        key,
        LanDataDirection::Send,
        LanDataStage::OutboundQueue,
        LanDataOutcome::Dropped(LanDataDropReason::PeerUnavailable),
        Some(0),
    );
    ledger.activate(identity(2), PeerLifecycleEpoch::test(1));

    let monitor = ledger.monitor();
    let snapshot = monitor.snapshot();
    assert_eq!(snapshot.peers[0].latest_lifecycle_epoch, 1);
    assert_eq!(snapshot.peers[0].send.dropped, 1);
    assert_eq!(
        monitor
            .drain_transitions()
            .into_iter()
            .map(|transition| transition.kind)
            .collect::<Vec<_>>(),
        [
            LanIncidentTransitionKind::Started,
            LanIncidentTransitionKind::Closed
        ]
    );
}

#[test]
fn closed_generation_rejects_the_pending_hook_owner_once() {
    let ledger = LanDataPlaneLedger::new();
    let key = ledger.activate(identity(2), PeerLifecycleEpoch::test(1));
    let gate = PeerGenerationGate::new(&CancellationToken::new());
    let receipt = LanInboundReceipt::new(key, gate.clone(), ledger.clone());
    gate.close();

    assert_eq!(receipt.with_active(|| true), None);
    receipt.complete_dropped(LanDataDropReason::GenerationClosed);

    let snapshot = ledger.monitor().snapshot();
    assert_eq!(snapshot.receive.dropped, 1);
    assert_eq!(snapshot.receive.resolved_success, 0);
    assert_eq!(snapshot.peers[0].epochs[0].receive.dropped, 1);
}

#[test]
fn transition_ring_is_bounded_and_reports_overflow() {
    let ledger = LanDataPlaneLedger::new();
    let key = ledger.activate(identity(2), PeerLifecycleEpoch::test(1));
    for _ in 0..130 {
        ledger.record(
            key,
            LanDataDirection::Send,
            LanDataStage::OutboundQueue,
            LanDataOutcome::Dropped(LanDataDropReason::QueueFull),
            Some(PER_PEER_PACKET_QUEUE_CAPACITY),
        );
        ledger.record(
            key,
            LanDataDirection::Send,
            LanDataStage::OutboundQueue,
            LanDataOutcome::Queued,
            Some(1),
        );
    }

    let monitor = ledger.monitor();
    assert_eq!(monitor.drain_transitions().len(), TRANSITION_CAPACITY);
    assert_eq!(monitor.snapshot().transitions_dropped, 4);
}

#[test]
fn evicting_closed_epoch_detail_never_reduces_peer_or_aggregate_totals() {
    let ledger = LanDataPlaneLedger::new();
    for epoch in 1..=10 {
        let key = ledger.activate(identity(2), PeerLifecycleEpoch::test(epoch));
        ledger.record(
            key,
            LanDataDirection::Send,
            LanDataStage::OutboundQueue,
            LanDataOutcome::Queued,
            Some(1),
        );
        ledger.record(
            key,
            LanDataDirection::Send,
            LanDataStage::UdpSend,
            LanDataOutcome::Succeeded,
            None,
        );
        ledger.close_epoch(key, Instant::now());
    }

    let snapshot = ledger.monitor().snapshot();
    assert_eq!(snapshot.peers.len(), 1);
    assert_eq!(snapshot.peers[0].epochs.len(), CLOSED_EPOCH_DETAIL_CAPACITY);
    assert_eq!(snapshot.peers[0].send.queued, 10);
    assert_eq!(snapshot.peers[0].send.resolved_success, 10);
    assert_eq!(snapshot.send.queued, 10);
    assert_eq!(snapshot.send.resolved_success, 10);
}
