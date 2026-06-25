use std::{
    fmt::{self, Display},
    time::{Duration, Instant},
};

use crate::state::{PeerId, PeerTransport};

pub(crate) const MISSING_TARGET_INITIAL_LOGS: u32 = 10;
pub(crate) const MISSING_TARGET_LOG_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RoomPeerSnapshot {
    pub(crate) peer_id: PeerId,
    pub(crate) steam_id64: String,
    pub(crate) transport: PeerTransport,
}

impl Display for RoomPeerSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}",
            self.peer_id, self.steam_id64, self.transport
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MissingTargetIncident {
    pub(crate) peers: Vec<RoomPeerSnapshot>,
}

impl MissingTargetIncident {
    pub(crate) fn peer_count(&self) -> usize {
        self.peers.len()
    }

    pub(crate) fn tcp_peer_count(&self) -> usize {
        self.peers
            .iter()
            .filter(|peer| peer.transport == PeerTransport::Tcp)
            .count()
    }

    pub(crate) fn udp_peer_count(&self) -> usize {
        self.peer_count().saturating_sub(self.tcp_peer_count())
    }

    pub(crate) fn peer_summary(&self) -> String {
        self.peers
            .iter()
            .map(RoomPeerSnapshot::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[derive(Debug, Default)]
pub(crate) struct MissingTargetLogBudget {
    logged: u32,
    last_logged: Option<Instant>,
}

impl MissingTargetLogBudget {
    pub(crate) fn should_log(&mut self, now: Instant) -> bool {
        let should_log = self.logged < MISSING_TARGET_INITIAL_LOGS
            || self.last_logged.is_none_or(|last_logged| {
                now.duration_since(last_logged) >= MISSING_TARGET_LOG_INTERVAL
            });
        if should_log {
            self.logged = self.logged.saturating_add(1);
            self.last_logged = Some(now);
        }
        should_log
    }
}
