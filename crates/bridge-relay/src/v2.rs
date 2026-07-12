use bs58::{FromBase58 as _, ToBase58 as _};
use tractor_beam_relay_protocol::v2::{
    ControlErrorCode, DataProfile as WireDataProfile, PeerPresence as WirePresence,
    PeerPresenceInfo, ResumeRejectCode, SecretString, ServerControl,
};

use crate::domain_v2::{
    DataProfile, PathKey, PeerView, Presence, ResumeFailure, ResumeKey, SessionKey, StateError,
};

pub(crate) fn session_key(secret: &SecretString) -> Result<SessionKey, StateError> {
    decode_base58_16(secret.expose_secret()).map(SessionKey)
}

pub(crate) fn resume_key(secret: &SecretString) -> Result<ResumeKey, ResumeFailure> {
    decode_base58_16(secret.expose_secret())
        .map(ResumeKey)
        .map_err(|_| ResumeFailure::InvalidCredential)
}

pub(crate) fn path_key(secret: &SecretString) -> Result<PathKey, StateError> {
    decode_base58_16(secret.expose_secret())
        .map(PathKey)
        .map_err(|_| StateError::PathNotValidated)
}

pub(crate) fn profile(profile: WireDataProfile) -> DataProfile {
    match profile {
        WireDataProfile::Tcp => DataProfile::Tcp,
        WireDataProfile::Udp => DataProfile::Udp,
    }
}

pub(crate) fn secret(bytes: [u8; 16]) -> SecretString {
    SecretString::new(bytes.to_base58())
}

pub(crate) fn hex(bytes: [u8; 16]) -> String {
    use std::fmt::Write as _;
    let mut value = String::with_capacity(32);
    for byte in bytes {
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

pub(crate) fn decode_hex_16(value: &str) -> Result<[u8; 16], StateError> {
    if value.len() != 32 {
        return Err(StateError::InvalidChallenge);
    }
    let mut bytes = [0_u8; 16];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(pair).map_err(|_| StateError::InvalidChallenge)?;
        bytes[index] = u8::from_str_radix(pair, 16).map_err(|_| StateError::InvalidChallenge)?;
    }
    Ok(bytes)
}

pub(crate) fn peer_views(peers: Vec<PeerView>) -> Vec<PeerPresenceInfo> {
    peers
        .into_iter()
        .map(|peer| PeerPresenceInfo {
            steam_id64: peer.steam_id64,
            display_name: peer.display_name,
            presence: match peer.presence {
                Presence::Connected => WirePresence::Connected,
                Presence::Reconnecting => WirePresence::Reconnecting,
            },
            capabilities: peer.capabilities,
        })
        .collect()
}

pub(crate) fn state_error(error: StateError) -> ServerControl {
    let (code, message, retryable) = match error {
        StateError::RoomFull => (ControlErrorCode::AdmissionRejected, "room is full", false),
        StateError::RelayFull => (
            ControlErrorCode::AdmissionRejected,
            "relay room limit reached",
            true,
        ),
        StateError::MissingChallenge | StateError::InvalidChallenge | StateError::InvalidProof => (
            ControlErrorCode::AdmissionRejected,
            "join proof was rejected",
            false,
        ),
        StateError::UnknownConnection => (
            ControlErrorCode::InvalidState,
            "connection is unknown",
            true,
        ),
        StateError::SenderMismatch => (
            ControlErrorCode::InvalidMessage,
            "data sender does not match the session",
            false,
        ),
        StateError::ProfileMismatch => (
            ControlErrorCode::InvalidState,
            "data profile does not match the session",
            false,
        ),
        StateError::PathNotValidated => (
            ControlErrorCode::PathValidationFailed,
            "UDP path is not validated",
            true,
        ),
        StateError::DuplicateFrame | StateError::FrameTooOld => (
            ControlErrorCode::InvalidMessage,
            "data frame was already observed",
            false,
        ),
        StateError::RateLimited => (
            ControlErrorCode::RateLimited,
            "data rate limit exceeded",
            true,
        ),
        StateError::TargetNotJoined => (
            ControlErrorCode::InvalidState,
            "target peer is not joined",
            true,
        ),
        StateError::TargetUnavailable => (
            ControlErrorCode::InvalidState,
            "target peer path is unavailable",
            true,
        ),
        StateError::ProbeUnsupported => (
            ControlErrorCode::InvalidState,
            "room path probe is unsupported",
            false,
        ),
        StateError::ProbeRateLimited => (
            ControlErrorCode::RateLimited,
            "room path probe rate limit exceeded",
            true,
        ),
    };
    ServerControl::Error {
        code,
        message: message.to_owned(),
        retryable,
    }
}

pub(crate) fn resume_rejection(error: ResumeFailure) -> ServerControl {
    let code = match error {
        ResumeFailure::UnknownConnection => ResumeRejectCode::UnknownConnection,
        ResumeFailure::InvalidCredential => ResumeRejectCode::InvalidCredential,
        ResumeFailure::Expired => ResumeRejectCode::Expired,
    };
    ServerControl::ResumeRejected {
        code,
        allow_full_join: matches!(
            error,
            ResumeFailure::UnknownConnection | ResumeFailure::Expired
        ),
    }
}

fn decode_base58_16(value: &str) -> Result<[u8; 16], StateError> {
    let bytes = value
        .from_base58()
        .map_err(|_| StateError::InvalidChallenge)?;
    bytes.try_into().map_err(|_| StateError::InvalidChallenge)
}
