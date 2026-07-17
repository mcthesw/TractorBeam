use std::time::{Duration, Instant};

use crate::{config::RelayConfig, domain::StateError};

const RATE_WINDOW: Duration = Duration::from_secs(1);
const PROBE_RATE_LIMIT_PER_SECOND: u32 = 10;

#[derive(Debug)]
pub(super) struct TrafficBudget {
    packet_limit: u32,
    byte_rate_limit: usize,
    byte_burst_limit: usize,
    window_started: Instant,
    packets: u32,
    bytes: usize,
    probe_window_started: Instant,
    probes: u32,
}

impl TrafficBudget {
    pub(super) fn new(config: &RelayConfig, now: Instant) -> Self {
        Self {
            packet_limit: config.rate_limit_per_second,
            byte_rate_limit: config.byte_rate_limit_per_second,
            byte_burst_limit: config.byte_rate_limit_burst,
            window_started: now,
            packets: 0,
            bytes: 0,
            probe_window_started: now,
            probes: 0,
        }
    }

    pub(super) fn check_traffic(
        &mut self,
        frame_bytes: usize,
        now: Instant,
    ) -> Result<(), StateError> {
        if now.duration_since(self.window_started) >= RATE_WINDOW {
            self.window_started = now;
            self.packets = 0;
            self.bytes = 0;
        }

        let next_packets = self.packets.saturating_add(1);
        let next_bytes = self.bytes.saturating_add(frame_bytes);
        if next_packets > self.packet_limit
            || next_bytes > self.byte_burst_limit
            || (self.bytes > 0 && next_bytes > self.byte_rate_limit)
        {
            return Err(StateError::RateLimited);
        }

        self.packets = next_packets;
        self.bytes = next_bytes;
        Ok(())
    }

    pub(super) fn check_probe(&mut self, now: Instant) -> Result<(), StateError> {
        if now.duration_since(self.probe_window_started) >= RATE_WINDOW {
            self.probe_window_started = now;
            self.probes = 0;
        }

        let next = self.probes.saturating_add(1);
        if next > PROBE_RATE_LIMIT_PER_SECOND {
            return Err(StateError::ProbeRateLimited);
        }
        self.probes = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_traffic_does_not_debit_either_budget() {
        let now = Instant::now();
        let config = RelayConfig {
            rate_limit_per_second: 2,
            byte_rate_limit_per_second: 100,
            byte_rate_limit_burst: 100,
            ..RelayConfig::default()
        };
        let mut budget = TrafficBudget::new(&config, now);

        assert_eq!(budget.check_traffic(101, now), Err(StateError::RateLimited));
        assert_eq!((budget.packets, budget.bytes), (0, 0));

        assert_eq!(budget.check_traffic(50, now), Ok(()));
        assert_eq!(budget.check_traffic(50, now), Ok(()));
        assert_eq!(budget.check_traffic(1, now), Err(StateError::RateLimited));
        assert_eq!((budget.packets, budget.bytes), (2, 100));
    }

    #[test]
    fn traffic_limits_reset_at_the_next_window() {
        let now = Instant::now();
        let config = RelayConfig {
            rate_limit_per_second: 1,
            byte_rate_limit_per_second: 100,
            byte_rate_limit_burst: 100,
            ..RelayConfig::default()
        };
        let mut budget = TrafficBudget::new(&config, now);

        assert_eq!(budget.check_traffic(100, now), Ok(()));
        assert_eq!(budget.check_traffic(1, now), Err(StateError::RateLimited));
        assert_eq!(budget.check_traffic(100, now + RATE_WINDOW), Ok(()));
    }

    #[test]
    fn probe_limit_is_independent_from_gameplay_budget() {
        let now = Instant::now();
        let config = RelayConfig::default();
        let mut budget = TrafficBudget::new(&config, now);

        assert_eq!(budget.check_traffic(128, now), Ok(()));
        for _ in 0..PROBE_RATE_LIMIT_PER_SECOND {
            assert_eq!(budget.check_probe(now), Ok(()));
        }
        assert_eq!(budget.check_probe(now), Err(StateError::ProbeRateLimited));
        assert_eq!(budget.check_probe(now + RATE_WINDOW), Ok(()));
    }
}
