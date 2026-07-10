use std::time::Instant;

use tractor_beam_relay_protocol::{
    ClientMetadata, ControlMessage, MessageType, PROTOCOL_MAJOR, PROTOCOL_MINOR, PeerInfo,
    PowChallenge, PowProof,
};

use crate::domain::{
    AdmissionChallenge, AdmissionProof, ClientIdentity, JoinCompletion, JoinRequest, JoinResponse,
    PeerDescription, PeerId, PeerTransport, RelayRejection, SupportedProtocol,
};

pub(crate) enum JoinCommand {
    Begin(JoinRequest),
    Complete(JoinCompletion),
    Rejected(RelayRejection),
}

pub(crate) const fn supported_protocol() -> SupportedProtocol {
    SupportedProtocol {
        major: PROTOCOL_MAJOR,
        minor: PROTOCOL_MINOR,
    }
}

pub(crate) fn decode_join(
    payload: &[u8],
    peer_id: PeerId,
    transport: PeerTransport,
    now: Instant,
) -> JoinCommand {
    let Ok(message) = ControlMessage::decode(payload) else {
        return JoinCommand::Rejected(RelayRejection::new("bad_join", "expected join message"));
    };
    match message {
        ControlMessage::Join {
            room,
            steam_id64,
            display_name: _,
            client,
            challenge: Some(challenge),
            pow_proof,
            admission: _,
        } => JoinCommand::Complete(JoinCompletion {
            peer_id,
            room,
            steam_id64,
            client: client.map(client_identity),
            challenge,
            pow_proof: pow_proof.map(admission_proof),
            transport,
            now,
        }),
        ControlMessage::Join {
            room,
            steam_id64,
            display_name,
            client,
            challenge: None,
            pow_proof: _,
            admission,
        } => JoinCommand::Begin(JoinRequest {
            peer_id,
            room,
            steam_id64,
            display_name,
            client: client.map(client_identity),
            admission,
            now,
        }),
        _ => JoinCommand::Rejected(RelayRejection::new("bad_join", "expected join message")),
    }
}

pub(crate) fn encode_join_response(response: JoinResponse) -> (MessageType, ControlMessage) {
    match response {
        JoinResponse::Challenge { token, pow } => (
            MessageType::JoinChallenge,
            ControlMessage::Challenge {
                token,
                pow: pow.map(pow_challenge),
            },
        ),
        JoinResponse::Ready { peers } => (
            MessageType::JoinReady,
            ControlMessage::Ready {
                peers: peers.into_iter().map(peer_info).collect(),
            },
        ),
        JoinResponse::Rejected(rejection) => (
            MessageType::Error,
            error_message(rejection.code, rejection.message),
        ),
    }
}

pub(crate) fn room_update(peers: Vec<PeerDescription>) -> ControlMessage {
    ControlMessage::RoomUpdate {
        peers: peers.into_iter().map(peer_info).collect(),
    }
}

pub(crate) fn error_message(code: impl Into<String>, message: impl Into<String>) -> ControlMessage {
    ControlMessage::Error {
        code: code.into(),
        message: message.into(),
    }
}

fn client_identity(client: ClientMetadata) -> ClientIdentity {
    ClientIdentity {
        protocol_major: client.protocol_major,
        protocol_minor: client.protocol_minor,
    }
}

fn admission_proof(proof: PowProof) -> AdmissionProof {
    AdmissionProof { nonce: proof.nonce }
}

fn pow_challenge(challenge: AdmissionChallenge) -> PowChallenge {
    PowChallenge {
        algorithm: challenge.algorithm,
        nonce: challenge.nonce,
        difficulty_bits: challenge.difficulty_bits,
    }
}

fn peer_info(peer: PeerDescription) -> PeerInfo {
    PeerInfo {
        steam_id64: peer.steam_id64,
        display_name: peer.display_name,
        transport: match peer.transport {
            PeerTransport::Udp => tractor_beam_relay_protocol::PeerTransport::Udp,
            PeerTransport::Tcp => tractor_beam_relay_protocol::PeerTransport::Tcp,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_bits_do_not_change_domain_admission_identity() {
        let identity_without_features = client_identity(metadata_with_features(0));
        let identity_with_features = client_identity(metadata_with_features(u64::MAX));

        assert_eq!(identity_without_features, identity_with_features);
        assert_eq!(identity_with_features.protocol_major, PROTOCOL_MAJOR);
        assert_eq!(identity_with_features.protocol_minor, PROTOCOL_MINOR);
    }

    fn metadata_with_features(features: u64) -> ClientMetadata {
        ClientMetadata {
            app_version: "0.2.1".to_owned(),
            git_hash: None,
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            features,
        }
    }
}
