use std::{
    fmt::{self, Display},
    time::Instant,
};

use sha2::{Digest as _, Sha256};

const POW_ALGORITHM_SHA256: &str = "sha256";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PeerId(u64);

impl PeerId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn value(self) -> u64 {
        self.0
    }
}

impl Display for PeerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "peer-{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeerTransport {
    Udp,
    Tcp,
}

impl Display for PeerTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Udp => formatter.write_str("udp"),
            Self::Tcp => formatter.write_str("tcp"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SupportedProtocol {
    pub(crate) major: u8,
    pub(crate) minor: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClientIdentity {
    pub(crate) protocol_major: u8,
    pub(crate) protocol_minor: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AdmissionChallenge {
    pub(crate) algorithm: String,
    pub(crate) nonce: String,
    pub(crate) difficulty_bits: u8,
}

impl AdmissionChallenge {
    pub(crate) fn sha256(nonce: String, difficulty_bits: u8) -> Self {
        Self {
            algorithm: POW_ALGORITHM_SHA256.to_owned(),
            nonce,
            difficulty_bits,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AdmissionProof {
    pub(crate) nonce: String,
}

impl AdmissionProof {
    #[cfg(test)]
    pub(crate) fn solve(
        challenge: &AdmissionChallenge,
        token: &str,
        room: &str,
        steam_id64: &str,
    ) -> Option<Self> {
        if challenge.algorithm != POW_ALGORITHM_SHA256 {
            return None;
        }
        for counter in 0_u64.. {
            let proof = Self {
                nonce: format!("{counter:016x}"),
            };
            if proof.verify(challenge, token, room, steam_id64) {
                return Some(proof);
            }
        }
        None
    }

    pub(crate) fn verify(
        &self,
        challenge: &AdmissionChallenge,
        token: &str,
        room: &str,
        steam_id64: &str,
    ) -> bool {
        if challenge.algorithm != POW_ALGORITHM_SHA256 {
            return false;
        }
        let mut hasher = Sha256::new();
        for part in [token, room, steam_id64, &challenge.nonce, &self.nonce] {
            hasher.update(part.as_bytes());
            hasher.update([0]);
        }
        has_leading_zero_bits(&hasher.finalize().into(), challenge.difficulty_bits)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PeerDescription {
    pub(crate) steam_id64: String,
    pub(crate) display_name: Option<String>,
    pub(crate) transport: PeerTransport,
}

#[derive(Clone, Debug)]
pub(crate) struct JoinRequest {
    pub(crate) peer_id: PeerId,
    pub(crate) room: String,
    pub(crate) steam_id64: String,
    pub(crate) display_name: Option<String>,
    pub(crate) client: Option<ClientIdentity>,
    pub(crate) admission: Option<String>,
    pub(crate) now: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct JoinCompletion {
    pub(crate) peer_id: PeerId,
    pub(crate) room: String,
    pub(crate) steam_id64: String,
    pub(crate) client: Option<ClientIdentity>,
    pub(crate) challenge: String,
    pub(crate) pow_proof: Option<AdmissionProof>,
    pub(crate) transport: PeerTransport,
    pub(crate) now: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct RoomBroadcast {
    pub(crate) recipients: Vec<PeerId>,
    pub(crate) peers: Vec<PeerDescription>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CleanupOutcome {
    pub(crate) broadcasts: Vec<RoomBroadcast>,
    pub(crate) removed_peers: Vec<PeerId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RelayRejection {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl RelayRejection {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum JoinResponse {
    Challenge {
        token: String,
        pow: Option<AdmissionChallenge>,
    },
    Ready {
        peers: Vec<PeerDescription>,
    },
    Rejected(RelayRejection),
}

#[derive(Clone, Debug)]
pub(crate) struct JoinOutcome {
    pub(crate) response: JoinResponse,
    pub(crate) broadcast: Option<RoomBroadcast>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RoomSummary {
    pub(crate) name: String,
    pub(crate) peers: usize,
    pub(crate) tcp_peers: usize,
    pub(crate) udp_peers: usize,
}

fn has_leading_zero_bits(bytes: &[u8; 32], difficulty_bits: u8) -> bool {
    let whole_bytes = usize::from(difficulty_bits / 8);
    let remaining_bits = difficulty_bits % 8;
    if whole_bytes > bytes.len() || bytes[..whole_bytes].iter().any(|byte| *byte != 0) {
        return false;
    }
    remaining_bits == 0
        || bytes
            .get(whole_bytes)
            .is_some_and(|byte| byte >> (8 - remaining_bits) == 0)
}
