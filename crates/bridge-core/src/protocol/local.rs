use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

use super::{
    GAME_PACKET_BASE_HEADER_LEN, GAME_PACKET_HEADER_LEN, GAME_PACKET_MAGIC, LOCAL_HEADER_LEN,
    LOCAL_MAGIC, PROTOCOL_MAJOR,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LocalPacketType {
    Outgoing = 1,
    Incoming = 2,
}

impl TryFrom<u8> for LocalPacketType {
    type Error = LocalPacketError;

    fn try_from(value: u8) -> Result<Self, LocalPacketError> {
        match value {
            1 => Ok(Self::Outgoing),
            2 => Ok(Self::Incoming),
            other => Err(LocalPacketError::UnknownPacketType(other)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalPacket {
    pub packet_type: LocalPacketType,
    pub peer: u64,
    pub sequence: u32,
    pub channel: i32,
    pub send_type: i32,
    pub payload: Bytes,
}

impl LocalPacket {
    #[must_use]
    pub fn incoming(peer: u64, sequence: u32, game: GamePacket) -> Self {
        Self {
            packet_type: LocalPacketType::Incoming,
            peer,
            sequence,
            channel: game.channel,
            send_type: game.send_type,
            payload: game.payload,
        }
    }

    pub fn encode(&self) -> Result<Bytes, LocalPacketError> {
        let payload_len =
            u32::try_from(self.payload.len()).map_err(|_| LocalPacketError::PayloadTooLarge)?;
        let mut bytes = BytesMut::with_capacity(LOCAL_HEADER_LEN + self.payload.len());
        bytes.put_slice(LOCAL_MAGIC);
        bytes.put_u8(PROTOCOL_MAJOR);
        bytes.put_u8(self.packet_type as u8);
        bytes.put_u16_le(u16::try_from(LOCAL_HEADER_LEN).expect("local header fits in u16"));
        bytes.put_u64_le(self.peer);
        bytes.put_u32_le(self.sequence);
        bytes.put_i32_le(self.channel);
        bytes.put_i32_le(self.send_type);
        bytes.put_u32_le(payload_len);
        bytes.put_slice(&self.payload);
        Ok(bytes.freeze())
    }

    pub fn decode(mut bytes: Bytes) -> Result<Self, LocalPacketError> {
        if bytes.len() < LOCAL_HEADER_LEN {
            return Err(LocalPacketError::TooShort);
        }
        let magic = bytes.copy_to_bytes(4);
        if magic.as_ref() != LOCAL_MAGIC {
            return Err(LocalPacketError::BadMagic);
        }
        let version = bytes.get_u8();
        if version != PROTOCOL_MAJOR {
            return Err(LocalPacketError::UnsupportedMajor(version));
        }
        let packet_type = LocalPacketType::try_from(bytes.get_u8())?;
        let header_len = usize::from(bytes.get_u16_le());
        if header_len < LOCAL_HEADER_LEN {
            return Err(LocalPacketError::BadHeaderLength(header_len));
        }
        if bytes.len() + 8 < header_len {
            return Err(LocalPacketError::TooShort);
        }
        let peer = bytes.get_u64_le();
        let sequence = bytes.get_u32_le();
        let channel = bytes.get_i32_le();
        let send_type = bytes.get_i32_le();
        let payload_len =
            usize::try_from(bytes.get_u32_le()).map_err(|_| LocalPacketError::PayloadTooLarge)?;
        let extension_len = header_len - LOCAL_HEADER_LEN;
        if bytes.len() < extension_len + payload_len {
            return Err(LocalPacketError::TooShort);
        }
        bytes.advance(extension_len);
        Ok(Self {
            packet_type,
            peer,
            sequence,
            channel,
            send_type,
            payload: bytes.copy_to_bytes(payload_len),
        })
    }
}

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
    #[must_use]
    pub fn from_local(from_steam_id64: impl Into<String>, packet: LocalPacket) -> Self {
        Self {
            from_steam_id64: from_steam_id64.into(),
            to_steam_id64: packet.peer,
            source_sequence: packet.sequence,
            channel: packet.channel,
            send_type: packet.send_type,
            payload: packet.payload,
        }
    }

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
pub enum LocalPacketError {
    #[error("local packet is too short")]
    TooShort,
    #[error("local packet magic is invalid")]
    BadMagic,
    #[error("unsupported local packet major version {0}")]
    UnsupportedMajor(u8),
    #[error("unknown local packet type {0}")]
    UnknownPacketType(u8),
    #[error("bad local packet header length {0}")]
    BadHeaderLength(usize),
    #[error("local packet payload is too large")]
    PayloadTooLarge,
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
