use bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;

use super::{
    GAME_PACKET_BASE_HEADER_LEN, GAME_PACKET_HEADER_LEN, GAME_PACKET_MAGIC, PROTOCOL_MAJOR,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GamePacket {
    pub from_steam_id64: String,
    pub to_steam_id64: u64,
    pub source_sequence: u32,
    pub channel: i32,
    pub send_type: i32,
    pub payload: Bytes,
}

impl GamePacket {
    pub fn encode(&self) -> Result<Bytes, GamePacketError> {
        let from_steam_id64 = self
            .from_steam_id64
            .parse::<u64>()
            .map_err(|_| GamePacketError::InvalidFromSteamId)?;
        let payload_len =
            u32::try_from(self.payload.len()).map_err(|_| GamePacketError::PayloadTooLarge)?;
        let mut bytes = BytesMut::with_capacity(GAME_PACKET_HEADER_LEN + self.payload.len());
        bytes.put_slice(GAME_PACKET_MAGIC);
        bytes.put_u8(PROTOCOL_MAJOR);
        bytes.put_u8(0);
        bytes.put_u16(u16::try_from(GAME_PACKET_HEADER_LEN).expect("game header fits in u16"));
        bytes.put_u64(from_steam_id64);
        bytes.put_u64(self.to_steam_id64);
        bytes.put_i32(self.channel);
        bytes.put_i32(self.send_type);
        bytes.put_u32(payload_len);
        bytes.put_u32(self.source_sequence);
        bytes.put_slice(&self.payload);
        Ok(bytes.freeze())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, GamePacketError> {
        if bytes.len() < GAME_PACKET_BASE_HEADER_LEN {
            return Err(GamePacketError::TooShort);
        }
        if &bytes[0..4] != GAME_PACKET_MAGIC {
            return Err(GamePacketError::BadMagic);
        }
        let major = bytes[4];
        if major != PROTOCOL_MAJOR {
            return Err(GamePacketError::UnsupportedMajor(major));
        }
        let header_len = usize::from(u16::from_be_bytes(
            bytes[6..8].try_into().expect("slice length checked"),
        ));
        if header_len < GAME_PACKET_BASE_HEADER_LEN {
            return Err(GamePacketError::BadHeaderLength(header_len));
        }
        let payload_len = usize::try_from(u32::from_be_bytes(
            bytes[32..36].try_into().expect("slice length checked"),
        ))
        .map_err(|_| GamePacketError::PayloadTooLarge)?;
        if bytes.len() < header_len + payload_len {
            return Err(GamePacketError::TooShort);
        }
        let from_steam_id64 = u64::from_be_bytes(bytes[8..16].try_into().expect("checked"));
        let to_steam_id64 = u64::from_be_bytes(bytes[16..24].try_into().expect("checked"));
        let channel = i32::from_be_bytes(bytes[24..28].try_into().expect("checked"));
        let send_type = i32::from_be_bytes(bytes[28..32].try_into().expect("checked"));
        let source_sequence = if header_len >= GAME_PACKET_HEADER_LEN {
            u32::from_be_bytes(bytes[36..40].try_into().expect("checked"))
        } else {
            0
        };

        Ok(Self {
            from_steam_id64: from_steam_id64.to_string(),
            to_steam_id64,
            source_sequence,
            channel,
            send_type,
            payload: Bytes::copy_from_slice(&bytes[header_len..header_len + payload_len]),
        })
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum GamePacketError {
    #[error("game packet is too short")]
    TooShort,
    #[error("game packet magic is invalid")]
    BadMagic,
    #[error("unsupported game packet major version {0}")]
    UnsupportedMajor(u8),
    #[error("bad game packet header length {0}")]
    BadHeaderLength(usize),
    #[error("game packet payload is too large")]
    PayloadTooLarge,
    #[error("game packet sender SteamID64 is invalid")]
    InvalidFromSteamId,
}
