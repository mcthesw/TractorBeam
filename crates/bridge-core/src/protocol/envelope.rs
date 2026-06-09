use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

use super::{
    CAP_PATH_VALIDATION, ENVELOPE_HEADER_LEN, ENVELOPE_MAGIC, NONCE_LEN, PROTOCOL_MAJOR,
    PROTOCOL_MINOR,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MessageType {
    Join = 1,
    JoinChallenge = 2,
    JoinReady = 3,
    Data = 4,
    Heartbeat = 5,
    Error = 6,
}

impl TryFrom<u8> for MessageType {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self, DecodeError> {
        match value {
            1 => Ok(Self::Join),
            2 => Ok(Self::JoinChallenge),
            3 => Ok(Self::JoinReady),
            4 => Ok(Self::Data),
            5 => Ok(Self::Heartbeat),
            6 => Ok(Self::Error),
            other => Err(DecodeError::UnknownMessageType(other)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope {
    pub message_type: MessageType,
    pub flags: u8,
    pub capabilities: u64,
    pub sequence: u64,
    pub nonce: [u8; NONCE_LEN],
    pub payload: Bytes,
}

impl Envelope {
    #[must_use]
    pub fn new(message_type: MessageType, payload: impl Into<Bytes>) -> Self {
        Self {
            message_type,
            flags: 0,
            capabilities: CAP_PATH_VALIDATION,
            sequence: 0,
            nonce: [0; NONCE_LEN],
            payload: payload.into(),
        }
    }

    pub fn encode(&self) -> Result<Bytes, EncodeError> {
        let payload_len =
            u32::try_from(self.payload.len()).map_err(|_| EncodeError::PayloadTooLarge)?;
        let header_len =
            u16::try_from(ENVELOPE_HEADER_LEN).map_err(|_| EncodeError::HeaderTooLarge)?;

        let mut bytes = BytesMut::with_capacity(ENVELOPE_HEADER_LEN + self.payload.len());
        bytes.put_slice(ENVELOPE_MAGIC);
        bytes.put_u8(PROTOCOL_MAJOR);
        bytes.put_u8(PROTOCOL_MINOR);
        bytes.put_u8(self.message_type as u8);
        bytes.put_u8(self.flags);
        bytes.put_u16(header_len);
        bytes.put_u32(payload_len);
        bytes.put_u64(self.capabilities);
        bytes.put_u64(self.sequence);
        bytes.put_slice(&self.nonce);
        bytes.put_slice(&self.payload);
        Ok(bytes.freeze())
    }

    pub fn decode(mut bytes: Bytes) -> Result<Self, DecodeError> {
        if bytes.len() < ENVELOPE_HEADER_LEN {
            return Err(DecodeError::TooShort);
        }

        let magic = bytes.copy_to_bytes(4);
        if magic.as_ref() != ENVELOPE_MAGIC {
            return Err(DecodeError::BadMagic);
        }

        let major = bytes.get_u8();
        if major != PROTOCOL_MAJOR {
            return Err(DecodeError::UnsupportedMajor(major));
        }
        let _minor = bytes.get_u8();
        let message_type = MessageType::try_from(bytes.get_u8())?;
        let flags = bytes.get_u8();
        let header_len = usize::from(bytes.get_u16());
        if header_len < ENVELOPE_HEADER_LEN {
            return Err(DecodeError::BadHeaderLength(header_len));
        }
        let payload_len =
            usize::try_from(bytes.get_u32()).map_err(|_| DecodeError::PayloadTooLarge)?;
        let capabilities = bytes.get_u64();
        let sequence = bytes.get_u64();
        let mut nonce = [0; NONCE_LEN];
        bytes.copy_to_slice(&mut nonce);

        let extension_len = header_len - ENVELOPE_HEADER_LEN;
        if bytes.len() < extension_len + payload_len {
            return Err(DecodeError::TooShort);
        }
        bytes.advance(extension_len);
        let payload = bytes.copy_to_bytes(payload_len);

        Ok(Self {
            message_type,
            flags,
            capabilities,
            sequence,
            nonce,
            payload,
        })
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum EncodeError {
    #[error("envelope header is too large")]
    HeaderTooLarge,
    #[error("envelope payload is too large")]
    PayloadTooLarge,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DecodeError {
    #[error("envelope is too short")]
    TooShort,
    #[error("envelope magic is invalid")]
    BadMagic,
    #[error("unsupported protocol major version {0}")]
    UnsupportedMajor(u8),
    #[error("unknown message type {0}")]
    UnknownMessageType(u8),
    #[error("bad header length {0}")]
    BadHeaderLength(usize),
    #[error("payload length overflow")]
    PayloadTooLarge,
}
