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
fn queue_and_sequence_windows_use_deltas() {
    let previous = QualityBaseline {
        queue_drops: 2,
        sequence_gaps: 3,
        sequence_reordered: 4,
        ..QualityBaseline::default()
    };
    let current = QualityBaseline {
        queue_drops: 2,
        sequence_gaps: 4,
        sequence_reordered: 9,
        ..QualityBaseline::default()
    };
    let window = current.delta(previous, Duration::from_secs(5));
    assert_eq!(window.queue_drops, 0);
    assert_eq!(window.sequence_gaps, 1);
    assert_eq!(window.sequence_reordered, 5);
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
            sequence_gaps: 1,
            runtime_rtt_timeouts: 1,
            ..active_window()
        },
    );
    assert_eq!(watch.quality, SessionQuality::Watch);
    assert_eq!(
        watch.reasons,
        [
            SessionQualityReason::SequenceGap,
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
    health.observe_relay_recv(1, startup);
    health.observe_outbound_enqueue(false);
    assert_eq!(
        health.snapshot(startup).quality,
        SessionQuality::Unavailable
    );

    let active = start + Duration::from_secs(ACTIVE_TRAFFIC_STARTUP_GRACE_SECONDS);
    health.observe_hook_in_recv(1, active);
    health.observe_relay_recv(1, active);
    let recovered = health.snapshot(active);
    assert_eq!(recovered.queues.total_dropped(), 1);
    assert_eq!(recovered.window.queue_drops, 0);
    assert_eq!(recovered.quality, SessionQuality::Good);
}

const fn active_window() -> SessionHealthWindow {
    SessionHealthWindow {
        duration_seconds: 5,
        hook_in_packets: 20,
        relay_recv_packets: 20,
        queue_drops: 0,
        sequence_gaps: 0,
        sequence_reordered: 0,
        runtime_rtt_sent: 0,
        runtime_rtt_timeouts: 0,
        hook_send_over_500_ms: 0,
        hook_send_over_1000_ms: 0,
        relay_gap_over_500_ms: 0,
        relay_gap_over_1000_ms: 0,
    }
}
