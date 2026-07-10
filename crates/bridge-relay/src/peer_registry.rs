use crate::domain::PeerId;

#[derive(Debug)]
pub(crate) struct PeerRegistry {
    next_id: u64,
}

impl Default for PeerRegistry {
    fn default() -> Self {
        Self { next_id: 1 }
    }
}

impl PeerRegistry {
    pub(crate) fn allocate(&mut self) -> PeerId {
        let peer_id = PeerId::new(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        peer_id
    }
}
