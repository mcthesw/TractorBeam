//! Versioned wire formats shared by the Bridge Client and Relay Server.

mod control;
mod envelope;
mod local;

pub use control::{ControlMessage, ControlMessageError};
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
            challenge: None,
        };

        let bytes = message.encode().unwrap();
        let decoded = ControlMessage::decode(&bytes).unwrap();

        assert_eq!(decoded, message);
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
