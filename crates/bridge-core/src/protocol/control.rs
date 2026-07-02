use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
        challenge: Option<String>,
    },
    Challenge {
        token: String,
    },
    Ready {
        peer_count: usize,
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
