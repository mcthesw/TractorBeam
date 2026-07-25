use super::*;

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
        queue_drops: 2,
        delivery_gaps: 3,
        delivery_reordered: 4,
        ..QualityBaseline::default()
    };
    let current = QualityBaseline {
        queue_drops: 2,
        delivery_gaps: 4,
        delivery_reordered: 9,
        ..QualityBaseline::default()
    };
    let window = current.delta(previous, Duration::from_secs(5));
    assert_eq!(window.queue_drops, 0);
    assert_eq!(window.delivery_gaps, 1);
    assert_eq!(window.delivery_reordered, 5);
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
