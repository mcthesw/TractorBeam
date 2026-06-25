use std::{collections::HashMap, io, net::SocketAddr, time::Instant};

use basement_bridge_core::udp_fec::{UdpFecDecoder, UdpFecEncoder, UdpFecProfile};
use bytes::Bytes;
use tokio::sync::mpsc;

use crate::state::PeerId;

#[derive(Debug)]
pub(crate) enum PeerEgress {
    Udp {
        address: SocketAddr,
        fec: Option<Box<UdpFecPeer>>,
    },
    Tcp(mpsc::Sender<Bytes>),
}

#[derive(Debug)]
pub(crate) struct UdpFecPeer {
    encoder: UdpFecEncoder,
    decoder: UdpFecDecoder,
}

#[derive(Debug)]
pub(crate) enum PeerOutput {
    Udp {
        address: SocketAddr,
        frames: Vec<Bytes>,
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
            Some(PeerEgress::Udp {
                address: existing, ..
            }) => *existing = address,
            _ => {
                self.peers
                    .insert(peer_id, PeerEgress::Udp { address, fec: None });
            }
        }
    }

    pub(crate) fn insert_tcp(&mut self, peer_id: PeerId, sender: mpsc::Sender<Bytes>) {
        self.peers.insert(peer_id, PeerEgress::Tcp(sender));
    }

    pub(crate) fn enable_udp_fec(&mut self, peer_id: PeerId, profile: UdpFecProfile) {
        if let Some(PeerEgress::Udp { fec, .. }) = self.peers.get_mut(&peer_id) {
            *fec = Some(Box::new(UdpFecPeer {
                encoder: UdpFecEncoder::new(profile),
                decoder: UdpFecDecoder::new(profile),
            }));
        }
    }

    pub(crate) fn decode_udp(
        &mut self,
        peer_id: PeerId,
        raw: Bytes,
        now: Instant,
    ) -> io::Result<Vec<Bytes>> {
        let Some(PeerEgress::Udp { fec: Some(fec), .. }) = self.peers.get_mut(&peer_id) else {
            return Ok(vec![raw]);
        };
        fec.decoder.decode(raw, now).map_err(io::Error::other)
    }

    pub(crate) fn send_control(&mut self, peer_id: PeerId, raw: Bytes) -> io::Result<PeerOutput> {
        self.peer_output(peer_id, raw, None)
    }

    pub(crate) fn send_data(
        &mut self,
        peer_id: PeerId,
        raw: Bytes,
        now: Instant,
    ) -> io::Result<PeerOutput> {
        self.peer_output(peer_id, raw, Some(now))
    }

    pub(crate) fn flush_udp_fec(&mut self, now: Instant) -> io::Result<Vec<(SocketAddr, Bytes)>> {
        let mut outputs = Vec::new();
        for peer in self.peers.values_mut() {
            let PeerEgress::Udp {
                address,
                fec: Some(fec),
            } = peer
            else {
                continue;
            };
            fec.decoder.expire(now);
            for frame in fec.encoder.flush_expired(now).map_err(io::Error::other)? {
                outputs.push((*address, frame));
            }
        }
        Ok(outputs)
    }

    pub(crate) fn remove(&mut self, peer_id: PeerId) {
        self.peers.remove(&peer_id);
    }

    fn peer_output(
        &mut self,
        peer_id: PeerId,
        raw: Bytes,
        fec_now: Option<Instant>,
    ) -> io::Result<PeerOutput> {
        let Some(target) = self.peers.get_mut(&peer_id) else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                format!("missing egress for {peer_id}"),
            ));
        };
        match target {
            PeerEgress::Udp { address, fec } => {
                let frames = match (fec, fec_now) {
                    (Some(fec), Some(now)) => fec
                        .encoder
                        .encode_or_passthrough(raw, now)
                        .map_err(io::Error::other)?,
                    _ => vec![raw],
                };
                Ok(PeerOutput::Udp {
                    address: *address,
                    frames,
                })
            }
            PeerEgress::Tcp(sender) => Ok(PeerOutput::Tcp {
                sender: sender.clone(),
                frame: raw,
            }),
        }
    }
}
