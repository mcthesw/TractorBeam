//! Versioned wire formats shared by the Bridge Client and Relay Server.

mod control;
mod envelope;
mod local;

pub use control::{
    ClientMetadata, ControlMessage, ControlMessageError, PeerInfo, PeerTransport, PowChallenge,
    PowProof,
};
pub use envelope::{DecodeError, EncodeError, Envelope, MessageType};
pub use local::{GamePacket, GamePacketError, LocalPacket, LocalPacketError, LocalPacketType};

pub const PROTOCOL_MAJOR: u8 = 1;
pub const PROTOCOL_MINOR: u8 = 0;
pub const ENVELOPE_MAGIC: &[u8; 4] = b"BBR1";
pub const ENVELOPE_HEADER_LEN: usize = 42;
pub const NONCE_LEN: usize = 12;
pub const LOCAL_MAGIC: &[u8; 4] = b"IBR1";
pub const LOCAL_HEADER_LEN: usize = 32;
pub const GAME_PACKET_MAGIC: &[u8; 4] = b"BBG1";
pub const GAME_PACKET_BASE_HEADER_LEN: usize = 36;
pub const GAME_PACKET_HEADER_LEN: usize = 40;

pub const CAP_PATH_VALIDATION: u64 = 1 << 0;
pub const CAP_ENCRYPTION_RESERVED: u64 = 1 << 1;
pub const CAP_POW_ADMISSION: u64 = 1 << 2;
pub const CAP_ADMISSION_MATERIAL: u64 = 1 << 3;

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn roundtrips_envelope() {
        let mut envelope = Envelope::new(MessageType::Join, Bytes::from_static(b"room"));
        envelope.sequence = 7;
        envelope.nonce = [3; NONCE_LEN];

        let bytes = envelope.encode().unwrap();
        let decoded = Envelope::decode(bytes).unwrap();

        assert_eq!(decoded, envelope);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = Envelope::new(MessageType::Heartbeat, Bytes::new())
            .encode()
            .unwrap()
            .to_vec();
        bytes[0] = b'X';

        assert_eq!(
            Envelope::decode(Bytes::from(bytes)),
            Err(DecodeError::BadMagic)
        );
    }

    #[test]
    fn skips_future_header_extensions() {
        let mut bytes = Envelope::new(MessageType::Data, Bytes::from_static(b"payload"))
            .encode()
            .unwrap()
            .to_vec();
        bytes[8..10].copy_from_slice(&(46_u16).to_be_bytes());
        bytes.splice(ENVELOPE_HEADER_LEN..ENVELOPE_HEADER_LEN, [0, 0, 0, 0]);

        let decoded = Envelope::decode(Bytes::from(bytes)).unwrap();

        assert_eq!(decoded.message_type, MessageType::Data);
        assert_eq!(decoded.payload, Bytes::from_static(b"payload"));
    }

    #[test]
    fn encodes_control_messages() {
        let message = ControlMessage::Join {
            room: "room".to_owned(),
            steam_id64: "76561198000000001".to_owned(),
            display_name: Some("Alice".to_owned()),
            client: Some(ClientMetadata {
                app_version: "1.2.3".to_owned(),
                git_hash: Some("0123456789abcdef".to_owned()),
                protocol_major: PROTOCOL_MAJOR,
                protocol_minor: PROTOCOL_MINOR,
                features: CAP_PATH_VALIDATION | CAP_POW_ADMISSION | CAP_ADMISSION_MATERIAL,
            }),
            challenge: None,
            pow_proof: None,
            admission: Some("AbCdEfGhIjKlMn12".to_owned()),
        };

        let bytes = message.encode().unwrap();
        let decoded = ControlMessage::decode(&bytes).unwrap();

        assert_eq!(decoded, message);
    }

    #[test]
    fn roundtrips_room_update() {
        let message = ControlMessage::RoomUpdate {
            peers: vec![
                super::PeerInfo {
                    steam_id64: "76561198000000001".to_owned(),
                    display_name: Some("Alice".to_owned()),
                    transport: super::PeerTransport::Tcp,
                },
                super::PeerInfo {
                    steam_id64: "76561198000000002".to_owned(),
                    display_name: None,
                    transport: super::PeerTransport::Udp,
                },
            ],
        };

        let bytes = message.encode().unwrap();
        let decoded = ControlMessage::decode(&bytes).unwrap();

        assert_eq!(decoded, message);
    }

    #[test]
    fn room_update_uses_snake_case_transport_tag() {
        let message = ControlMessage::RoomUpdate {
            peers: vec![super::PeerInfo {
                steam_id64: "1".to_owned(),
                display_name: None,
                transport: super::PeerTransport::Tcp,
            }],
        };
        let json = String::from_utf8(message.encode().unwrap().to_vec()).unwrap();
        assert!(json.contains("\"type\":\"room_update\""));
        assert!(json.contains("\"transport\":\"tcp\""));
    }

    #[test]
    fn room_update_message_type_round_trips() {
        let envelope = super::Envelope::new(MessageType::RoomUpdate, Bytes::from_static(b"x"));
        let encoded = envelope.encode().unwrap();
        let decoded = super::Envelope::decode(encoded).unwrap();
        assert_eq!(decoded.message_type, MessageType::RoomUpdate);
    }

    #[test]
    fn legacy_ready_without_peers_decodes_to_empty_vec() {
        let json = br#"{"type":"ready","peer_count":3}"#;
        let decoded = ControlMessage::decode(json).unwrap();
        match decoded {
            ControlMessage::Ready { peers } => assert!(peers.is_empty()),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn legacy_join_without_admission_fields_decodes() {
        let json = br#"{"type":"join","room":"room","steam_id64":"76561198000000001","display_name":null,"challenge":null}"#;
        let decoded = ControlMessage::decode(json).unwrap();

        match decoded {
            ControlMessage::Join {
                client,
                pow_proof,
                admission,
                ..
            } => {
                assert_eq!(client, None);
                assert_eq!(pow_proof, None);
                assert_eq!(admission, None);
            }
            other => panic!("expected Join, got {other:?}"),
        }
    }

    #[test]
    fn challenge_can_carry_pow_metadata() {
        let message = ControlMessage::Challenge {
            token: "token".to_owned(),
            pow: Some(PowChallenge {
                algorithm: "sha256".to_owned(),
                nonce: "nonce".to_owned(),
                difficulty_bits: 18,
            }),
        };

        let bytes = message.encode().unwrap();
        let decoded = ControlMessage::decode(&bytes).unwrap();

        assert_eq!(decoded, message);
    }

    #[test]
    fn pow_proof_solves_and_verifies_challenge() {
        let challenge = PowChallenge::sha256("nonce".to_owned(), 8);
        let proof = PowProof::solve(&challenge, "token", "room", "76561198000000001").unwrap();

        assert!(proof.verify(&challenge, "token", "room", "76561198000000001",));
        assert!(!proof.verify(&challenge, "token", "other-room", "76561198000000001",));
    }

    #[test]
    fn roundtrips_local_packet() {
        let packet = LocalPacket {
            packet_type: LocalPacketType::Outgoing,
            peer: 42,
            sequence: 7,
            channel: 1,
            send_type: 2,
            payload: Bytes::from_static(b"payload"),
        };

        let bytes = packet.encode().unwrap();
        let decoded = LocalPacket::decode(bytes).unwrap();

        assert_eq!(decoded, packet);
    }

    #[test]
    fn roundtrips_game_packet() {
        let game = GamePacket {
            from_steam_id64: "76561198000000001".to_owned(),
            to_steam_id64: 42,
            source_sequence: 7,
            channel: 1,
            send_type: 2,
            payload: Bytes::from_static(b"payload"),
        };

        let bytes = game.encode().unwrap();
        let decoded = GamePacket::decode(&bytes).unwrap();

        assert_eq!(decoded, game);
    }

    #[test]
    fn encodes_game_packet_without_json_payload_expansion() {
        let game = GamePacket {
            from_steam_id64: "76561198000000001".to_owned(),
            to_steam_id64: 76_561_198_000_000_002,
            source_sequence: 7,
            channel: 1,
            send_type: 2,
            payload: Bytes::from(vec![255; 2_048]),
        };

        let bytes = game.encode().unwrap();

        assert_eq!(bytes.len(), GAME_PACKET_HEADER_LEN + game.payload.len());
    }
}
