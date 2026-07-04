use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    build_info,
    protocol::{
        CAP_ADMISSION_MATERIAL, CAP_PATH_VALIDATION, CAP_POW_ADMISSION, PROTOCOL_MAJOR,
        PROTOCOL_MINOR,
    },
};

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
    pub fn current() -> Self {
        let build = build_info::current();
        Self {
            app_version: build.version.to_owned(),
            git_hash: build.git_hash.map(str::to_owned),
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PowProof {
    pub nonce: String,
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
