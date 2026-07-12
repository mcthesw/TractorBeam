use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

use super::frame::MAX_CONTROL_PAYLOAD;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataProfile {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerPresence {
    Connected,
    Reconnecting,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeRejectCode {
    UnknownConnection,
    InvalidCredential,
    Expired,
    ProfileMismatch,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    InvalidMessage,
    InvalidState,
    AdmissionRejected,
    RateLimited,
    UdpUnavailable,
    PathValidationFailed,
    Internal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PeerPresenceInfo {
    pub steam_id64: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub presence: PeerPresence,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub capabilities: u64,
}

const fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientControl {
    JoinBegin {
        session_credential: SecretString,
        steam_id64: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
        data_profile: DataProfile,
    },
    JoinProof {
        challenge_id: String,
        proof: SecretString,
    },
    Resume {
        connection_id: u64,
        resume_credential: SecretString,
    },
    Stop,
    UdpPathRequest,
    UdpPathHello {
        connection_id: u64,
        path_token: SecretString,
    },
    ControlPing {
        id: u64,
    },
    ControlPong {
        id: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerControl {
    AdmissionChallenge {
        challenge_id: String,
        algorithm: String,
        nonce: String,
        difficulty_bits: u8,
    },
    JoinReady {
        connection_id: u64,
        resume_credential: SecretString,
        peers: Vec<PeerPresenceInfo>,
    },
    ResumeReady {
        connection_id: u64,
        peers: Vec<PeerPresenceInfo>,
        udp_path_valid: bool,
    },
    ResumeRejected {
        code: ResumeRejectCode,
        allow_full_join: bool,
    },
    PeerPresenceUpdate {
        peers: Vec<PeerPresenceInfo>,
    },
    UdpPathToken {
        connection_id: u64,
        path_token: SecretString,
    },
    UdpPathReady {
        connection_id: u64,
    },
    ControlPing {
        id: u64,
    },
    ControlPong {
        id: u64,
    },
    Error {
        code: ControlErrorCode,
        message: String,
        retryable: bool,
    },
}

pub fn encode_client_control(message: &ClientControl) -> Result<Bytes, ControlEncodeError> {
    encode(message)
}

pub fn decode_client_control(payload: &[u8]) -> Result<ClientControl, ControlDecodeError> {
    decode(payload)
}

pub fn encode_server_control(message: &ServerControl) -> Result<Bytes, ControlEncodeError> {
    encode(message)
}

pub fn decode_server_control(payload: &[u8]) -> Result<ServerControl, ControlDecodeError> {
    decode(payload)
}

fn encode<T: Serialize>(message: &T) -> Result<Bytes, ControlEncodeError> {
    let payload = serde_json::to_vec(message).map_err(ControlEncodeError::Json)?;
    if payload.len() > MAX_CONTROL_PAYLOAD {
        return Err(ControlEncodeError::PayloadTooLarge(payload.len()));
    }
    Ok(Bytes::from(payload))
}

fn decode<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ControlDecodeError> {
    if payload.len() > MAX_CONTROL_PAYLOAD {
        return Err(ControlDecodeError::PayloadTooLarge(payload.len()));
    }
    serde_json::from_slice(payload).map_err(ControlDecodeError::Json)
}

#[derive(Debug, Error)]
pub enum ControlEncodeError {
    #[error("control json error: {0}")]
    Json(serde_json::Error),
    #[error("control payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
}

#[derive(Debug, Error)]
pub enum ControlDecodeError {
    #[error("control payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("control json error: {0}")]
    Json(serde_json::Error),
}
