use std::{collections::HashMap, io, net::SocketAddr, sync::Arc};

use bytes::Bytes;
use tokio::{
    net::UdpSocket,
    sync::{Mutex, mpsc},
};

use crate::domain::PeerId;

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

pub(crate) async fn send_control_frame(
    udp_socket: Option<Arc<UdpSocket>>,
    egress: Arc<Mutex<EgressTable>>,
    peer_id: PeerId,
    raw: Bytes,
) -> io::Result<()> {
    let output = egress.lock().await.send_control(peer_id, raw)?;
    send_peer_output(udp_socket, peer_id, output).await
}

pub(crate) fn tcp_egress_channel(capacity: usize) -> (mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
    mpsc::channel(capacity)
}

pub(crate) async fn send_data_frame(
    udp_socket: Option<Arc<UdpSocket>>,
    egress: Arc<Mutex<EgressTable>>,
    peer_id: PeerId,
    raw: Bytes,
) -> io::Result<()> {
    let output = egress.lock().await.send_data(peer_id, raw)?;
    send_peer_output(udp_socket, peer_id, output).await
}

async fn send_peer_output(
    udp_socket: Option<Arc<UdpSocket>>,
    peer_id: PeerId,
    output: PeerOutput,
) -> io::Result<()> {
    match output {
        PeerOutput::Udp { address, frame } => {
            let Some(socket) = udp_socket else {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    format!("UDP listener is disabled for {peer_id}"),
                ));
            };
            socket.send_to(&frame, address).await?;
            Ok(())
        }
        PeerOutput::Tcp { sender, frame } => sender.try_send(frame).map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("TCP egress queue is full for {peer_id}"),
            )
        }),
    }
}
