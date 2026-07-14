use std::{
    io,
    time::{Duration, Instant},
};

use super::{SharedMetrics, SharedState, SharedTcpEgress, data::send_presence};
use tokio::time;

pub(super) async fn cleanup_task(
    state: SharedState,
    egress: SharedTcpEgress,
    metrics: SharedMetrics,
) -> io::Result<()> {
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let broadcasts = state.lock().await.cleanup(Instant::now());
        for _ in &broadcasts {
            metrics.record_control("session_expire", "accepted");
            tracing::info!("detached session expired");
        }
        for broadcast in broadcasts {
            send_presence(&egress, &metrics, broadcast).await;
        }
    }
}

pub(super) async fn metrics_task(
    state: SharedState,
    egress: SharedTcpEgress,
    queue_capacity: usize,
    metrics: SharedMetrics,
) -> io::Result<()> {
    let mut interval = time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        let state = state.lock().await;
        let (rooms, _) = state.active_counts();
        let peer_counts = state.active_peer_counts();
        drop(state);
        let max_queue_utilization = egress
            .lock()
            .await
            .values()
            .map(|sender| 1.0 - sender.capacity() as f64 / queue_capacity as f64)
            .fold(0.0_f64, f64::max);
        metrics.record_snapshot(rooms, peer_counts, max_queue_utilization);
    }
}
