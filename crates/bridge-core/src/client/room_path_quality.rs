use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::protocol::{CAP_ROOM_PATH_PROBE, PeerPresence, PeerPresenceInfo};

const COMPLETED_CAPACITY: usize = 30;
const MIN_COMPLETED_SAMPLES: usize = 5;
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const STALE_AFTER: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub enum RoomPathQualityState {
    #[default]
    Unavailable,
    Measuring,
    Current,
    Stale,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RoomPathQualitySnapshot {
    pub steam_id64: u64,
    pub state: RoomPathQualityState,
    pub completed: u32,
    pub responses: u32,
    pub median_rtt: Option<Duration>,
    pub p95_rtt: Option<Duration>,
    pub jitter: Option<Duration>,
    pub loss_basis_points: Option<u16>,
    pub freshness: Option<Duration>,
}

#[derive(Clone, Copy, Debug)]
enum CompletedProbe {
    Response(Duration),
    Timeout,
}

#[derive(Debug, Default)]
struct PeerMeasurement {
    pending: HashMap<u64, Instant>,
    completed: VecDeque<CompletedProbe>,
    last_completed: Option<Instant>,
}

#[derive(Debug, Default)]
pub(super) struct RoomPathQuality {
    peers: HashMap<u64, PeerMeasurement>,
}

impl RoomPathQuality {
    pub(super) fn sync_peers(&mut self, peers: &[PeerPresenceInfo], local_steam_id64: u64) {
        self.peers.retain(|steam_id64, _| {
            peers.iter().any(|peer| {
                peer.steam_id64 == *steam_id64
                    && peer.steam_id64 != local_steam_id64
                    && peer.presence == PeerPresence::Connected
                    && peer.capabilities & CAP_ROOM_PATH_PROBE != 0
            })
        });
        for peer in peers {
            if peer.steam_id64 != local_steam_id64
                && peer.presence == PeerPresence::Connected
                && peer.capabilities & CAP_ROOM_PATH_PROBE != 0
            {
                self.peers.entry(peer.steam_id64).or_default();
            }
        }
    }

    pub(super) fn targets(&self) -> impl Iterator<Item = u64> + '_ {
        self.peers.keys().copied()
    }

    pub(super) fn record_sent(&mut self, target: u64, probe_id: u64, now: Instant) {
        if let Some(peer) = self.peers.get_mut(&target) {
            peer.pending.insert(probe_id, now);
        }
    }

    pub(super) fn record_echo(&mut self, source: u64, probe_id: u64, now: Instant) -> bool {
        let Some(peer) = self.peers.get_mut(&source) else {
            return false;
        };
        let Some(sent_at) = peer.pending.remove(&probe_id) else {
            return false;
        };
        peer.complete(CompletedProbe::Response(now.duration_since(sent_at)), now);
        true
    }

    pub(super) fn expire(&mut self, now: Instant) {
        for peer in self.peers.values_mut() {
            let expired = peer
                .pending
                .iter()
                .filter_map(|(id, sent_at)| {
                    (now.duration_since(*sent_at) >= PROBE_TIMEOUT).then_some(*id)
                })
                .collect::<Vec<_>>();
            for id in expired {
                peer.pending.remove(&id);
                peer.complete(CompletedProbe::Timeout, now);
            }
        }
    }

    pub(super) fn snapshots(&self, now: Instant) -> Vec<RoomPathQualitySnapshot> {
        let mut snapshots = self
            .peers
            .iter()
            .map(|(steam_id64, peer)| peer.snapshot(*steam_id64, now))
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| snapshot.steam_id64);
        snapshots
    }

    pub(super) fn clear(&mut self) {
        self.peers.clear();
    }
}

impl PeerMeasurement {
    fn complete(&mut self, result: CompletedProbe, now: Instant) {
        self.completed.push_back(result);
        while self.completed.len() > COMPLETED_CAPACITY {
            self.completed.pop_front();
        }
        self.last_completed = Some(now);
    }

    fn snapshot(&self, steam_id64: u64, now: Instant) -> RoomPathQualitySnapshot {
        let completed = self.completed.len();
        let responses = self
            .completed
            .iter()
            .filter(|sample| matches!(sample, CompletedProbe::Response(_)))
            .count();
        let freshness = self.last_completed.map(|at| now.duration_since(at));
        let state = if completed < MIN_COMPLETED_SAMPLES {
            RoomPathQualityState::Measuring
        } else if freshness.is_some_and(|age| age > STALE_AFTER) {
            RoomPathQualityState::Stale
        } else {
            RoomPathQualityState::Current
        };
        let mut rtts = self
            .completed
            .iter()
            .filter_map(|sample| match sample {
                CompletedProbe::Response(rtt) => Some(*rtt),
                CompletedProbe::Timeout => None,
            })
            .collect::<Vec<_>>();
        rtts.sort_unstable();
        let loss_basis_points = (completed >= MIN_COMPLETED_SAMPLES).then(|| {
            let timeouts = completed.saturating_sub(responses);
            u16::try_from(timeouts.saturating_mul(10_000) / completed).unwrap_or(10_000)
        });
        RoomPathQualitySnapshot {
            steam_id64,
            state,
            completed: u32::try_from(completed).unwrap_or(u32::MAX),
            responses: u32::try_from(responses).unwrap_or(u32::MAX),
            median_rtt: percentile(&rtts, 50),
            p95_rtt: percentile(&rtts, 95),
            jitter: window_jitter(&self.completed),
            loss_basis_points,
            freshness,
        }
    }
}

fn window_jitter(samples: &VecDeque<CompletedProbe>) -> Option<Duration> {
    let mut responses = samples.iter().filter_map(|sample| match sample {
        CompletedProbe::Response(rtt) => Some(*rtt),
        CompletedProbe::Timeout => None,
    });
    let mut previous = responses.next()?;
    let mut total_micros = 0_u128;
    let mut deltas = 0_u128;
    for current in responses {
        total_micros = total_micros.saturating_add(current.abs_diff(previous).as_micros());
        deltas = deltas.saturating_add(1);
        previous = current;
    }
    (deltas > 0)
        .then(|| Duration::from_micros(u64::try_from(total_micros / deltas).unwrap_or(u64::MAX)))
}

fn percentile(sorted: &[Duration], percentile: usize) -> Option<Duration> {
    if sorted.is_empty() {
        return None;
    }
    let numerator = sorted.len().saturating_sub(1).saturating_mul(percentile);
    sorted.get((numerator + 50) / 100).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(steam_id64: u64) -> PeerPresenceInfo {
        PeerPresenceInfo {
            steam_id64,
            display_name: None,
            presence: PeerPresence::Connected,
            capabilities: CAP_ROOM_PATH_PROBE,
        }
    }

    #[test]
    fn publishes_bounded_window_median_jitter_and_loss() {
        let start = Instant::now();
        let mut quality = RoomPathQuality::default();
        quality.sync_peers(&[peer(1), peer(2)], 1);
        for id in 1..=35 {
            let sent = start + Duration::from_secs(id);
            quality.record_sent(2, id, sent);
            if id % 5 == 0 {
                quality.expire(sent + PROBE_TIMEOUT);
            } else {
                let rtt = Duration::from_millis(20 + id);
                assert!(quality.record_echo(2, id, sent + rtt));
            }
        }
        let snapshot = quality.snapshots(start + Duration::from_secs(40))[0];
        assert_eq!(snapshot.completed, 30);
        assert_eq!(snapshot.responses, 24);
        assert_eq!(snapshot.loss_basis_points, Some(2_000));
        assert!(snapshot.median_rtt.is_some());
        assert!(snapshot.p95_rtt >= snapshot.median_rtt);
        assert!(snapshot.jitter.is_some());
    }

    #[test]
    fn measuring_becomes_stale_and_peer_removal_clears_state() {
        let start = Instant::now();
        let mut quality = RoomPathQuality::default();
        quality.sync_peers(&[peer(1), peer(2)], 1);
        for id in 1..=5 {
            quality.record_sent(2, id, start);
            assert!(quality.record_echo(2, id, start + Duration::from_millis(id * 10)));
        }
        assert_eq!(
            quality.snapshots(start + Duration::from_secs(1))[0].state,
            RoomPathQualityState::Current
        );
        assert_eq!(
            quality.snapshots(start + Duration::from_secs(6))[0].state,
            RoomPathQualityState::Stale
        );
        quality.sync_peers(&[peer(1)], 1);
        assert!(quality.snapshots(start).is_empty());
    }

    #[test]
    fn jitter_uses_only_the_completed_sample_window() {
        let start = Instant::now();
        let mut quality = RoomPathQuality::default();
        quality.sync_peers(&[peer(1), peer(2)], 1);
        for id in 1..=31 {
            let sent = start + Duration::from_secs(id);
            quality.record_sent(2, id, sent);
            let rtt = if id == 1 {
                Duration::from_millis(500)
            } else {
                Duration::from_millis(20)
            };
            assert!(quality.record_echo(2, id, sent + rtt));
        }

        assert_eq!(
            quality.snapshots(start + Duration::from_secs(32))[0].jitter,
            Some(Duration::ZERO)
        );
    }
}
