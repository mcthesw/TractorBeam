use std::{collections::BTreeMap, sync::Arc, time::Instant};

use tokio::sync::mpsc;
use tractor_beam_direct_protocol::{
    ControlMessage, PathId, PathToken, PeerDescriptor, PeerIdentity,
};

use super::{
    PathManager, PathMaterial, PeerPath,
    candidate::{nonzero_random, path_offer},
    data,
    data_plane::{
        LanDataDirection, LanDataDropReason, LanDataStage, LanInboundDelivery,
        PER_PEER_PACKET_QUEUE_CAPACITY, PeerPathDataPlane,
    },
};
use crate::client::lan::membership::PeerLifecycleEpoch;

impl PathManager {
    pub fn peer_connected(
        self: &Arc<Self>,
        epoch: PeerLifecycleEpoch,
        descriptor: PeerDescriptor,
        control: mpsc::Sender<ControlMessage>,
    ) {
        let remote = descriptor.identity;
        let mut offer = None;
        let (outbound, outbound_rx) = mpsc::channel(PER_PEER_PACKET_QUEUE_CAPACITY);
        let replaced;
        let data_key;
        {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            if state
                .latest_epochs
                .get(&remote)
                .is_some_and(|latest| *latest >= epoch)
            {
                return;
            }
            state.transactions.retain(|_, check| check.peer != remote);
            replaced = state.peers.remove(&remote);
            data_key = self.ledger.activate(remote, epoch);
            let path = PeerPath {
                identity: remote,
                control: control.clone(),
                material: None,
                remote_candidates: Vec::new(),
                checks: BTreeMap::new(),
                nominated: None,
                pending_nomination: None,
                next_heartbeat_id: 1,
                checking_since: Instant::now(),
                next_frame_id: 1,
                last_received_frame_id: 0,
                data: PeerPathDataPlane::new(data_key, &self.cancellation, outbound),
            };
            state.peers.insert(remote, path);
            state.latest_epochs.insert(remote, epoch);
            state.latest_data_keys.insert(remote.steam_id64, data_key);
            if !state.peer_order.contains(&remote) {
                state.peer_order.push(remote);
            }
            if self.local < remote {
                let material = PathMaterial {
                    id: PathId::from_bytes(nonzero_random()),
                    token: PathToken::from_bytes(nonzero_random()),
                };
                if let Some(path) = state.peers.get_mut(&remote) {
                    path.material = Some(material);
                }
                offer = Some(path_offer(self.local, material, &self.candidates));
            }
        }
        if let Some(replaced) = replaced {
            self.close_peer_data(replaced, LanDataDropReason::GenerationClosed);
        }
        let worker = tokio::spawn(data::run_outbound_worker(
            self.clone(),
            data_key,
            outbound_rx,
        ));
        let mut workers = self
            .data_workers
            .lock()
            .expect("LAN data worker lock poisoned");
        workers.retain(|worker| !worker.is_finished());
        workers.push(worker);
        drop(workers);
        if let Some(offer) = offer {
            let _ = control.try_send(offer);
        }
    }

    pub fn peer_disconnected(&self, peer: PeerIdentity, epoch: PeerLifecycleEpoch) {
        let removed = {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            if state
                .latest_epochs
                .get(&peer)
                .is_some_and(|latest| *latest > epoch)
            {
                return;
            }
            state.latest_epochs.insert(peer, epoch);
            if state
                .peers
                .get(&peer)
                .is_none_or(|path| path.data.key.epoch != epoch)
            {
                return;
            }
            state.transactions.retain(|_, check| check.peer != peer);
            state.peer_order.retain(|identity| *identity != peer);
            state.peers.remove(&peer)
        };
        if let Some(path) = removed {
            self.close_peer_data(path, LanDataDropReason::GenerationClosed);
        }
    }

    fn close_peer_data(&self, mut path: PeerPath, reason: LanDataDropReason) {
        path.data.gate.close();
        let dropped = u64::try_from(path.data.inbound.len()).unwrap_or(u64::MAX);
        path.data.inbound.clear();
        self.ledger.record_dropped_batch(
            path.data.key,
            LanDataDirection::Receive,
            LanDataStage::InboundQueue,
            reason,
            dropped,
            0,
        );
        self.ledger.close_epoch(path.data.key, Instant::now());
        self.inbound_notify.notify_waiters();
    }

    pub(in crate::client::lan) fn clear_inbound(&self) {
        let dropped = {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            state
                .peers
                .values_mut()
                .filter_map(|path| {
                    let count = u64::try_from(path.data.inbound.len()).unwrap_or(u64::MAX);
                    path.data.inbound.clear();
                    (count > 0).then_some((path.data.key, count))
                })
                .collect::<Vec<_>>()
        };
        for (key, count) in dropped {
            self.ledger.record_dropped_batch(
                key,
                LanDataDirection::Receive,
                LanDataStage::InboundQueue,
                LanDataDropReason::SessionClosed,
                count,
                0,
            );
        }
    }

    pub(super) fn pop_next_inbound(&self, cursor: &mut usize) -> Option<LanInboundDelivery> {
        let mut state = self.inner.lock().expect("LAN path lock poisoned");
        let peer_count = state.peer_order.len();
        if peer_count == 0 {
            *cursor = 0;
            return None;
        }
        *cursor %= peer_count;
        for offset in 0..peer_count {
            let index = (*cursor + offset) % peer_count;
            let identity = state.peer_order[index];
            let Some(path) = state.peers.get_mut(&identity) else {
                continue;
            };
            let Some(packet) = path.data.inbound.pop_front() else {
                continue;
            };
            *cursor = (index + 1) % peer_count;
            let key = path.data.key;
            let gate = path.data.gate.clone();
            self.ledger
                .set_queue_depth(key, LanDataDirection::Receive, path.data.inbound.len());
            return Some(LanInboundDelivery {
                packet,
                receipt: super::data_plane::LanInboundReceipt::new(key, gate, self.ledger.clone()),
            });
        }
        None
    }

    pub(in crate::client::lan) async fn stop_data_workers(&self) {
        let workers = {
            let mut workers = self
                .data_workers
                .lock()
                .expect("LAN data worker lock poisoned");
            std::mem::take(&mut *workers)
        };
        for worker in workers {
            let _ = worker.await;
        }
        self.ledger.finish_all(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use tokio_util::sync::CancellationToken;
    use tractor_beam_direct_protocol::{InstanceId, PeerDescriptor};

    use super::*;
    use crate::client::packet_flow::{DeliveryStreamId, InboundGamePacket, OutboundGamePacket};

    fn identity(id: u8) -> PeerIdentity {
        PeerIdentity::new(u64::from(id), InstanceId::from_bytes([id; 16]))
    }

    fn descriptor(id: u8) -> PeerDescriptor {
        PeerDescriptor {
            identity: identity(id),
            display_name: Some(format!("Peer {id}")),
            control_candidates: Vec::new(),
            capabilities: 0,
        }
    }

    fn outbound(target: u8, sequence: u32) -> OutboundGamePacket {
        OutboundGamePacket {
            to_steam_id64: u64::from(target),
            hook_sequence: sequence,
            delivery_stream_id: DeliveryStreamId::from_bytes([target; 16]),
            delivery_sequence: u64::from(sequence),
            channel: 0,
            send_type: 0,
            payload: Bytes::from_static(b"test"),
        }
    }

    fn inbound(source: u8, sequence: u32) -> InboundGamePacket {
        InboundGamePacket {
            from_steam_id64: u64::from(source),
            delivery_stream_id: DeliveryStreamId::from_bytes([source; 16]),
            delivery_sequence: u64::from(sequence),
            channel: 0,
            send_type: 0,
            payload: Bytes::from_static(b"test"),
        }
    }

    fn install_test_peer(
        manager: &Arc<PathManager>,
        peer: PeerIdentity,
        epoch: PeerLifecycleEpoch,
    ) -> tokio::sync::mpsc::Receiver<OutboundGamePacket> {
        let key = manager.ledger.activate(peer, epoch);
        let (outbound, receiver) =
            mpsc::channel(super::super::data_plane::PER_PEER_PACKET_QUEUE_CAPACITY);
        let (control, _) = mpsc::channel(1);
        let path = PeerPath {
            identity: peer,
            control,
            material: None,
            remote_candidates: Vec::new(),
            checks: BTreeMap::new(),
            nominated: None,
            pending_nomination: None,
            next_heartbeat_id: 1,
            checking_since: Instant::now(),
            next_frame_id: 1,
            last_received_frame_id: 0,
            data: PeerPathDataPlane::new(key, &manager.cancellation, outbound),
        };
        let mut state = manager.inner.lock().unwrap();
        state.peers.insert(peer, path);
        state.peer_order.push(peer);
        state.latest_epochs.insert(peer, epoch);
        state.latest_data_keys.insert(peer.steam_id64, key);
        receiver
    }

    #[tokio::test]
    async fn stale_epoch_callbacks_cannot_replace_or_remove_the_current_path() {
        let cancellation = CancellationToken::new();
        let (manager, _, _) = PathManager::new(identity(1), Vec::new(), cancellation.clone())
            .await
            .unwrap();
        let (control, _) = mpsc::channel(8);
        let old = PeerLifecycleEpoch::test(1);
        let current = PeerLifecycleEpoch::test(2);

        manager.peer_connected(current, descriptor(2), control.clone());
        manager.peer_connected(old, descriptor(2), control);
        assert_eq!(
            manager.inner.lock().unwrap().peers[&identity(2)]
                .data
                .key
                .epoch,
            current
        );

        manager.peer_disconnected(identity(2), old);
        assert!(
            manager
                .inner
                .lock()
                .unwrap()
                .peers
                .contains_key(&identity(2))
        );
        manager.peer_disconnected(identity(2), current);
        assert!(
            !manager
                .inner
                .lock()
                .unwrap()
                .peers
                .contains_key(&identity(2))
        );

        cancellation.cancel();
        manager.stop_data_workers().await;
    }

    #[tokio::test]
    async fn full_peer_outbound_queue_does_not_consume_another_peers_capacity() {
        let cancellation = CancellationToken::new();
        let (manager, _, monitor) = PathManager::new(identity(1), Vec::new(), cancellation.clone())
            .await
            .unwrap();
        let _peer_b_receiver =
            install_test_peer(&manager, identity(2), PeerLifecycleEpoch::test(1));
        let _peer_c_receiver =
            install_test_peer(&manager, identity(3), PeerLifecycleEpoch::test(2));

        for sequence in 0..super::super::data_plane::PER_PEER_PACKET_QUEUE_CAPACITY {
            manager
                .try_send_game(outbound(2, u32::try_from(sequence).unwrap()))
                .unwrap();
        }
        assert!(matches!(
            manager.try_send_game(outbound(2, u32::MAX)),
            Err(super::super::LanGameSendError::QueueFull(2))
        ));
        manager.try_send_game(outbound(3, 1)).unwrap();

        let snapshot = monitor.snapshot();
        let peer_b = snapshot
            .peers
            .iter()
            .find(|peer| peer.peer_steam_id64 == 2)
            .unwrap();
        let peer_c = snapshot
            .peers
            .iter()
            .find(|peer| peer.peer_steam_id64 == 3)
            .unwrap();
        assert_eq!(peer_b.send.queued, 256);
        assert_eq!(peer_b.send.dropped, 1);
        assert_eq!(peer_c.send.queued, 1);
        assert_eq!(peer_c.send.dropped, 0);
        assert_eq!(snapshot.send.queued, 257);
        assert_eq!(snapshot.send.dropped, 1);

        cancellation.cancel();
    }

    #[tokio::test]
    async fn clear_inbound_discards_packets_between_gameplay_sessions() {
        let cancellation = CancellationToken::new();
        let (manager, mut receiver, monitor) =
            PathManager::new(identity(1), Vec::new(), cancellation.clone())
                .await
                .unwrap();
        let _peer_receiver = install_test_peer(&manager, identity(2), PeerLifecycleEpoch::test(1));
        manager
            .inner
            .lock()
            .unwrap()
            .peers
            .get_mut(&identity(2))
            .unwrap()
            .data
            .inbound
            .push_back(inbound(2, 1));

        manager.clear_inbound();

        assert!(receiver.try_recv().is_err());
        assert_eq!(monitor.snapshot().receive.dropped, 1);
        cancellation.cancel();
    }

    #[tokio::test]
    async fn inbound_receiver_rotates_between_nonempty_peer_queues() {
        let cancellation = CancellationToken::new();
        let (manager, mut receiver, _) =
            PathManager::new(identity(1), Vec::new(), cancellation.clone())
                .await
                .unwrap();
        let _peer_b_receiver =
            install_test_peer(&manager, identity(2), PeerLifecycleEpoch::test(1));
        let _peer_c_receiver =
            install_test_peer(&manager, identity(3), PeerLifecycleEpoch::test(2));
        {
            let mut state = manager.inner.lock().unwrap();
            state
                .peers
                .get_mut(&identity(2))
                .unwrap()
                .data
                .inbound
                .extend([inbound(2, 1), inbound(2, 2)]);
            state
                .peers
                .get_mut(&identity(3))
                .unwrap()
                .data
                .inbound
                .extend([inbound(3, 1), inbound(3, 2)]);
        }

        let mut order = Vec::new();
        for _ in 0..4 {
            let delivery = receiver.try_recv().unwrap();
            order.push(delivery.packet.from_steam_id64);
            delivery.receipt.complete_accepted();
        }
        assert_eq!(order, [2, 3, 2, 3]);

        cancellation.cancel();
    }
}
