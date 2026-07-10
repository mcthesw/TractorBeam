use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub(crate) struct RelayMetricsV2 {
    pub(crate) bootstrap_accepted: AtomicU64,
    pub(crate) bootstrap_rejected: AtomicU64,
    pub(crate) joins: AtomicU64,
    pub(crate) control_detached: AtomicU64,
    pub(crate) resumes_attempted: AtomicU64,
    pub(crate) resumes_succeeded: AtomicU64,
    pub(crate) resumes_rejected: AtomicU64,
    pub(crate) sessions_expired: AtomicU64,
    pub(crate) data_received: AtomicU64,
    pub(crate) data_forwarded: AtomicU64,
    pub(crate) data_duplicates: AtomicU64,
    pub(crate) data_rate_limited: AtomicU64,
    pub(crate) data_rejected: AtomicU64,
}

impl RelayMetricsV2 {
    pub(crate) fn increment(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn value(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

pub(crate) static METRICS: RelayMetricsV2 = RelayMetricsV2 {
    bootstrap_accepted: AtomicU64::new(0),
    bootstrap_rejected: AtomicU64::new(0),
    joins: AtomicU64::new(0),
    control_detached: AtomicU64::new(0),
    resumes_attempted: AtomicU64::new(0),
    resumes_succeeded: AtomicU64::new(0),
    resumes_rejected: AtomicU64::new(0),
    sessions_expired: AtomicU64::new(0),
    data_received: AtomicU64::new(0),
    data_forwarded: AtomicU64::new(0),
    data_duplicates: AtomicU64::new(0),
    data_rate_limited: AtomicU64::new(0),
    data_rejected: AtomicU64::new(0),
};
