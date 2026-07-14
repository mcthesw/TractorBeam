use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::{Duration, Instant},
};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tractor_beam_direct_protocol::{
    ControlMessage, LinkId, MAX_PEERS, PeerDescriptor, PeerIdentity,
};

pub(super) const PEER_RECOVERY_HORIZON: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub(super) struct ActiveLink {
    pub key: LinkKey,
    pub descriptor: PeerDescriptor,
    pub sender: mpsc::Sender<ControlMessage>,
    pub cancellation: CancellationToken,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct LinkKey {
    pub link_id: LinkId,
    pub initiator: PeerIdentity,
}

pub(super) enum RegisterResult {
    Accepted { replaced: Option<ActiveLink> },
    DuplicateLost,
    DuplicateSteamIdentity,
    SelfConnection,
}

pub(super) struct Membership {
    local: PeerDescriptor,
    hints: BTreeMap<PeerIdentity, PeerDescriptor>,
    active: HashMap<PeerIdentity, ActiveLink>,
    dialing: HashSet<PeerIdentity>,
    recovery_deadlines: HashMap<PeerIdentity, Instant>,
}

impl Membership {
    pub fn new(local: PeerDescriptor) -> Self {
        Self {
            local,
            hints: BTreeMap::new(),
            active: HashMap::new(),
            dialing: HashSet::new(),
            recovery_deadlines: HashMap::new(),
        }
    }

    pub fn local(&self) -> &PeerDescriptor {
        &self.local
    }

    pub fn snapshot(&self) -> Vec<PeerDescriptor> {
        let mut peers = BTreeMap::new();
        peers.insert(self.local.identity, self.local.clone());
        for link in self.active.values() {
            peers.insert(link.descriptor.identity, link.descriptor.clone());
        }
        for (identity, descriptor) in &self.hints {
            peers.entry(*identity).or_insert_with(|| descriptor.clone());
        }
        peers.into_values().take(MAX_PEERS).collect()
    }

    pub fn merge_hints(&mut self, peers: Vec<PeerDescriptor>) -> Vec<PeerDescriptor> {
        let mut discovered = Vec::new();
        for peer in peers {
            if peer.identity == self.local.identity
                || peer.identity.steam_id64 == self.local.identity.steam_id64
                || self.active.contains_key(&peer.identity)
                || self.active.values().any(|link| {
                    link.descriptor.identity.steam_id64 == peer.identity.steam_id64
                        && link.descriptor.identity != peer.identity
                })
            {
                continue;
            }
            let is_new = !self.hints.contains_key(&peer.identity);
            self.hints.insert(peer.identity, peer.clone());
            if is_new {
                discovered.push(peer);
            }
        }
        trim_hints(&mut self.hints, &self.active);
        discovered
    }

    pub fn begin_dial(&mut self, identity: PeerIdentity) -> bool {
        if identity == self.local.identity || self.active.contains_key(&identity) {
            return false;
        }
        self.dialing.insert(identity)
    }

    pub fn end_dial(&mut self, identity: PeerIdentity, failed: bool) {
        self.dialing.remove(&identity);
        if failed {
            self.recovery_deadlines
                .entry(identity)
                .or_insert_with(|| Instant::now() + PEER_RECOVERY_HORIZON);
        }
    }

    pub fn register(&mut self, link: ActiveLink) -> RegisterResult {
        let identity = link.descriptor.identity;
        if identity == self.local.identity || identity.steam_id64 == self.local.identity.steam_id64
        {
            return RegisterResult::SelfConnection;
        }
        if self.active.values().any(|existing| {
            existing.descriptor.identity.steam_id64 == identity.steam_id64
                && existing.descriptor.identity != identity
        }) {
            return RegisterResult::DuplicateSteamIdentity;
        }
        let replaced = if let Some(existing) = self.active.get(&identity) {
            if existing.key <= link.key {
                return RegisterResult::DuplicateLost;
            }
            self.active.remove(&identity)
        } else {
            None
        };
        self.hints.insert(identity, link.descriptor.clone());
        self.hints.retain(|hint_identity, _| {
            hint_identity.steam_id64 != identity.steam_id64 || *hint_identity == identity
        });
        self.recovery_deadlines.remove(&identity);
        self.dialing.remove(&identity);
        self.active.insert(identity, link);
        RegisterResult::Accepted { replaced }
    }

    pub fn remove_link(&mut self, identity: PeerIdentity, key: LinkKey, graceful: bool) -> bool {
        if self
            .active
            .get(&identity)
            .is_none_or(|link| link.key != key)
        {
            return false;
        }
        self.active.remove(&identity);
        if graceful {
            self.hints.remove(&identity);
            self.recovery_deadlines.remove(&identity);
        } else {
            self.recovery_deadlines
                .insert(identity, Instant::now() + PEER_RECOVERY_HORIZON);
        }
        true
    }

    pub fn expire(&mut self, now: Instant) {
        let expired = self
            .recovery_deadlines
            .iter()
            .filter_map(|(identity, deadline)| (*deadline <= now).then_some(*identity))
            .collect::<Vec<_>>();
        for identity in expired {
            self.recovery_deadlines.remove(&identity);
            self.hints.remove(&identity);
            self.dialing.remove(&identity);
        }
    }

    pub fn retry_candidates(&self) -> Vec<PeerDescriptor> {
        self.hints
            .values()
            .filter(|peer| {
                !self.active.contains_key(&peer.identity)
                    && !self.dialing.contains(&peer.identity)
                    && self
                        .recovery_deadlines
                        .get(&peer.identity)
                        .is_none_or(|deadline| *deadline > Instant::now())
            })
            .cloned()
            .collect()
    }

    pub fn links(&self) -> Vec<ActiveLink> {
        self.active.values().cloned().collect()
    }

    pub fn connected_descriptors(&self) -> Vec<PeerDescriptor> {
        let mut peers = self
            .active
            .values()
            .map(|link| link.descriptor.clone())
            .collect::<Vec<_>>();
        peers.sort_by_key(|peer| peer.identity);
        peers
    }

    pub fn hinted_descriptors(&self) -> Vec<PeerDescriptor> {
        self.hints
            .values()
            .filter(|peer| !self.active.contains_key(&peer.identity))
            .cloned()
            .collect()
    }

    pub fn is_recovering(&self, identity: PeerIdentity) -> bool {
        self.recovery_deadlines.contains_key(&identity)
    }
}

fn trim_hints(
    hints: &mut BTreeMap<PeerIdentity, PeerDescriptor>,
    active: &HashMap<PeerIdentity, ActiveLink>,
) {
    while hints.len().saturating_add(1) > MAX_PEERS {
        let removable = hints
            .keys()
            .rev()
            .find(|identity| !active.contains_key(identity))
            .copied();
        let Some(identity) = removable else {
            break;
        };
        hints.remove(&identity);
    }
}

#[cfg(test)]
mod tests {
    use tractor_beam_direct_protocol::{HostCandidate, InstanceId, KNOWN_CAPABILITIES};

    use super::*;

    fn descriptor(id: u8, instance: u8) -> PeerDescriptor {
        PeerDescriptor {
            identity: PeerIdentity::new(u64::from(id), InstanceId::from_bytes([instance; 16])),
            display_name: Some(format!("Peer {id}")),
            control_candidates: vec![
                HostCandidate::new(
                    format!("127.0.0.1:{}", 20_000 + u16::from(id))
                        .parse()
                        .unwrap(),
                    1,
                    0,
                )
                .unwrap(),
            ],
            capabilities: KNOWN_CAPABILITIES,
        }
    }

    #[test]
    fn snapshots_are_idempotent_hints_and_cannot_replace_live_instance() {
        let mut membership = Membership::new(descriptor(1, 1));
        let hinted = descriptor(2, 2);
        assert_eq!(membership.merge_hints(vec![hinted.clone()]).len(), 1);
        assert!(membership.merge_hints(vec![hinted]).is_empty());

        let (sender, _) = mpsc::channel(8);
        let live = ActiveLink {
            key: LinkKey {
                link_id: LinkId::from_bytes([1; 16]),
                initiator: descriptor(1, 1).identity,
            },
            descriptor: descriptor(2, 2),
            sender,
            cancellation: CancellationToken::new(),
        };
        assert!(matches!(
            membership.register(live),
            RegisterResult::Accepted { .. }
        ));
        membership.merge_hints(vec![descriptor(2, 3)]);
        assert_eq!(
            membership.connected_descriptors()[0].identity.instance_id,
            InstanceId::from_bytes([2; 16])
        );
    }

    #[test]
    fn deterministic_link_key_has_one_winner() {
        let low = LinkKey {
            link_id: LinkId::from_bytes([1; 16]),
            initiator: descriptor(1, 1).identity,
        };
        let high = LinkKey {
            link_id: LinkId::from_bytes([2; 16]),
            initiator: descriptor(2, 2).identity,
        };
        assert!(low < high);
    }

    #[test]
    fn restart_replaces_disconnected_instance_and_grace_expiry_removes_stale_hint() {
        let mut membership = Membership::new(descriptor(1, 1));
        let old = descriptor(2, 2);
        let (sender, _) = mpsc::channel(8);
        let key = LinkKey {
            link_id: LinkId::from_bytes([1; 16]),
            initiator: descriptor(1, 1).identity,
        };
        membership.register(ActiveLink {
            key,
            descriptor: old.clone(),
            sender,
            cancellation: CancellationToken::new(),
        });
        assert!(membership.remove_link(old.identity, key, false));

        let restarted = descriptor(2, 3);
        let (sender, _) = mpsc::channel(8);
        assert!(matches!(
            membership.register(ActiveLink {
                key: LinkKey {
                    link_id: LinkId::from_bytes([2; 16]),
                    initiator: descriptor(1, 1).identity,
                },
                descriptor: restarted.clone(),
                sender,
                cancellation: CancellationToken::new(),
            }),
            RegisterResult::Accepted { .. }
        ));
        assert_eq!(membership.snapshot().len(), 2);

        let active = membership.links().pop().unwrap();
        membership.remove_link(restarted.identity, active.key, false);
        membership.expire(Instant::now() + PEER_RECOVERY_HORIZON + Duration::from_secs(1));
        assert_eq!(membership.snapshot(), vec![descriptor(1, 1)]);
    }
}
