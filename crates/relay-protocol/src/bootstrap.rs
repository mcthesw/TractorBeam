//! Bootstrap negotiation codec and compatibility selection.

use bytes::{BufMut as _, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::KNOWN_CAPABILITIES;

pub const BOOTSTRAP_SCHEMA: u16 = 1;
pub const MAX_BOOTSTRAP_PAYLOAD: usize = 16 * 1024;
const LENGTH_PREFIX_LEN: usize = 4;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProtocolVersion {
    pub major: u8,
    pub minor: u8,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProtocolRange {
    pub major: u8,
    pub min_minor: u8,
    pub max_minor: u8,
}

impl ProtocolRange {
    #[must_use]
    pub const fn contains(self, version: ProtocolVersion) -> bool {
        self.major == version.major
            && version.minor >= self.min_minor
            && version.minor <= self.max_minor
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildMetadata {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectCode {
    UnsupportedBootstrapSchema,
    UnsupportedProtocol,
    MissingRequiredCapabilities,
    InvalidHello,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompatibilityReject {
    pub code: RejectCode,
    pub relay_supported_ranges: Vec<ProtocolRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_client_version: Option<String>,
    pub relay: BuildMetadata,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BootstrapMessage {
    ClientHello {
        bootstrap_schema: u16,
        supported_protocol_ranges: Vec<ProtocolRange>,
        required_capabilities: u64,
        optional_capabilities: u64,
        client: BuildMetadata,
    },
    ServerHello {
        bootstrap_schema: u16,
        selected_protocol: ProtocolVersion,
        enabled_capabilities: u64,
        relay: BuildMetadata,
    },
    CompatibilityReject(CompatibilityReject),
}

pub fn encode_bootstrap(message: &BootstrapMessage) -> Result<Bytes, BootstrapEncodeError> {
    let payload = serde_json::to_vec(message).map_err(BootstrapEncodeError::Json)?;
    if payload.len() > MAX_BOOTSTRAP_PAYLOAD {
        return Err(BootstrapEncodeError::PayloadTooLarge(payload.len()));
    }
    let payload_len = u32::try_from(payload.len())
        .map_err(|_| BootstrapEncodeError::PayloadTooLarge(payload.len()))?;
    let mut encoded = BytesMut::with_capacity(LENGTH_PREFIX_LEN + payload.len());
    encoded.put_u32(payload_len);
    encoded.put_slice(&payload);
    Ok(encoded.freeze())
}

pub fn decode_bootstrap(frame: &[u8]) -> Result<BootstrapMessage, BootstrapDecodeError> {
    if frame.len() < LENGTH_PREFIX_LEN {
        return Err(BootstrapDecodeError::TooShort);
    }
    let payload_len = usize::try_from(u32::from_be_bytes(
        frame[..LENGTH_PREFIX_LEN]
            .try_into()
            .expect("bootstrap prefix length checked"),
    ))
    .map_err(|_| BootstrapDecodeError::PayloadTooLarge(usize::MAX))?;
    if payload_len > MAX_BOOTSTRAP_PAYLOAD {
        return Err(BootstrapDecodeError::PayloadTooLarge(payload_len));
    }
    let expected_len = LENGTH_PREFIX_LEN + payload_len;
    if frame.len() < expected_len {
        return Err(BootstrapDecodeError::TooShort);
    }
    if frame.len() != expected_len {
        return Err(BootstrapDecodeError::TrailingBytes);
    }
    serde_json::from_slice(&frame[LENGTH_PREFIX_LEN..]).map_err(BootstrapDecodeError::Json)
}

/// Selects the capabilities enabled for one control connection.
///
/// Unknown optional bits are ignored. Unknown or unavailable required bits are
/// rejected before admission.
pub fn select_capabilities(
    required: u64,
    optional: u64,
    available: u64,
) -> Result<u64, CapabilityError> {
    let available = available & KNOWN_CAPABILITIES;
    let missing = required & !available;
    if missing != 0 {
        return Err(CapabilityError::MissingRequired(missing));
    }
    Ok(required | (optional & available))
}

/// Selects the newest version present in both advertised range sets.
pub fn select_protocol(
    client: &[ProtocolRange],
    relay: &[ProtocolRange],
) -> Result<ProtocolVersion, ProtocolSelectionError> {
    let mut selected = None;
    for range in client.iter().chain(relay) {
        if range.min_minor > range.max_minor {
            return Err(ProtocolSelectionError::InvalidRange(*range));
        }
    }
    for client_range in client {
        for relay_range in relay {
            if client_range.major != relay_range.major {
                continue;
            }
            let min_minor = client_range.min_minor.max(relay_range.min_minor);
            let max_minor = client_range.max_minor.min(relay_range.max_minor);
            if min_minor > max_minor {
                continue;
            }
            let candidate = ProtocolVersion {
                major: client_range.major,
                minor: max_minor,
            };
            if selected.is_none_or(|current: ProtocolVersion| {
                (candidate.major, candidate.minor) > (current.major, current.minor)
            }) {
                selected = Some(candidate);
            }
        }
    }
    selected.ok_or(ProtocolSelectionError::NoCommonProtocol)
}

#[derive(Debug, Error)]
pub enum BootstrapEncodeError {
    #[error("bootstrap json error: {0}")]
    Json(serde_json::Error),
    #[error("bootstrap payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
}

#[derive(Debug, Error)]
pub enum BootstrapDecodeError {
    #[error("bootstrap frame is too short")]
    TooShort,
    #[error("bootstrap payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("bootstrap frame has trailing bytes")]
    TrailingBytes,
    #[error("bootstrap json error: {0}")]
    Json(serde_json::Error),
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CapabilityError {
    #[error("required capabilities are unavailable: {0:#x}")]
    MissingRequired(u64),
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ProtocolSelectionError {
    #[error("protocol range has minimum minor above maximum minor: {0:?}")]
    InvalidRange(ProtocolRange),
    #[error("client and relay have no common protocol version")]
    NoCommonProtocol,
}
