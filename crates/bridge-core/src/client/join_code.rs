//! Opaque player-shareable codes for explicit session routes.

use std::net::SocketAddr;

use bs58::{FromBase58 as _, ToBase58 as _};
use rand::RngExt as _;
use sha2::{Digest as _, Sha256};
use tractor_beam_direct_protocol::{CandidateValidationError, PeerIdentity};

mod lan;
mod relay;

use super::RelayEndpoint;

const JOIN_CODE_MAGIC: &[u8; 2] = b"TB";
const JOIN_CODE_PREFIX: char = 'T';
const JOIN_CODE_SUFFIX: char = 'T';
const CHECKSUM_LEN: usize = 4;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct SessionCredential([u8; 16]);

impl SessionCredential {
    #[must_use]
    pub fn generate() -> Self {
        Self(rand::rng().random())
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub(super) fn wire_secret(&self) -> String {
        self.0.to_base58()
    }
}

impl std::fmt::Debug for SessionCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionCredential([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JoinCode {
    ExternalRelay(RelayJoinCode),
    LanDirect(LanJoinCode),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayJoinCode {
    pub relay_id: Option<String>,
    pub relay_host: String,
    pub relay_port: u16,
    pub session_credential: SessionCredential,
}

impl RelayJoinCode {
    #[must_use]
    pub fn endpoint(&self) -> RelayEndpoint {
        RelayEndpoint::new(&self.relay_host, self.relay_port)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanJoinCode {
    pub introducer: PeerIdentity,
    pub control_endpoints: Vec<SocketAddr>,
    pub session_credential: SessionCredential,
}

#[derive(Debug, thiserror::Error)]
pub enum JoinCodeError {
    #[error(
        "this join code is from an older Tractor Beam version; ask the room creator to copy a new code"
    )]
    LegacyV4,
    #[error("invalid join code: missing T prefix")]
    MissingPrefix,
    #[error("invalid join code: missing T suffix")]
    MissingSuffix,
    #[error("invalid join code encoding: {0}")]
    InvalidEncoding(String),
    #[error("invalid join code: truncated payload")]
    Truncated,
    #[error("invalid join code: trailing payload bytes")]
    TrailingBytes,
    #[error("invalid join code: payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("invalid join code: bad magic")]
    BadMagic,
    #[error("invalid join code: unsupported version {0}")]
    UnsupportedVersion(u8),
    #[error("invalid join code: unsupported route {0}")]
    UnsupportedRoute(u8),
    #[error("invalid join code: unsupported flags {0:#x}")]
    UnsupportedFlags(u8),
    #[error("invalid join code: reserved field is non-zero")]
    NonZeroReserved,
    #[error("invalid join code: relay id is too long")]
    RelayIdTooLong,
    #[error("invalid join code: relay host is required")]
    MissingRelayHost,
    #[error("invalid join code: relay host is too long")]
    RelayHostTooLong,
    #[error("invalid join code: relay port is invalid")]
    InvalidRelayPort,
    #[error("invalid join code: relay id is not UTF-8")]
    InvalidRelayIdUtf8,
    #[error("invalid join code: relay host is not UTF-8")]
    InvalidRelayHostUtf8,
    #[error("invalid LAN join code: at least one Introducer candidate is required")]
    MissingLanCandidates,
    #[error("invalid LAN join code: too many Introducer candidates: {0}")]
    TooManyLanCandidates(usize),
    #[error("invalid LAN join code: duplicate Introducer candidate: {0}")]
    DuplicateLanCandidate(SocketAddr),
    #[error("invalid LAN join code candidate: {0}")]
    InvalidLanCandidate(#[from] CandidateValidationError),
    #[error("invalid LAN join code: Introducer SteamID64 must be non-zero")]
    ZeroIntroducerSteamId,
    #[error("invalid LAN join code: Introducer instance id must be non-zero")]
    ZeroIntroducerInstanceId,
    #[error("invalid LAN join code: unsupported candidate address family {0}")]
    UnsupportedAddressFamily(u8),
    #[error("invalid LAN join code: IPv6 flow information is unsupported")]
    UnsupportedIpv6FlowInfo,
    #[error("invalid LAN join code: candidate reserved field is non-zero")]
    NonZeroCandidateReserved,
    #[error("invalid join code: checksum mismatch")]
    ChecksumMismatch,
}

impl JoinCode {
    pub fn encode(&self) -> Result<String, JoinCodeError> {
        let payload = match self {
            Self::ExternalRelay(code) => relay::encode_payload(code)?,
            Self::LanDirect(code) => lan::encode_payload(code)?,
        };
        Ok(format!(
            "{JOIN_CODE_PREFIX}{}{JOIN_CODE_SUFFIX}",
            payload.to_base58()
        ))
    }

    pub fn decode(value: &str) -> Result<Self, JoinCodeError> {
        let trimmed = value.trim();
        if trimmed.starts_with('B') && trimmed.ends_with('B') {
            return Err(JoinCodeError::LegacyV4);
        }
        let Some(inner) = trimmed.strip_prefix(JOIN_CODE_PREFIX) else {
            return Err(JoinCodeError::MissingPrefix);
        };
        let Some(encoded) = inner.strip_suffix(JOIN_CODE_SUFFIX) else {
            return Err(JoinCodeError::MissingSuffix);
        };
        let bytes = encoded
            .from_base58()
            .map_err(|error| JoinCodeError::InvalidEncoding(format!("{error:?}")))?;
        decode_payload(&bytes)
    }
}

fn decode_payload(bytes: &[u8]) -> Result<JoinCode, JoinCodeError> {
    if bytes.len() < JOIN_CODE_MAGIC.len() + 1 {
        return Err(JoinCodeError::Truncated);
    }
    if &bytes[..JOIN_CODE_MAGIC.len()] != JOIN_CODE_MAGIC {
        return Err(JoinCodeError::BadMagic);
    }
    match bytes[2] {
        relay::RELAY_JOIN_CODE_VERSION => relay::decode_payload(bytes).map(JoinCode::ExternalRelay),
        lan::LAN_JOIN_CODE_VERSION => lan::decode_payload(bytes).map(JoinCode::LanDirect),
        version => Err(JoinCodeError::UnsupportedVersion(version)),
    }
}

fn append_checksum(payload: &mut Vec<u8>) {
    payload.extend_from_slice(&checksum(payload));
}

fn validate_checksum(bytes: &[u8], content_len: usize) -> Result<(), JoinCodeError> {
    if checksum(&bytes[..content_len]) != bytes[content_len..] {
        return Err(JoinCodeError::ChecksumMismatch);
    }
    Ok(())
}

fn checksum(payload: &[u8]) -> [u8; CHECKSUM_LEN] {
    let digest = Sha256::digest(payload);
    digest[..CHECKSUM_LEN]
        .try_into()
        .expect("checksum slice length is fixed")
}

#[cfg(test)]
#[path = "join_code_tests.rs"]
mod tests;
