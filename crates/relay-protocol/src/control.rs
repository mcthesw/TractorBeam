use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    CAP_ADMISSION_MATERIAL, CAP_PATH_VALIDATION, CAP_POW_ADMISSION, PROTOCOL_MAJOR, PROTOCOL_MINOR,
};

pub const POW_ALGORITHM_SHA256: &str = "sha256";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerTransport {
    Udp,
    Tcp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PeerInfo {
    pub steam_id64: String,
    pub display_name: Option<String>,
    pub transport: PeerTransport,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    Join {
        room: String,
        steam_id64: String,
        display_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<ClientMetadata>,
        challenge: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pow_proof: Option<PowProof>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        admission: Option<String>,
    },
    Challenge {
        token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pow: Option<PowChallenge>,
    },
    Ready {
        #[serde(default)]
        peers: Vec<PeerInfo>,
    },
    RoomUpdate {
        peers: Vec<PeerInfo>,
    },
    Error {
        code: String,
        message: String,
    },
    Heartbeat,
    HealthPing {
        id: u64,
    },
    HealthPong {
        id: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientMetadata {
    pub app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_hash: Option<String>,
    pub protocol_major: u8,
    pub protocol_minor: u8,
    pub features: u64,
}

impl ClientMetadata {
    #[must_use]
    pub fn for_build(app_version: &str, git_hash: Option<&str>) -> Self {
        Self {
            app_version: app_version.to_owned(),
            git_hash: git_hash.map(str::to_owned),
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            features: CAP_PATH_VALIDATION | CAP_POW_ADMISSION | CAP_ADMISSION_MATERIAL,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PowChallenge {
    pub algorithm: String,
    pub nonce: String,
    pub difficulty_bits: u8,
}

impl PowChallenge {
    #[must_use]
    pub fn sha256(nonce: String, difficulty_bits: u8) -> Self {
        Self {
            algorithm: POW_ALGORITHM_SHA256.to_owned(),
            nonce,
            difficulty_bits,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PowProof {
    pub nonce: String,
}

impl PowProof {
    #[must_use]
    pub fn solve(
        challenge: &PowChallenge,
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

    #[must_use]
    pub fn verify(
        &self,
        challenge: &PowChallenge,
        token: &str,
        room: &str,
        steam_id64: &str,
    ) -> bool {
        if challenge.algorithm != POW_ALGORITHM_SHA256 {
            return false;
        }
        has_leading_zero_bits(
            &pow_digest(challenge, token, room, steam_id64, &self.nonce),
            challenge.difficulty_bits,
        )
    }
}

fn pow_digest(
    challenge: &PowChallenge,
    token: &str,
    room: &str,
    steam_id64: &str,
    proof_nonce: &str,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in [token, room, steam_id64, &challenge.nonce, proof_nonce] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher.finalize().into()
}

fn has_leading_zero_bits(bytes: &[u8; 32], difficulty_bits: u8) -> bool {
    let whole_bytes = usize::from(difficulty_bits / 8);
    let remaining_bits = difficulty_bits % 8;
    if whole_bytes > bytes.len() {
        return false;
    }
    if bytes[..whole_bytes].iter().any(|byte| *byte != 0) {
        return false;
    }
    if remaining_bits == 0 {
        return true;
    }
    let Some(byte) = bytes.get(whole_bytes) else {
        return false;
    };
    byte >> (8 - remaining_bits) == 0
}

impl ControlMessage {
    pub fn encode(&self) -> Result<Bytes, ControlMessageError> {
        serde_json::to_vec(self)
            .map(Bytes::from)
            .map_err(ControlMessageError::Json)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ControlMessageError> {
        serde_json::from_slice(bytes).map_err(ControlMessageError::Json)
    }
}

#[derive(Debug, Error)]
pub enum ControlMessageError {
    #[error("control message json error: {0}")]
    Json(serde_json::Error),
}
