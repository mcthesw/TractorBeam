use std::{collections::HashMap, net::SocketAddr};

use crate::state::PeerId;

#[derive(Debug)]
pub(crate) struct PeerRegistry {
    next_id: u64,
    udp_peers: HashMap<SocketAddr, PeerId>,
}

impl Default for PeerRegistry {
    fn default() -> Self {
        Self {
            next_id: 1,
            udp_peers: HashMap::new(),
        }
    }
}

impl PeerRegistry {
    pub(crate) fn allocate(&mut self) -> PeerId {
        let peer_id = PeerId::new(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        peer_id
    }

    pub(crate) fn udp_peer(&mut self, address: SocketAddr) -> PeerId {
        if let Some(peer_id) = self.udp_peers.get(&address) {
            return *peer_id;
        }
        let peer_id = self.allocate();
        self.udp_peers.insert(address, peer_id);
        peer_id
    }
}
