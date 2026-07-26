use super::*;
use crate::client::lan::{
    LanDataDropReason, LanDataPlaneSnapshot, LanDataStage, LanDirectionSnapshot,
    LanEpochDataSnapshot, LanPeerDataSnapshot, LanRejectionSnapshot,
};

#[test]
fn latency_summary_reports_percentiles_and_thresholds() {
    let mut accumulator = LatencyAccumulator::default();
    for value in [10, 50, 210, 510, 1_100] {
        accumulator.observe(Duration::from_millis(value));
    }
    let summary = accumulator.summary();
    assert_eq!(summary.count, 5);
    assert_eq!(summary.median_ms, Some(210));
    assert_eq!(summary.p95_ms, Some(1_100));
    assert_eq!(summary.over_500_ms, 2);
    assert_eq!(summary.over_1000_ms, 1);
}

#[test]
fn queue_and_delivery_windows_use_deltas() {
    let previous = QualityBaseline {
        direct_send_succeeded: 9,
        direct_send_dropped: 1,
        direct_receive_accepted: 4,
        direct_receive_dropped: 1,
        queue_drops: 2,
        delivery_gaps: 3,
        delivery_reordered: 4,
        ..QualityBaseline::default()
    };
    let current = QualityBaseline {
        direct_send_succeeded: 12,
        direct_send_dropped: 2,
        direct_receive_accepted: 7,
        direct_receive_dropped: 2,
        queue_drops: 2,
        delivery_gaps: 4,
        delivery_reordered: 9,
        ..QualityBaseline::default()
    };
    let window = current.delta(previous, Duration::from_secs(5));
    assert_eq!(window.direct.send.resolved_success, 3);
    assert_eq!(window.direct.send.dropped, 1);
    assert_eq!(window.direct.receive.resolved_success, 3);
    assert_eq!(window.direct.receive.dropped, 1);
    assert_eq!(window.queue_drops, 0);
    assert_eq!(window.delivery_gaps, 1);
    assert_eq!(window.delivery_reordered, 5);
}

#[test]
fn direct_peer_totals_drive_aggregate_window_and_directional_quality() {
    let start = Instant::now();
    let active = start + Duration::from_secs(ACTIVE_TRAFFIC_STARTUP_GRACE_SECONDS);
    let mut health = SessionHealth::new(false, Duration::from_secs(1), start);

    health.refresh_direct(LanDataPlaneSnapshot {
        send: lan_direction(999, 999, 999),
        receive: lan_direction(999, 999, 999),
        peers: vec![
            lan_peer(2, 76561198000000002, (5, 4, 1), (10, 8, 2)),
            lan_peer(1, 76561198000000001, (7, 3, 2), (7, 6, 1)),
        ],
        transitions_dropped: 4,
    });
    let snapshot = health.snapshot(active);

    assert!(snapshot.direct.enabled);
    assert_eq!(snapshot.direct.peers[0].peer_slot, 1);
    assert_eq!(snapshot.direct.peers[1].peer_slot, 2);
    assert_eq!(snapshot.direct.send.queued, 12);
    assert_eq!(snapshot.direct.send.resolved_success, 7);
    assert_eq!(snapshot.direct.send.dropped, 3);
    assert_eq!(snapshot.direct.receive.queued, 17);
    assert_eq!(snapshot.direct.receive.resolved_success, 14);
    assert_eq!(snapshot.direct.receive.dropped, 3);
    assert_eq!(snapshot.direct.transitions_dropped, 4);
    assert_eq!(snapshot.queues.total_dropped(), 0);
    assert_eq!(snapshot.network_send_dropped, 0);
    assert_eq!(snapshot.window.direct.send.resolved_success, 7);
    assert_eq!(snapshot.window.direct.send.dropped, 3);
    assert_eq!(snapshot.window.direct.receive.resolved_success, 14);
    assert_eq!(snapshot.window.direct.receive.dropped, 3);
    assert_eq!(snapshot.quality, SessionQuality::Poor);
    assert_eq!(
        snapshot.reasons,
        [
            SessionQualityReason::DirectSendDrop,
            SessionQualityReason::DirectReceiveDrop,
        ]
    );
}

#[test]
fn closed_epoch_detail_eviction_does_not_hide_lifetime_window_progress() {
    let start = Instant::now();
    let active = start + Duration::from_secs(ACTIVE_TRAFFIC_STARTUP_GRACE_SECONDS);
    let mut health = SessionHealth::new(false, Duration::from_secs(1), start);
    health.refresh_direct(LanDataPlaneSnapshot {
        peers: vec![lan_peer(1, 76561198000000001, (4, 3, 1), (4, 4, 0))],
        ..LanDataPlaneSnapshot::default()
    });
    let first = health.snapshot(active);
    assert_eq!(first.direct.peers[0].epochs.len(), 1);

    let mut peer = lan_peer(1, 76561198000000001, (5, 4, 1), (6, 5, 1));
    peer.epochs.clear();
    health.refresh_direct(LanDataPlaneSnapshot {
        peers: vec![peer],
        ..LanDataPlaneSnapshot::default()
    });
    let second = health.snapshot(active + Duration::from_secs(5));

    assert!(second.direct.peers[0].epochs.is_empty());
    assert_eq!(second.direct.send.resolved_success, 4);
    assert_eq!(second.direct.receive.dropped, 1);
    assert_eq!(second.window.direct.send.resolved_success, 1);
    assert_eq!(second.window.direct.receive.resolved_success, 1);
    assert_eq!(second.window.direct.receive.dropped, 1);
}

#[test]
fn three_player_six_edge_fixture_reconciles_every_local_gui_input() {
    let start = Instant::now();
    let active = start + Duration::from_secs(ACTIVE_TRAFFIC_STARTUP_GRACE_SECONDS);
    let mut room_send_resolved = 0_u128;
    let mut room_receive_resolved = 0_u128;

    for local_index in 0..3_u64 {
        let mut health = SessionHealth::new(false, Duration::from_secs(1), start);
        let peers = (0..3_u64)
            .filter(|remote_index| *remote_index != local_index)
            .enumerate()
            .map(|(slot, remote_index)| {
                lan_peer(
                    u32::try_from(slot + 1).unwrap(),
                    76561198000000000 + remote_index,
                    (1, 1, 0),
                    (1, 1, 0),
                )
            })
            .collect();
        health.refresh_direct(LanDataPlaneSnapshot {
            peers,
            ..LanDataPlaneSnapshot::default()
        });
        let snapshot = health.snapshot(active);

        assert_eq!(snapshot.direct.send.resolved_outcomes(), 2);
        assert_eq!(snapshot.direct.receive.resolved_outcomes(), 2);
        assert_eq!(snapshot.window.direct.send.loss_percent(), Some(0));
        assert_eq!(snapshot.window.direct.receive.loss_percent(), Some(0));
        room_send_resolved =
            room_send_resolved.saturating_add(snapshot.direct.send.resolved_outcomes());
        room_receive_resolved =
            room_receive_resolved.saturating_add(snapshot.direct.receive.resolved_outcomes());
    }

    assert_eq!(room_send_resolved, 6);
    assert_eq!(room_receive_resolved, 6);
}

#[test]
fn direct_loss_percentage_uses_terminal_outcomes_and_widened_rounding() {
    assert_eq!(DirectOutcomeWindow::default().loss_percent(), None);
    assert_eq!(
        DirectOutcomeWindow {
            resolved_success: 10,
            dropped: 0,
        }
        .loss_percent(),
        Some(0)
    );
    assert_eq!(
        DirectOutcomeWindow {
            resolved_success: 17,
            dropped: 3,
        }
        .loss_percent(),
        Some(15)
    );
    assert_eq!(
        DirectOutcomeWindow {
            resolved_success: u64::MAX,
            dropped: u64::MAX,
        }
        .loss_percent(),
        Some(50)
    );
    assert_eq!(
        DirectOutcomeWindow {
            resolved_success: 0,
            dropped: u64::MAX,
        }
        .loss_percent(),
        Some(100)
    );
}

#[test]
fn startup_and_idle_are_unavailable() {
    for (elapsed, window) in [(5, active_window()), (60, SessionHealthWindow::default())] {
        let assessment = classify_quality(elapsed, window);
        assert_eq!(assessment.quality, SessionQuality::Unavailable);
        assert_eq!(assessment.confidence, QualityConfidence::None);
        assert_eq!(assessment.reasons, [SessionQualityReason::StartupOrIdle]);
    }
}

#[test]
fn current_anomalies_have_deterministic_reasons_and_severity() {
    let watch = classify_quality(
        60,
        SessionHealthWindow {
            delivery_gaps: 1,
            runtime_rtt_timeouts: 1,
            ..active_window()
        },
    );
    assert_eq!(watch.quality, SessionQuality::Watch);
    assert_eq!(
        watch.reasons,
        [
            SessionQualityReason::DeliveryGap,
            SessionQualityReason::RuntimeRttTimeout,
        ]
    );

    let poor = classify_quality(
        60,
        SessionHealthWindow {
            queue_drops: 1,
            hook_send_over_500_ms: 1,
            hook_send_over_1000_ms: 1,
            ..active_window()
        },
    );
    assert_eq!(poor.quality, SessionQuality::Poor);
    assert_eq!(
        poor.reasons,
        [
            SessionQualityReason::LocalQueueDrop,
            SessionQualityReason::HookSendStall,
        ]
    );
}

#[test]
fn recovered_window_is_not_degraded_by_lifetime_anomaly() {
    let start = Instant::now();
    let mut health = SessionHealth::new(true, Duration::from_millis(10), start);
    let startup = start + Duration::from_secs(5);
    health.observe_hook_in_recv(1, startup);
    health.observe_network_recv(1, startup);
    health.observe_outbound_enqueue(false);
    assert_eq!(
        health.snapshot(startup).quality,
        SessionQuality::Unavailable
    );

    let active = start + Duration::from_secs(ACTIVE_TRAFFIC_STARTUP_GRACE_SECONDS);
    health.observe_hook_in_recv(1, active);
    health.observe_network_recv(1, active);
    let recovered = health.snapshot(active);
    assert_eq!(recovered.queues.total_dropped(), 1);
    assert_eq!(recovered.window.queue_drops, 0);
    assert_eq!(recovered.quality, SessionQuality::Good);
}

#[test]
fn route_neutral_network_evidence_reports_confirmed_delivery_and_send_drops() {
    let start = Instant::now();
    let active = start + Duration::from_secs(ACTIVE_TRAFFIC_STARTUP_GRACE_SECONDS);
    let mut health = SessionHealth::new(false, Duration::from_secs(1), start);

    health.observe_hook_in_recv(8, active);
    health.observe_network_recv(8, active);
    let stream = DeliveryStreamId::from_bytes([1; 16]);
    health.observe_delivery(42, stream, 10);
    health.observe_network_recv(8, active + Duration::from_millis(1));
    health.observe_delivery(42, stream, 12);
    health.observe_network_recv(8, active + Duration::from_millis(2));
    health.observe_delivery(42, stream, 140);
    health.observe_network_send_drop();

    let snapshot = health.snapshot(active + Duration::from_millis(3));

    assert_eq!(snapshot.network_recv.packets, 3);
    assert_eq!(snapshot.delivery.confirmed_gaps, 1);
    assert_eq!(snapshot.network_send_dropped, 1);
    assert_eq!(snapshot.window.network_send_dropped, 1);
    assert_eq!(snapshot.quality, SessionQuality::Poor);
    assert_eq!(
        snapshot.reasons,
        [
            SessionQualityReason::NetworkSendDrop,
            SessionQualityReason::DeliveryGap,
        ]
    );
}

#[test]
fn delivery_tracker_baselines_reorders_and_confirms_only_aged_gaps() {
    let stream = DeliveryStreamId::from_bytes([1; 16]);
    let mut stats = DeliveryStats::default();

    stats.observe(42, stream, 4_812);
    assert_eq!(stats.snapshot().confirmed_gaps, 0);
    stats.observe(42, stream, 4_814);
    assert_eq!(stats.snapshot().open_gaps, 1);
    stats.observe(42, stream, 4_813);
    let reordered = stats.snapshot();
    assert_eq!(reordered.open_gaps, 0);
    assert_eq!(reordered.reordered, 1);

    stats.observe(42, stream, 4_816);
    stats.observe(42, stream, 4_944);
    let confirmed = stats.snapshot();
    assert_eq!(confirmed.confirmed_gaps, 1);
}

#[test]
fn retired_delivery_stream_cannot_reactivate() {
    let first = DeliveryStreamId::from_bytes([1; 16]);
    let second = DeliveryStreamId::from_bytes([2; 16]);
    let mut stats = DeliveryStats::default();

    stats.observe(42, first, 1);
    stats.observe(42, second, 1);
    stats.observe(42, first, 2);

    let snapshot = stats.snapshot();
    assert_eq!(snapshot.first_packets, 2);
    assert_eq!(snapshot.in_order, 0);
    assert_eq!(snapshot.tracked_peers, 1);
}

const fn active_window() -> SessionHealthWindow {
    SessionHealthWindow {
        duration_seconds: 5,
        hook_in_packets: 20,
        network_recv_packets: 20,
        direct: DirectFlowWindow {
            send: DirectOutcomeWindow {
                resolved_success: 0,
                dropped: 0,
            },
            receive: DirectOutcomeWindow {
                resolved_success: 0,
                dropped: 0,
            },
        },
        network_send_dropped: 0,
        queue_drops: 0,
        delivery_gaps: 0,
        delivery_reordered: 0,
        runtime_rtt_sent: 0,
        runtime_rtt_timeouts: 0,
        hook_send_over_500_ms: 0,
        hook_send_over_1000_ms: 0,
        network_gap_over_500_ms: 0,
        network_gap_over_1000_ms: 0,
    }
}

fn lan_peer(
    peer_slot: u32,
    peer_steam_id64: u64,
    send_outcomes: (u64, u64, u64),
    receive_outcomes: (u64, u64, u64),
) -> LanPeerDataSnapshot {
    let (send_queued, send_succeeded, send_dropped) = send_outcomes;
    let (receive_queued, receive_accepted, receive_dropped) = receive_outcomes;
    let send = lan_direction(send_queued, send_succeeded, send_dropped);
    let receive = lan_direction(receive_queued, receive_accepted, receive_dropped);
    LanPeerDataSnapshot {
        peer_slot,
        peer_steam_id64,
        latest_lifecycle_epoch: 2,
        active: true,
        send: send.clone(),
        receive: receive.clone(),
        epochs: vec![LanEpochDataSnapshot {
            lifecycle_epoch: 2,
            active: true,
            send,
            receive,
        }],
    }
}

fn lan_direction(queued: u64, resolved_success: u64, dropped: u64) -> LanDirectionSnapshot {
    LanDirectionSnapshot {
        queued,
        resolved_success,
        dropped,
        current_queue_depth: usize::try_from(queued.saturating_sub(resolved_success))
            .unwrap_or(usize::MAX),
        max_queue_depth: usize::try_from(queued).unwrap_or(usize::MAX),
        rejections: if dropped == 0 {
            Vec::new()
        } else {
            vec![LanRejectionSnapshot {
                stage: LanDataStage::OutboundQueue,
                reason: LanDataDropReason::QueueFull,
                count: dropped,
            }]
        },
    }
}
