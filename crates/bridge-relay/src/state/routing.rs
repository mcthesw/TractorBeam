use std::collections::HashMap;

use rand::RngExt as _;
use sha2::{Digest as _, Sha256};
use tractor_beam_relay_protocol::CAP_ROOM_PATH_PROBE;

use super::{Peer, PendingJoin, Room};
use crate::domain::{
    DataDestination, DataProfile, DataSource, PeerView, Presence, SessionKey, StateError,
};

pub(super) fn room_views(room: &Room) -> Vec<PeerView> {
    let mut peers = room
        .peers
        .values()
        .map(|peer| PeerView {
            steam_id64: peer.steam_id64,
            display_name: peer.display_name.clone(),
            presence: peer.presence,
            capabilities: peer.capabilities,
        })
        .collect::<Vec<_>>();
    peers.sort_by_key(|peer| peer.steam_id64);
    peers
}

pub(super) fn validate_source(
    sender: &Peer,
    from_steam_id64: u64,
    source: DataSource,
) -> Result<(), StateError> {
    if sender.steam_id64 != from_steam_id64 {
        return Err(StateError::SenderMismatch);
    }
    match (sender.profile, source) {
        (DataProfile::Tcp, DataSource::Tcp(peer)) if peer == sender.control_peer => Ok(()),
        (DataProfile::Udp, DataSource::Udp(address)) if Some(address) == sender.udp_address => {
            Ok(())
        }
        (DataProfile::Udp, DataSource::Udp(_)) => Err(StateError::PathNotValidated),
        _ => Err(StateError::ProfileMismatch),
    }
}

pub(super) fn target_destination(
    room: &Room,
    to_steam_id64: u64,
    require_probe: bool,
) -> Result<DataDestination, StateError> {
    let target = room
        .peers
        .values()
        .find(|peer| peer.steam_id64 == to_steam_id64)
        .ok_or(StateError::TargetNotJoined)?;
    if require_probe && target.capabilities & CAP_ROOM_PATH_PROBE == 0 {
        return Err(StateError::ProbeUnsupported);
    }
    match target.profile {
        DataProfile::Tcp if target.presence == Presence::Connected => {
            Ok(DataDestination::Tcp(target.control_peer))
        }
        DataProfile::Udp => target
            .udp_address
            .map(DataDestination::Udp)
            .ok_or(StateError::TargetUnavailable),
        DataProfile::Tcp => Err(StateError::TargetUnavailable),
    }
}

pub(super) fn verify_pow(pending: &PendingJoin, proof: &str, difficulty: u8) -> bool {
    if difficulty == 0 {
        return proof.is_empty();
    }
    let mut hasher = Sha256::new();
    hasher.update(pending.challenge_id);
    hasher.update(pending.session.0);
    hasher.update(pending.steam_id64.to_be_bytes());
    hasher.update(pending.pow_nonce);
    hasher.update(proof.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    leading_zero_bits(&digest, difficulty)
}

fn leading_zero_bits(bytes: &[u8; 32], bits: u8) -> bool {
    let whole = usize::from(bits / 8);
    let rest = bits % 8;
    whole <= bytes.len()
        && bytes[..whole].iter().all(|byte| *byte == 0)
        && (rest == 0 || bytes.get(whole).is_some_and(|byte| byte >> (8 - rest) == 0))
}

pub(super) fn random_bytes() -> [u8; 16] {
    rand::rng().random()
}

pub(super) fn random_nonzero_u64(existing: &HashMap<u64, SessionKey>) -> u64 {
    loop {
        let value: u64 = rand::rng().random();
        if value != 0 && !existing.contains_key(&value) {
            return value;
        }
    }
}
