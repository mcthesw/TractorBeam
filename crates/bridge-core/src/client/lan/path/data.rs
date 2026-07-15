use std::{io, net::SocketAddr};

use thiserror::Error;
use tractor_beam_direct_protocol::{DataFrame, PathContext};

use super::{NominatedPath, PathManager};
use crate::client::packet_flow::{InboundGamePacket, OutboundGamePacket};

#[derive(Debug, Error)]
pub(in crate::client) enum LanGameSendError {
    #[error("direct peer path is unavailable for SteamID64 {0}")]
    Unavailable(u64),
    #[error("direct game payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("direct game frame encoding failed: {0}")]
    Encode(tractor_beam_direct_protocol::FrameEncodeError),
    #[error("direct UDP send failed: {0}")]
    Send(io::Error),
}

impl PathManager {
    pub(in crate::client::lan) async fn send_game(
        &self,
        packet: OutboundGamePacket,
    ) -> Result<(), LanGameSendError> {
        if packet.payload.len() > tractor_beam_direct_protocol::MAX_DATA_PAYLOAD {
            return Err(LanGameSendError::PayloadTooLarge(packet.payload.len()));
        }
        let (socket, endpoint, frame) = {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            let path = state
                .peers
                .values_mut()
                .find(|path| path.peer_steam_id64() == packet.to_steam_id64)
                .ok_or(LanGameSendError::Unavailable(packet.to_steam_id64))?;
            let nominated = path
                .nominated
                .ok_or(LanGameSendError::Unavailable(packet.to_steam_id64))?;
            let material = path
                .material
                .ok_or(LanGameSendError::Unavailable(packet.to_steam_id64))?;
            let frame_id = path.next_frame_id;
            path.next_frame_id = path.next_frame_id.checked_add(1).unwrap_or(1);
            let socket = self
                .socket_for(nominated.local_endpoint)
                .ok_or(LanGameSendError::Unavailable(packet.to_steam_id64))?;
            let remote = path.remote_identity();
            let frame = DataFrame {
                path: PathContext {
                    path_id: material.id,
                    path_token: material.token,
                    from: self.local,
                    to_steam_id64: remote.steam_id64,
                },
                frame_id,
                source_sequence: packet.source_sequence,
                channel: packet.channel,
                send_type: packet.send_type,
                payload: packet.payload,
            }
            .encode()
            .map_err(LanGameSendError::Encode)?;
            (socket, nominated.remote_endpoint, frame)
        };
        socket
            .send_to(&frame, endpoint)
            .await
            .map_err(LanGameSendError::Send)?;
        Ok(())
    }

    pub(in crate::client::lan) fn handle_data(
        &self,
        local: SocketAddr,
        source: SocketAddr,
        frame: DataFrame,
    ) {
        let packet = {
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
                || (frame.source_sequence != 0
                    && frame.source_sequence <= path.last_source_sequence)
            {
                return;
            }
            path.last_received_frame_id = frame.frame_id;
            if frame.source_sequence != 0 {
                path.last_source_sequence = frame.source_sequence;
            }
            InboundGamePacket {
                from_steam_id64: frame.path.from.steam_id64,
                source_sequence: frame.source_sequence,
                channel: frame.channel,
                send_type: frame.send_type,
                payload: frame.payload,
            }
        };
        let _ = self.inbound.try_send(packet);
    }
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
    use tractor_beam_direct_protocol::{InstanceId, PathId, PathToken, PeerIdentity};

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
            source_sequence: 1,
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
