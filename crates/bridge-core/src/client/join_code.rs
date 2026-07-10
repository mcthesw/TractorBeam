use bs58::{FromBase58 as _, ToBase58 as _};
use rand::RngExt as _;
use sha2::{Digest as _, Sha256};

use super::RelayEndpoint;

const JOIN_CODE_VERSION: u8 = 5;
const JOIN_CODE_MAGIC: &[u8; 2] = b"TB";
const JOIN_CODE_PREFIX: char = 'T';
const JOIN_CODE_SUFFIX: char = 'T';
const CHECKSUM_LEN: usize = 4;
const FIXED_PAYLOAD_LEN: usize = 24;
const RELAY_ID_PRESENT: u8 = 1 << 0;
const MAX_RELAY_ID_LEN: usize = 64;
const MAX_RELAY_HOST_LEN: usize = 255;

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

    pub(super) fn legacy_wire_value(&self) -> String {
        use std::fmt::Write as _;

        let mut value = String::with_capacity(32);
        for byte in self.0 {
            write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
        }
        value
    }
}

impl std::fmt::Debug for SessionCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionCredential([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinCode {
    pub relay_id: Option<String>,
    pub relay_host: String,
    pub relay_port: u16,
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
    #[error("invalid join code: bad magic")]
    BadMagic,
    #[error("invalid join code: unsupported version {0}")]
    UnsupportedVersion(u8),
    #[error("invalid join code: unsupported flags {0:#x}")]
    UnsupportedFlags(u8),
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
    #[error("invalid join code: checksum mismatch")]
    ChecksumMismatch,
}

impl JoinCode {
    pub fn encode(&self) -> Result<String, JoinCodeError> {
        let relay_id = self.relay_id.as_deref().unwrap_or("").trim();
        let relay_host = self.relay_host.trim();
        validate_fields(relay_id, relay_host, self.relay_port)?;

        let relay_id_len =
            u8::try_from(relay_id.len()).map_err(|_| JoinCodeError::RelayIdTooLong)?;
        let relay_host_len =
            u8::try_from(relay_host.len()).map_err(|_| JoinCodeError::RelayHostTooLong)?;
        let flags = if relay_id.is_empty() {
            0
        } else {
            RELAY_ID_PRESENT
        };
        let mut payload = Vec::with_capacity(
            FIXED_PAYLOAD_LEN + relay_id.len() + relay_host.len() + CHECKSUM_LEN,
        );
        payload.extend_from_slice(JOIN_CODE_MAGIC);
        payload.push(JOIN_CODE_VERSION);
        payload.push(flags);
        payload.push(relay_id_len);
        payload.push(relay_host_len);
        payload.extend_from_slice(&self.relay_port.to_be_bytes());
        payload.extend_from_slice(self.session_credential.as_bytes());
        payload.extend_from_slice(relay_id.as_bytes());
        payload.extend_from_slice(relay_host.as_bytes());
        let checksum = checksum(&payload);
        payload.extend_from_slice(&checksum);
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

    #[must_use]
    pub fn endpoint(&self) -> RelayEndpoint {
        RelayEndpoint::new(&self.relay_host, self.relay_port)
    }
}

fn decode_payload(bytes: &[u8]) -> Result<JoinCode, JoinCodeError> {
    if bytes.len() < FIXED_PAYLOAD_LEN + CHECKSUM_LEN {
        return Err(JoinCodeError::Truncated);
    }
    if &bytes[..2] != JOIN_CODE_MAGIC {
        return Err(JoinCodeError::BadMagic);
    }
    if bytes[2] != JOIN_CODE_VERSION {
        return Err(JoinCodeError::UnsupportedVersion(bytes[2]));
    }
    let flags = bytes[3];
    if flags & !RELAY_ID_PRESENT != 0 {
        return Err(JoinCodeError::UnsupportedFlags(flags));
    }
    let relay_id_len = usize::from(bytes[4]);
    let relay_host_len = usize::from(bytes[5]);
    if relay_id_len > MAX_RELAY_ID_LEN {
        return Err(JoinCodeError::RelayIdTooLong);
    }
    if relay_host_len > MAX_RELAY_HOST_LEN {
        return Err(JoinCodeError::RelayHostTooLong);
    }
    if (flags & RELAY_ID_PRESENT == 0) != (relay_id_len == 0) {
        return Err(JoinCodeError::UnsupportedFlags(flags));
    }
    let expected_len = FIXED_PAYLOAD_LEN
        .checked_add(relay_id_len)
        .and_then(|length| length.checked_add(relay_host_len))
        .and_then(|length| length.checked_add(CHECKSUM_LEN))
        .ok_or(JoinCodeError::Truncated)?;
    if bytes.len() < expected_len {
        return Err(JoinCodeError::Truncated);
    }
    if bytes.len() != expected_len {
        return Err(JoinCodeError::TrailingBytes);
    }
    let content_len = expected_len - CHECKSUM_LEN;
    if checksum(&bytes[..content_len]) != bytes[content_len..] {
        return Err(JoinCodeError::ChecksumMismatch);
    }

    let relay_port = u16::from_be_bytes([bytes[6], bytes[7]]);
    let mut credential = [0_u8; 16];
    credential.copy_from_slice(&bytes[8..FIXED_PAYLOAD_LEN]);
    let relay_id_end = FIXED_PAYLOAD_LEN + relay_id_len;
    let relay_id = std::str::from_utf8(&bytes[FIXED_PAYLOAD_LEN..relay_id_end])
        .map_err(|_| JoinCodeError::InvalidRelayIdUtf8)?;
    let relay_host = std::str::from_utf8(&bytes[relay_id_end..content_len])
        .map_err(|_| JoinCodeError::InvalidRelayHostUtf8)?;
    validate_fields(relay_id, relay_host, relay_port)?;

    Ok(JoinCode {
        relay_id: (!relay_id.is_empty()).then(|| relay_id.to_owned()),
        relay_host: relay_host.to_owned(),
        relay_port,
        session_credential: SessionCredential::from_bytes(credential),
    })
}

fn validate_fields(relay_id: &str, relay_host: &str, relay_port: u16) -> Result<(), JoinCodeError> {
    if relay_id.len() > MAX_RELAY_ID_LEN {
        return Err(JoinCodeError::RelayIdTooLong);
    }
    if relay_host.is_empty() {
        return Err(JoinCodeError::MissingRelayHost);
    }
    if relay_host.len() > MAX_RELAY_HOST_LEN {
        return Err(JoinCodeError::RelayHostTooLong);
    }
    if relay_port == 0 {
        return Err(JoinCodeError::InvalidRelayPort);
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
mod tests {
    use super::*;

    fn sample_code(relay_id: Option<&str>) -> JoinCode {
        JoinCode {
            relay_id: relay_id.map(str::to_owned),
            relay_host: "relay.example.test".to_owned(),
            relay_port: 25_910,
            session_credential: SessionCredential::from_bytes([
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
            ]),
        }
    }

    #[test]
    fn round_trips_compact_v5_with_surrounding_whitespace() {
        for code in [sample_code(Some("guangzhou")), sample_code(None)] {
            let encoded = code.encode().unwrap();
            assert!(encoded.starts_with('T'));
            assert!(encoded.ends_with('T'));
            assert!(!encoded.contains("relay.example.test"));
            assert_eq!(
                JoinCode::decode(&format!(" \r\n{encoded}\n ")).unwrap(),
                code
            );
        }
    }

    #[test]
    fn generated_credentials_are_random_and_redacted() {
        let first = SessionCredential::generate();
        let second = SessionCredential::generate();
        assert_ne!(first, second);
        let rendered = format!("{first:?}");
        assert!(rendered.contains("REDACTED"));
        assert!(!rendered.contains(&first.legacy_wire_value()));
    }

    #[test]
    fn rejects_legacy_v4_with_specific_error() {
        assert!(matches!(
            JoinCode::decode("BlegacyB"),
            Err(JoinCodeError::LegacyV4)
        ));
    }

    #[test]
    fn detects_corruption_truncation_and_wrong_version() {
        let encoded = sample_code(Some("relay")).encode().unwrap();
        let inner = encoded.trim_matches('T');
        let mut bytes = inner.from_base58().unwrap();

        bytes[10] ^= 1;
        let corrupted = format!("T{}T", bytes.to_base58());
        assert!(matches!(
            JoinCode::decode(&corrupted),
            Err(JoinCodeError::ChecksumMismatch)
        ));

        bytes.pop();
        let truncated = format!("T{}T", bytes.to_base58());
        assert!(matches!(
            JoinCode::decode(&truncated),
            Err(JoinCodeError::Truncated)
        ));

        let mut wrong_version = inner.from_base58().unwrap();
        wrong_version[2] = 99;
        let wrong_version = format!("T{}T", wrong_version.to_base58());
        assert!(matches!(
            JoinCode::decode(&wrong_version),
            Err(JoinCodeError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn rejects_invalid_bounds_before_encoding() {
        let mut code = sample_code(None);
        code.relay_host = "x".repeat(MAX_RELAY_HOST_LEN + 1);
        assert!(matches!(
            code.encode(),
            Err(JoinCodeError::RelayHostTooLong)
        ));
        code.relay_host = "host".to_owned();
        code.relay_port = 0;
        assert!(matches!(
            code.encode(),
            Err(JoinCodeError::InvalidRelayPort)
        ));
    }
}
