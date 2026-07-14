//! Bounded direct Peer control messages and JSON codec.

use std::net::SocketAddr;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    HostCandidate, LinkId, MAX_CONTROL_PAYLOAD, MAX_PEERS, PathId, PathToken, PeerDescriptor,
    PeerIdentity, ProtocolRange, ProtocolVersion, SessionProof, TransactionId,
    types::{CandidateValidationError, validate_candidates, validate_protocol_ranges},
};

const MAX_ERROR_MESSAGE_LEN: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    InvalidMessage,
    UnsupportedProtocol,
    InvalidCredential,
    IdentityMismatch,
    DuplicateIdentity,
    AdmissionRejected,
    InvalidState,
    RateLimited,
    PathValidationFailed,
    Internal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    ProbeRequest {
        transaction_id: TransactionId,
        introducer: PeerIdentity,
        supported_protocol_ranges: Vec<ProtocolRange>,
        required_capabilities: u64,
        optional_capabilities: u64,
        session_proof: SessionProof,
    },
    ProbeResponse {
        transaction_id: TransactionId,
        introducer: PeerIdentity,
        selected_protocol: ProtocolVersion,
        enabled_capabilities: u64,
    },
    JoinRequest {
        link_id: LinkId,
        peer: PeerDescriptor,
        supported_protocol_ranges: Vec<ProtocolRange>,
        required_capabilities: u64,
        optional_capabilities: u64,
        session_proof: SessionProof,
    },
    JoinAccepted {
        selected_protocol: ProtocolVersion,
        enabled_capabilities: u64,
        peers: Vec<PeerDescriptor>,
    },
    JoinRejected {
        code: ControlErrorCode,
        message: String,
        retryable: bool,
    },
    PeerSnapshot {
        peers: Vec<PeerDescriptor>,
    },
    Leave,
    PathOffer {
        peer: PeerIdentity,
        path_id: PathId,
        path_token: PathToken,
        data_candidates: Vec<HostCandidate>,
    },
    Nominate {
        path_id: PathId,
        local_endpoint: SocketAddr,
        remote_endpoint: SocketAddr,
    },
    NominateAck {
        path_id: PathId,
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

pub fn encode_control(message: &ControlMessage) -> Result<Bytes, ControlEncodeError> {
    validate_control(message).map_err(ControlEncodeError::Validation)?;
    let payload = serde_json::to_vec(message).map_err(ControlEncodeError::Json)?;
    if payload.len() > MAX_CONTROL_PAYLOAD {
        return Err(ControlEncodeError::PayloadTooLarge(payload.len()));
    }
    Ok(Bytes::from(payload))
}

pub fn decode_control(payload: &[u8]) -> Result<ControlMessage, ControlDecodeError> {
    if payload.len() > MAX_CONTROL_PAYLOAD {
        return Err(ControlDecodeError::PayloadTooLarge(payload.len()));
    }
    let message = serde_json::from_slice(payload).map_err(ControlDecodeError::Json)?;
    validate_control(&message).map_err(ControlDecodeError::Validation)?;
    Ok(message)
}

fn validate_control(message: &ControlMessage) -> Result<(), ControlValidationError> {
    match message {
        ControlMessage::ProbeRequest {
            transaction_id,
            introducer,
            supported_protocol_ranges,
            required_capabilities,
            optional_capabilities,
            session_proof,
        } => {
            validate_transaction(*transaction_id)?;
            introducer.validate()?;
            validate_protocol_ranges(supported_protocol_ranges)?;
            let _ = (required_capabilities, optional_capabilities);
            validate_session_proof(*session_proof)?;
        }
        ControlMessage::ProbeResponse {
            transaction_id,
            introducer,
            selected_protocol,
            enabled_capabilities,
        } => {
            validate_transaction(*transaction_id)?;
            introducer.validate()?;
            validate_protocol(*selected_protocol)?;
            validate_enabled_capabilities(*enabled_capabilities)?;
        }
        ControlMessage::JoinRequest {
            link_id,
            peer,
            supported_protocol_ranges,
            required_capabilities,
            optional_capabilities,
            session_proof,
        } => {
            if link_id.is_zero() {
                return Err(ControlValidationError::ZeroLinkId);
            }
            peer.validate()?;
            validate_protocol_ranges(supported_protocol_ranges)?;
            let _ = (required_capabilities, optional_capabilities);
            validate_session_proof(*session_proof)?;
        }
        ControlMessage::JoinAccepted {
            selected_protocol,
            enabled_capabilities,
            peers,
        } => {
            validate_protocol(*selected_protocol)?;
            validate_enabled_capabilities(*enabled_capabilities)?;
            validate_peers(peers)?;
        }
        ControlMessage::JoinRejected { message, .. } | ControlMessage::Error { message, .. } => {
            validate_error_message(message)?;
        }
        ControlMessage::PeerSnapshot { peers } => validate_peers(peers)?,
        ControlMessage::Leave => {}
        ControlMessage::PathOffer {
            peer,
            path_id,
            path_token,
            data_candidates,
        } => {
            peer.validate()?;
            validate_path(*path_id, *path_token)?;
            validate_candidates(data_candidates)?;
        }
        ControlMessage::Nominate {
            path_id,
            local_endpoint,
            remote_endpoint,
        } => {
            if path_id.is_zero() {
                return Err(ControlValidationError::ZeroPathId);
            }
            validate_endpoint(*local_endpoint)?;
            validate_endpoint(*remote_endpoint)?;
        }
        ControlMessage::NominateAck { path_id } => {
            if path_id.is_zero() {
                return Err(ControlValidationError::ZeroPathId);
            }
        }
        ControlMessage::ControlPing { id } | ControlMessage::ControlPong { id } => {
            if *id == 0 {
                return Err(ControlValidationError::ZeroControlId);
            }
        }
    }
    Ok(())
}

fn validate_enabled_capabilities(enabled: u64) -> Result<(), ControlValidationError> {
    let unknown = enabled & !super::KNOWN_CAPABILITIES;
    if unknown != 0 {
        return Err(ControlValidationError::UnknownCapabilities(unknown));
    }
    Ok(())
}

fn validate_protocol(version: ProtocolVersion) -> Result<(), ControlValidationError> {
    if version.major == 0 {
        return Err(ControlValidationError::ZeroProtocolMajor);
    }
    Ok(())
}

fn validate_transaction(transaction: TransactionId) -> Result<(), ControlValidationError> {
    if transaction.is_zero() {
        return Err(ControlValidationError::ZeroTransactionId);
    }
    Ok(())
}

fn validate_session_proof(proof: SessionProof) -> Result<(), ControlValidationError> {
    if proof.is_zero() {
        return Err(ControlValidationError::ZeroSessionProof);
    }
    Ok(())
}

fn validate_path(path_id: PathId, path_token: PathToken) -> Result<(), ControlValidationError> {
    if path_id.is_zero() {
        return Err(ControlValidationError::ZeroPathId);
    }
    if path_token.is_zero() {
        return Err(ControlValidationError::ZeroPathToken);
    }
    Ok(())
}

fn validate_peers(peers: &[PeerDescriptor]) -> Result<(), ControlValidationError> {
    if peers.len() > MAX_PEERS {
        return Err(ControlValidationError::TooManyPeers(peers.len()));
    }
    for (index, peer) in peers.iter().enumerate() {
        peer.validate()?;
        if peers[..index]
            .iter()
            .any(|existing| existing.identity == peer.identity)
        {
            return Err(ControlValidationError::DuplicatePeer(peer.identity));
        }
    }
    Ok(())
}

fn validate_error_message(message: &str) -> Result<(), ControlValidationError> {
    if message.len() > MAX_ERROR_MESSAGE_LEN {
        return Err(ControlValidationError::ErrorMessageTooLong(message.len()));
    }
    Ok(())
}

fn validate_endpoint(endpoint: SocketAddr) -> Result<(), ControlValidationError> {
    HostCandidate::new(endpoint, 1, 0)?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum ControlEncodeError {
    #[error("direct control validation error: {0}")]
    Validation(#[from] ControlValidationError),
    #[error("direct control json error: {0}")]
    Json(serde_json::Error),
    #[error("direct control payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
}

#[derive(Debug, Error)]
pub enum ControlDecodeError {
    #[error("direct control payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("direct control json error: {0}")]
    Json(serde_json::Error),
    #[error("direct control validation error: {0}")]
    Validation(#[from] ControlValidationError),
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ControlValidationError {
    #[error("invalid candidate or peer descriptor: {0}")]
    Candidate(#[from] CandidateValidationError),
    #[error("link id must be non-zero")]
    ZeroLinkId,
    #[error("transaction id must be non-zero")]
    ZeroTransactionId,
    #[error("session proof must be non-zero")]
    ZeroSessionProof,
    #[error("path id must be non-zero")]
    ZeroPathId,
    #[error("path token must be non-zero")]
    ZeroPathToken,
    #[error("control ping/pong id must be non-zero")]
    ZeroControlId,
    #[error("protocol major version must be non-zero")]
    ZeroProtocolMajor,
    #[error("unknown capability bits: {0:#x}")]
    UnknownCapabilities(u64),
    #[error("too many peers: {0}")]
    TooManyPeers(usize),
    #[error("duplicate peer identity: {0:?}")]
    DuplicatePeer(PeerIdentity),
    #[error("control error message is too long: {0} bytes")]
    ErrorMessageTooLong(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(byte: u8) -> PeerIdentity {
        PeerIdentity::new(
            u64::from(byte),
            super::super::InstanceId::from_bytes([byte; 16]),
        )
    }

    fn candidate(port: u16) -> HostCandidate {
        HostCandidate::new(format!("127.0.0.1:{port}").parse().unwrap(), 1, 0).unwrap()
    }

    fn descriptor(byte: u8) -> PeerDescriptor {
        PeerDescriptor {
            identity: identity(byte),
            display_name: Some(format!("Peer {byte}")),
            control_candidates: vec![candidate(25_900 + u16::from(byte))],
            capabilities: super::super::KNOWN_CAPABILITIES,
        }
    }

    #[test]
    fn control_round_trip_validates_nested_values() {
        let message = ControlMessage::JoinAccepted {
            selected_protocol: ProtocolVersion { major: 1, minor: 0 },
            enabled_capabilities: super::super::KNOWN_CAPABILITIES,
            peers: vec![descriptor(1), descriptor(2)],
        };
        let encoded = encode_control(&message).unwrap();
        assert_eq!(decode_control(&encoded).unwrap(), message);
    }

    #[test]
    fn control_rejects_duplicate_peers_and_oversized_payload_before_json() {
        let peer = descriptor(1);
        let duplicate = ControlMessage::PeerSnapshot {
            peers: vec![peer.clone(), peer],
        };
        assert!(matches!(
            encode_control(&duplicate),
            Err(ControlEncodeError::Validation(
                ControlValidationError::DuplicatePeer(_)
            ))
        ));

        let oversized = vec![b' '; MAX_CONTROL_PAYLOAD + 1];
        assert!(matches!(
            decode_control(&oversized),
            Err(ControlDecodeError::PayloadTooLarge(size)) if size == MAX_CONTROL_PAYLOAD + 1
        ));
    }

    #[test]
    fn secret_fields_are_redacted_from_debug_output() {
        let secret = SessionProof::from_bytes([0xa5; 32]);
        let token = PathToken::from_bytes([0xb6; 16]);
        assert!(!format!("{secret:?}").contains("165"));
        assert!(!format!("{token:?}").contains("182"));
        assert!(format!("{secret:?}").contains("REDACTED"));
    }
}
