use std::{collections::HashMap, io, net::SocketAddr};

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::state::PeerId;

#[derive(Debug)]
pub(crate) enum PeerEgress {
    Udp { address: SocketAddr },
    Tcp(mpsc::Sender<Bytes>),
}

#[derive(Debug)]
pub(crate) enum PeerOutput {
    Udp {
        address: SocketAddr,
        frame: Bytes,
    },
    Tcp {
        sender: mpsc::Sender<Bytes>,
        frame: Bytes,
    },
}

#[derive(Debug, Default)]
pub(crate) struct EgressTable {
    peers: HashMap<PeerId, PeerEgress>,
}

impl EgressTable {
    pub(crate) fn upsert_udp(&mut self, peer_id: PeerId, address: SocketAddr) {
        match self.peers.get_mut(&peer_id) {
            Some(PeerEgress::Udp { address: existing }) => *existing = address,
            _ => {
                self.peers.insert(peer_id, PeerEgress::Udp { address });
            }
        }
    }

    pub(crate) fn insert_tcp(&mut self, peer_id: PeerId, sender: mpsc::Sender<Bytes>) {
        self.peers.insert(peer_id, PeerEgress::Tcp(sender));
    }

    pub(crate) fn send_control(&mut self, peer_id: PeerId, raw: Bytes) -> io::Result<PeerOutput> {
        self.peer_output(peer_id, raw)
    }

    pub(crate) fn send_data(&mut self, peer_id: PeerId, raw: Bytes) -> io::Result<PeerOutput> {
        self.peer_output(peer_id, raw)
    }

    pub(crate) fn remove(&mut self, peer_id: PeerId) {
        self.peers.remove(&peer_id);
    }

    fn peer_output(&mut self, peer_id: PeerId, raw: Bytes) -> io::Result<PeerOutput> {
        let Some(target) = self.peers.get_mut(&peer_id) else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                format!("missing egress for {peer_id}"),
            ));
        };
        match target {
            PeerEgress::Udp { address } => Ok(PeerOutput::Udp {
                address: *address,
                frame: raw,
            }),
            PeerEgress::Tcp(sender) => Ok(PeerOutput::Tcp {
                sender: sender.clone(),
                frame: raw,
            }),
        }
    }
}
