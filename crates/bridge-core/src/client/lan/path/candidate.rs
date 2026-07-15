use super::*;
use rand::RngExt as _;

pub(super) fn path_offer(
    local: PeerIdentity,
    material: PathMaterial,
    candidates: &[LocalCandidate],
) -> ControlMessage {
    ControlMessage::PathOffer {
        peer: local,
        path_id: material.id,
        path_token: material.token,
        data_candidates: candidates.iter().map(|candidate| candidate.wire).collect(),
    }
}

pub(super) fn select_candidate_pair(
    checks: &BTreeMap<(SocketAddr, SocketAddr), CheckState>,
    local_priorities: &HashMap<SocketAddr, u32>,
    remote_priorities: &HashMap<SocketAddr, u32>,
) -> Option<(SocketAddr, SocketAddr)> {
    checks
        .iter()
        .filter(|(_, check)| check.request_seen && check.response_seen)
        .max_by_key(|((local, remote), _)| {
            (
                local_priorities.get(local).copied().unwrap_or_default()
                    + remote_priorities.get(remote).copied().unwrap_or_default(),
                *local,
                *remote,
            )
        })
        .map(|(pair, _)| *pair)
}

pub(super) fn nonzero_random<const N: usize>() -> [u8; N] {
    loop {
        let value = rand::rng().random::<[u8; N]>();
        if value.iter().any(|byte| *byte != 0) {
            return value;
        }
    }
}
