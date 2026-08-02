use std::{net::SocketAddr, sync::Arc, time::Instant};

use thiserror::Error;
use tractor_beam_direct_protocol::{DataFrame, PathContext};

use super::{
    LanDataDirection, LanDataDropReason, LanDataOutcome, LanDataStage, NominatedPath, PathManager,
    data_plane::PeerDataKey,
};
use crate::client::packet_flow::{InboundGamePacket, OutboundGamePacket};

#[derive(Debug, Error)]
pub(in crate::client) enum LanGameSendError {
    #[error("direct peer path is unavailable for SteamID64 {0}")]
    Unavailable(u64),
    #[error("direct peer outbound queue is full for SteamID64 {0}")]
    QueueFull(u64),
    #[error("direct peer generation is closed for SteamID64 {0}")]
    GenerationClosed(u64),
}

pub(in crate::client) struct LanGameSendSuccess {
    pub peer: u64,
    pub hook_sequence: u32,
    pub delivery_sequence: u64,
    pub channel: i32,
    pub send_type: i32,
    pub payload_bytes: usize,
    pub wire_bytes: usize,
}

pub(in crate::client) trait LanGameSendObserver: Send + Sync {
    fn observe(&self, success: LanGameSendSuccess, duration: std::time::Duration);
}

struct PreparedOutbound {
    socket: Arc<tokio::net::UdpSocket>,
    endpoint: SocketAddr,
    frame: DataFrame,
    success: LanGameSendSuccess,
}

impl PathManager {
    pub(in crate::client::lan) fn try_send_game(
        &self,
        packet: OutboundGamePacket,
    ) -> Result<(), LanGameSendError> {
        let peer = packet.to_steam_id64;
        let selected = {
            let state = self.inner.lock().expect("LAN path lock poisoned");
            state
                .peers
                .values()
                .find(|path| path.peer_steam_id64() == packet.to_steam_id64)
                .map(|path| {
                    (
                        path.data.key,
                        path.data.gate.clone(),
                        path.data.outbound.clone(),
                    )
                })
        };
        let Some((key, gate, outbound)) = selected else {
            let key = self
                .inner
                .lock()
                .expect("LAN path lock poisoned")
                .latest_data_keys
                .get(&peer)
                .copied()
                .unwrap_or_else(|| self.ledger.unattached(peer));
            self.ledger.record(
                key,
                LanDataDirection::Send,
                LanDataStage::OutboundQueue,
                LanDataOutcome::Dropped(LanDataDropReason::PeerUnavailable),
                Some(0),
            );
            return Err(LanGameSendError::Unavailable(peer));
        };
        let result = gate.with_active(|| match outbound.try_send(packet) {
            Ok(()) => {
                let queue_depth = outbound.max_capacity().saturating_sub(outbound.capacity());
                self.ledger.record(
                    key,
                    LanDataDirection::Send,
                    LanDataStage::OutboundQueue,
                    LanDataOutcome::Queued,
                    Some(queue_depth),
                );
                Ok(())
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.ledger.record(
                    key,
                    LanDataDirection::Send,
                    LanDataStage::OutboundQueue,
                    LanDataOutcome::Dropped(LanDataDropReason::QueueFull),
                    Some(outbound.max_capacity()),
                );
                Err(LanGameSendError::QueueFull(peer))
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.ledger.record(
                    key,
                    LanDataDirection::Send,
                    LanDataStage::OutboundQueue,
                    LanDataOutcome::Dropped(LanDataDropReason::QueueClosed),
                    Some(0),
                );
                Err(LanGameSendError::GenerationClosed(peer))
            }
        });
        result.unwrap_or_else(|| {
            self.ledger.record(
                key,
                LanDataDirection::Send,
                LanDataStage::OutboundQueue,
                LanDataOutcome::Dropped(LanDataDropReason::GenerationClosed),
                Some(0),
            );
            Err(LanGameSendError::GenerationClosed(peer))
        })
    }

    fn prepare_outbound(
        &self,
        key: PeerDataKey,
        packet: OutboundGamePacket,
    ) -> Result<PreparedOutbound, LanDataDropReason> {
        if packet.payload.len() > tractor_beam_direct_protocol::MAX_DATA_PAYLOAD {
            return Err(LanDataDropReason::PayloadTooLarge);
        }
        let mut state = self.inner.lock().expect("LAN path lock poisoned");
        let Some(path) = state.peers.get_mut(&key.identity) else {
            return Err(LanDataDropReason::GenerationClosed);
        };
        if path.data.key != key {
            return Err(LanDataDropReason::GenerationClosed);
        }
        let Some(nominated) = path.nominated else {
            return Err(LanDataDropReason::PeerUnavailable);
        };
        let Some(material) = path.material else {
            return Err(LanDataDropReason::PeerUnavailable);
        };
        let Some(socket) = self.socket_for(nominated.local_endpoint) else {
            return Err(LanDataDropReason::PeerUnavailable);
        };
        let frame_id = path.next_frame_id;
        path.next_frame_id = path.next_frame_id.checked_add(1).unwrap_or(1);
        let remote = path.remote_identity();
        let success = LanGameSendSuccess {
            peer: packet.to_steam_id64,
            hook_sequence: packet.hook_sequence,
            delivery_sequence: packet.delivery_sequence,
            channel: packet.channel,
            send_type: packet.send_type,
            payload_bytes: packet.payload.len(),
            wire_bytes: tractor_beam_direct_protocol::DATA_FRAME_OVERHEAD + packet.payload.len(),
        };
        Ok(PreparedOutbound {
            socket,
            endpoint: nominated.remote_endpoint,
            frame: DataFrame {
                path: PathContext {
                    path_id: material.id,
                    path_token: material.token,
                    from: self.local,
                    to_steam_id64: remote.steam_id64,
                },
                frame_id,
                delivery_stream_id: tractor_beam_direct_protocol::DeliveryStreamId::from_bytes(
                    packet.delivery_stream_id.as_bytes(),
                ),
                delivery_sequence: packet.delivery_sequence,
                channel: packet.channel,
                send_type: packet.send_type,
                payload: packet.payload,
            },
            success,
        })
    }

    pub(in crate::client::lan) fn handle_data(
        &self,
        local: SocketAddr,
        source: SocketAddr,
        frame: DataFrame,
    ) {
        let gameplay_attached = self
            .send_observer
            .lock()
            .expect("LAN send observer lock poisoned")
            .is_some();
        let enqueued = {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            let Some(path) = state.peers.get_mut(&frame.path.from) else {
                return;
            };
            let Some(nominated) = path.nominated else {
                return;
            };
            if !valid_data_path(
                self.local.steam_id64,
                path.identity,
                local,
                source,
                nominated,
                path.material,
                &frame,
            ) || frame.frame_id <= path.last_received_frame_id
            {
                return;
            }
            path.last_received_frame_id = frame.frame_id;
            let packet = InboundGamePacket {
                from_steam_id64: frame.path.from.steam_id64,
                delivery_stream_id: crate::client::packet_flow::DeliveryStreamId::from_bytes(
                    frame.delivery_stream_id.as_bytes().to_owned(),
                ),
                delivery_sequence: frame.delivery_sequence,
                channel: frame.channel,
                send_type: frame.send_type,
                payload: frame.payload,
            };
            let key = path.data.key;
            if !gameplay_attached {
                self.ledger.record(
                    key,
                    LanDataDirection::Receive,
                    LanDataStage::InboundQueue,
                    LanDataOutcome::Dropped(LanDataDropReason::SessionClosed),
                    Some(path.data.inbound.len()),
                );
                false
            } else if path.data.inbound.len() == super::PER_PEER_PACKET_QUEUE_CAPACITY {
                self.ledger.record(
                    key,
                    LanDataDirection::Receive,
                    LanDataStage::InboundQueue,
                    LanDataOutcome::Dropped(LanDataDropReason::QueueFull),
                    Some(path.data.inbound.len()),
                );
                false
            } else {
                path.data.inbound.push_back(packet);
                self.ledger.record(
                    key,
                    LanDataDirection::Receive,
                    LanDataStage::InboundQueue,
                    LanDataOutcome::Queued,
                    Some(path.data.inbound.len()),
                );
                true
            }
        };
        if enqueued {
            self.inbound_notify.notify_one();
        }
    }
}

pub(super) async fn run_outbound_worker(
    manager: Arc<PathManager>,
    key: PeerDataKey,
    mut outbound: tokio::sync::mpsc::Receiver<OutboundGamePacket>,
) {
    loop {
        tokio::select! {
            biased;
            () = manager.cancellation.cancelled() => {
                drain_outbound(&manager, key, &mut outbound);
                return;
            }
            packet = outbound.recv() => {
                let Some(packet) = packet else {
                    drain_outbound(&manager, key, &mut outbound);
                    return;
                };
                manager.ledger.set_queue_depth(
                    key,
                    LanDataDirection::Send,
                    outbound.len(),
                );
                send_outbound(&manager, key, packet).await;
            }
        }
    }
}

async fn send_outbound(manager: &PathManager, key: PeerDataKey, packet: OutboundGamePacket) {
    let started = Instant::now();
    let prepared = manager.prepare_outbound(key, packet);
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(reason) => {
            manager.ledger.record(
                key,
                LanDataDirection::Send,
                LanDataStage::UdpSend,
                LanDataOutcome::Dropped(reason),
                None,
            );
            return;
        }
    };
    let encoded = match prepared.frame.encode() {
        Ok(encoded) => encoded,
        Err(_) => {
            manager.ledger.record(
                key,
                LanDataDirection::Send,
                LanDataStage::UdpSend,
                LanDataOutcome::Dropped(LanDataDropReason::EncodeFailed),
                None,
            );
            return;
        }
    };
    let succeeded = prepared
        .socket
        .send_to(&encoded, prepared.endpoint)
        .await
        .is_ok();
    let outcome = if succeeded {
        LanDataOutcome::Succeeded
    } else {
        LanDataOutcome::Dropped(LanDataDropReason::SendFailed)
    };
    manager.ledger.record(
        key,
        LanDataDirection::Send,
        LanDataStage::UdpSend,
        outcome,
        None,
    );
    if succeeded && let Some(observer) = manager.send_observer() {
        observer.observe(prepared.success, started.elapsed());
    }
}

fn drain_outbound(
    manager: &PathManager,
    key: PeerDataKey,
    outbound: &mut tokio::sync::mpsc::Receiver<OutboundGamePacket>,
) {
    let mut dropped = 0_u64;
    while outbound.try_recv().is_ok() {
        dropped = dropped.saturating_add(1);
    }
    manager.ledger.record_dropped_batch(
        key,
        LanDataDirection::Send,
        LanDataStage::OutboundQueue,
        LanDataDropReason::GenerationClosed,
        dropped,
        0,
    );
    manager.ledger.close_epoch(key, Instant::now());
}

impl super::PeerPath {
    fn remote_identity(&self) -> tractor_beam_direct_protocol::PeerIdentity {
        self.identity
    }

    fn peer_steam_id64(&self) -> u64 {
        self.identity.steam_id64
    }
}

fn valid_data_path(
    local_steam_id64: u64,
    remote_identity: tractor_beam_direct_protocol::PeerIdentity,
    local: SocketAddr,
    source: SocketAddr,
    nominated: NominatedPath,
    material: Option<super::PathMaterial>,
    frame: &DataFrame,
) -> bool {
    frame.path.to_steam_id64 == local_steam_id64
        && frame.path.from == remote_identity
        && nominated.local_endpoint == local
        && nominated.remote_endpoint == source
        && material.is_some_and(|material| {
            material.id == frame.path.path_id && material.token == frame.path.path_token
        })
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use tractor_beam_direct_protocol::{
        DeliveryStreamId, InstanceId, PathId, PathToken, PeerIdentity,
    };

    use super::*;

    #[test]
    fn stale_endpoint_identity_target_and_material_are_rejected() {
        let local = PeerIdentity::new(1, InstanceId::from_bytes([1; 16]));
        let remote = PeerIdentity::new(2, InstanceId::from_bytes([2; 16]));
        let local_endpoint = "127.0.0.1:21001".parse().unwrap();
        let remote_endpoint = "127.0.0.1:22001".parse().unwrap();
        let material = super::super::PathMaterial {
            id: PathId::from_bytes([3; 16]),
            token: PathToken::from_bytes([4; 16]),
        };
        let nominated = NominatedPath {
            local_endpoint,
            remote_endpoint,
            last_seen: std::time::Instant::now(),
        };
        let frame = DataFrame {
            path: PathContext {
                path_id: material.id,
                path_token: material.token,
                from: remote,
                to_steam_id64: local.steam_id64,
            },
            frame_id: 1,
            delivery_stream_id: DeliveryStreamId::from_bytes([1; 16]),
            delivery_sequence: 1,
            channel: 0,
            send_type: 0,
            payload: Bytes::new(),
        };

        assert!(valid_data_path(
            local.steam_id64,
            remote,
            local_endpoint,
            remote_endpoint,
            nominated,
            Some(material),
            &frame,
        ));
        assert!(!valid_data_path(
            local.steam_id64,
            remote,
            local_endpoint,
            "127.0.0.1:22002".parse().unwrap(),
            nominated,
            Some(material),
            &frame,
        ));
        let mut wrong_target = frame.clone();
        wrong_target.path.to_steam_id64 = 3;
        assert!(!valid_data_path(
            local.steam_id64,
            remote,
            local_endpoint,
            remote_endpoint,
            nominated,
            Some(material),
            &wrong_target,
        ));
        let mut wrong_identity = frame.clone();
        wrong_identity.path.from = PeerIdentity::new(3, InstanceId::from_bytes([3; 16]));
        assert!(!valid_data_path(
            local.steam_id64,
            remote,
            local_endpoint,
            remote_endpoint,
            nominated,
            Some(material),
            &wrong_identity,
        ));
        let wrong_material = super::super::PathMaterial {
            id: PathId::from_bytes([5; 16]),
            token: PathToken::from_bytes([6; 16]),
        };
        assert!(!valid_data_path(
            local.steam_id64,
            remote,
            local_endpoint,
            remote_endpoint,
            nominated,
            Some(wrong_material),
            &frame,
        ));
    }
}
