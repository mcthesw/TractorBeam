//! Relay Join Code v5 compatibility codec.

use super::{
    CHECKSUM_LEN, JOIN_CODE_MAGIC, JoinCodeError, RelayJoinCode, SessionCredential,
    append_checksum, validate_checksum,
};

pub(super) const RELAY_JOIN_CODE_VERSION: u8 = 5;
const FIXED_PAYLOAD_LEN: usize = 24;
const RELAY_ID_PRESENT: u8 = 1 << 0;
const MAX_RELAY_ID_LEN: usize = 64;
const MAX_RELAY_HOST_LEN: usize = 255;
#[cfg(test)]
pub(super) const MAX_RELAY_HOST_LEN_FOR_TEST: usize = MAX_RELAY_HOST_LEN;

pub(super) fn encode_payload(code: &RelayJoinCode) -> Result<Vec<u8>, JoinCodeError> {
    let relay_id = code.relay_id.as_deref().unwrap_or("").trim();
    let relay_host = code.relay_host.trim();
    validate_fields(relay_id, relay_host, code.relay_port)?;

    let relay_id_len = u8::try_from(relay_id.len()).map_err(|_| JoinCodeError::RelayIdTooLong)?;
    let relay_host_len =
        u8::try_from(relay_host.len()).map_err(|_| JoinCodeError::RelayHostTooLong)?;
    let flags = if relay_id.is_empty() {
        0
    } else {
        RELAY_ID_PRESENT
    };
    let mut payload =
        Vec::with_capacity(FIXED_PAYLOAD_LEN + relay_id.len() + relay_host.len() + CHECKSUM_LEN);
    payload.extend_from_slice(JOIN_CODE_MAGIC);
    payload.push(RELAY_JOIN_CODE_VERSION);
    payload.push(flags);
    payload.push(relay_id_len);
    payload.push(relay_host_len);
    payload.extend_from_slice(&code.relay_port.to_be_bytes());
    payload.extend_from_slice(code.session_credential.as_bytes());
    payload.extend_from_slice(relay_id.as_bytes());
    payload.extend_from_slice(relay_host.as_bytes());
    append_checksum(&mut payload);
    Ok(payload)
}

pub(super) fn decode_payload(bytes: &[u8]) -> Result<RelayJoinCode, JoinCodeError> {
    if bytes.len() < FIXED_PAYLOAD_LEN + CHECKSUM_LEN {
        return Err(JoinCodeError::Truncated);
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
    validate_checksum(bytes, content_len)?;

    let relay_port = u16::from_be_bytes([bytes[6], bytes[7]]);
    let mut credential = [0_u8; 16];
    credential.copy_from_slice(&bytes[8..FIXED_PAYLOAD_LEN]);
    let relay_id_end = FIXED_PAYLOAD_LEN + relay_id_len;
    let relay_id = std::str::from_utf8(&bytes[FIXED_PAYLOAD_LEN..relay_id_end])
        .map_err(|_| JoinCodeError::InvalidRelayIdUtf8)?;
    let relay_host = std::str::from_utf8(&bytes[relay_id_end..content_len])
        .map_err(|_| JoinCodeError::InvalidRelayHostUtf8)?;
    validate_fields(relay_id, relay_host, relay_port)?;

    Ok(RelayJoinCode {
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
