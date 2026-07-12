use bytes::{Buf as _, BufMut as _, Bytes, BytesMut};
use thiserror::Error;

use super::{FRAME_MAGIC, IPV4_UDP_DATAGRAM_BUDGET, PROTOCOL_MAJOR, PROTOCOL_MINOR};

pub const COMMON_HEADER_LEN: usize = 16;
pub const DATA_FRAME_HEADER_LEN: usize = 60;
pub const DATA_FRAME_OVERHEAD: usize = DATA_FRAME_HEADER_LEN;
pub const PROBE_FRAME_HEADER_LEN: usize = 56;
pub const MAX_CONTROL_PAYLOAD: usize = 16 * 1024;
pub const MAX_FRAME_LEN: usize = IPV4_UDP_DATAGRAM_BUDGET;
pub const MAX_DATA_PAYLOAD: usize = MAX_FRAME_LEN - DATA_FRAME_HEADER_LEN;
pub const IPV4_SAFE_DATA_PAYLOAD: usize = MAX_DATA_PAYLOAD;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FrameKind {
    ClientControl = 1,
    ServerControl = 2,
    Data = 3,
    Probe = 4,
}

impl TryFrom<u8> for FrameKind {
    type Error = FrameDecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::ClientControl),
            2 => Ok(Self::ServerControl),
            3 => Ok(Self::Data),
            4 => Ok(Self::Probe),
            other => Err(FrameDecodeError::UnknownKind(other)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataFrame {
    pub connection_id: u64,
    pub frame_id: u64,
    pub from_steam_id64: u64,
    pub to_steam_id64: u64,
    pub source_sequence: u32,
    pub channel: i32,
    pub send_type: i32,
    pub payload: Bytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ProbePhase {
    Request = 1,
    Echo = 2,
}

impl TryFrom<u8> for ProbePhase {
    type Error = FrameDecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Request),
            2 => Ok(Self::Echo),
            other => Err(FrameDecodeError::UnknownProbePhase(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeFrame {
    pub connection_id: u64,
    pub probe_id: u64,
    pub from_steam_id64: u64,
    pub to_steam_id64: u64,
    pub phase: ProbePhase,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    ClientControl(Bytes),
    ServerControl(Bytes),
    Data(DataFrame),
    Probe(ProbeFrame),
}

impl Frame {
    pub fn encode(&self) -> Result<Bytes, FrameEncodeError> {
        match self {
            Self::ClientControl(payload) => encode_common(FrameKind::ClientControl, payload),
            Self::ServerControl(payload) => encode_common(FrameKind::ServerControl, payload),
            Self::Data(frame) => frame.encode(),
            Self::Probe(frame) => frame.encode(),
        }
    }
}

impl ProbeFrame {
    pub fn encode(self) -> Result<Bytes, FrameEncodeError> {
        if self.connection_id == 0 {
            return Err(FrameEncodeError::ZeroConnectionId);
        }
        if self.probe_id == 0 {
            return Err(FrameEncodeError::ZeroProbeId);
        }
        let mut bytes = BytesMut::with_capacity(PROBE_FRAME_HEADER_LEN);
        put_common_header(&mut bytes, FrameKind::Probe, PROBE_FRAME_HEADER_LEN, 0)?;
        bytes.put_u64(self.connection_id);
        bytes.put_u64(self.probe_id);
        bytes.put_u64(self.from_steam_id64);
        bytes.put_u64(self.to_steam_id64);
        bytes.put_u8(self.phase as u8);
        bytes.put_slice(&[0; 7]);
        Ok(bytes.freeze())
    }
}

impl DataFrame {
    pub fn encode(&self) -> Result<Bytes, FrameEncodeError> {
        if self.connection_id == 0 {
            return Err(FrameEncodeError::ZeroConnectionId);
        }
        if self.frame_id == 0 {
            return Err(FrameEncodeError::ZeroFrameId);
        }
        if self.payload.len() > MAX_DATA_PAYLOAD {
            return Err(FrameEncodeError::PayloadTooLarge(self.payload.len()));
        }
        let payload_len = u32::try_from(self.payload.len())
            .map_err(|_| FrameEncodeError::PayloadTooLarge(self.payload.len()))?;
        let mut bytes = BytesMut::with_capacity(DATA_FRAME_HEADER_LEN + self.payload.len());
        put_common_header(
            &mut bytes,
            FrameKind::Data,
            DATA_FRAME_HEADER_LEN,
            payload_len,
        )?;
        bytes.put_u64(self.connection_id);
        bytes.put_u64(self.frame_id);
        bytes.put_u64(self.from_steam_id64);
        bytes.put_u64(self.to_steam_id64);
        bytes.put_u32(self.source_sequence);
        bytes.put_i32(self.channel);
        bytes.put_i32(self.send_type);
        bytes.put_slice(&self.payload);
        Ok(bytes.freeze())
    }
}

pub fn decode_frame(mut bytes: Bytes) -> Result<Frame, FrameDecodeError> {
    let original_len = bytes.len();
    if bytes.len() < COMMON_HEADER_LEN {
        return Err(FrameDecodeError::TooShort);
    }
    if &bytes[..4] != FRAME_MAGIC {
        return Err(FrameDecodeError::BadMagic);
    }
    bytes.advance(4);
    let major = bytes.get_u8();
    let minor = bytes.get_u8();
    if major != PROTOCOL_MAJOR {
        return Err(FrameDecodeError::UnsupportedMajor(major));
    }
    if minor > PROTOCOL_MINOR {
        return Err(FrameDecodeError::UnsupportedMinor(minor));
    }
    let kind = FrameKind::try_from(bytes.get_u8())?;
    let flags = bytes.get_u8();
    if flags != 0 {
        return Err(FrameDecodeError::UnsupportedFlags(flags));
    }
    let header_len = usize::from(bytes.get_u16());
    let reserved = bytes.get_u16();
    if reserved != 0 {
        return Err(FrameDecodeError::NonZeroReserved(reserved));
    }
    let payload_len = usize::try_from(bytes.get_u32())
        .map_err(|_| FrameDecodeError::PayloadTooLarge(usize::MAX))?;
    let minimum_header = match kind {
        FrameKind::Data => DATA_FRAME_HEADER_LEN,
        FrameKind::Probe => PROBE_FRAME_HEADER_LEN,
        FrameKind::ClientControl | FrameKind::ServerControl => COMMON_HEADER_LEN,
    };
    if header_len < minimum_header {
        return Err(FrameDecodeError::BadHeaderLength(header_len));
    }
    let max_payload = match kind {
        FrameKind::Data => MAX_DATA_PAYLOAD,
        FrameKind::Probe => 0,
        FrameKind::ClientControl | FrameKind::ServerControl => MAX_CONTROL_PAYLOAD,
    };
    if payload_len > max_payload {
        return Err(FrameDecodeError::PayloadTooLarge(payload_len));
    }
    let total_len = header_len
        .checked_add(payload_len)
        .ok_or(FrameDecodeError::PayloadTooLarge(payload_len))?;
    if original_len < total_len {
        return Err(FrameDecodeError::TooShort);
    }
    if original_len != total_len {
        return Err(FrameDecodeError::TrailingBytes);
    }

    let extension_len = header_len - COMMON_HEADER_LEN;
    match kind {
        FrameKind::ClientControl | FrameKind::ServerControl => {
            bytes.advance(extension_len);
            let payload = bytes.copy_to_bytes(payload_len);
            Ok(match kind {
                FrameKind::ClientControl => Frame::ClientControl(payload),
                FrameKind::ServerControl => Frame::ServerControl(payload),
                FrameKind::Data => unreachable!("data handled in separate branch"),
                FrameKind::Probe => unreachable!("probe handled in separate branch"),
            })
        }
        FrameKind::Data => {
            let connection_id = bytes.get_u64();
            let frame_id = bytes.get_u64();
            let from_steam_id64 = bytes.get_u64();
            let to_steam_id64 = bytes.get_u64();
            let source_sequence = bytes.get_u32();
            let channel = bytes.get_i32();
            let send_type = bytes.get_i32();
            if connection_id == 0 {
                return Err(FrameDecodeError::ZeroConnectionId);
            }
            if frame_id == 0 {
                return Err(FrameDecodeError::ZeroFrameId);
            }
            bytes.advance(header_len - DATA_FRAME_HEADER_LEN);
            Ok(Frame::Data(DataFrame {
                connection_id,
                frame_id,
                from_steam_id64,
                to_steam_id64,
                source_sequence,
                channel,
                send_type,
                payload: bytes.copy_to_bytes(payload_len),
            }))
        }
        FrameKind::Probe => {
            let connection_id = bytes.get_u64();
            let probe_id = bytes.get_u64();
            let from_steam_id64 = bytes.get_u64();
            let to_steam_id64 = bytes.get_u64();
            let phase = ProbePhase::try_from(bytes.get_u8())?;
            if connection_id == 0 {
                return Err(FrameDecodeError::ZeroConnectionId);
            }
            if probe_id == 0 {
                return Err(FrameDecodeError::ZeroProbeId);
            }
            if bytes[..7].iter().any(|byte| *byte != 0) {
                return Err(FrameDecodeError::NonZeroProbeReserved);
            }
            bytes.advance(7);
            bytes.advance(header_len - PROBE_FRAME_HEADER_LEN);
            Ok(Frame::Probe(ProbeFrame {
                connection_id,
                probe_id,
                from_steam_id64,
                to_steam_id64,
                phase,
            }))
        }
    }
}

fn encode_common(kind: FrameKind, payload: &Bytes) -> Result<Bytes, FrameEncodeError> {
    if payload.len() > MAX_CONTROL_PAYLOAD {
        return Err(FrameEncodeError::PayloadTooLarge(payload.len()));
    }
    let payload_len = u32::try_from(payload.len())
        .map_err(|_| FrameEncodeError::PayloadTooLarge(payload.len()))?;
    let mut bytes = BytesMut::with_capacity(COMMON_HEADER_LEN + payload.len());
    put_common_header(&mut bytes, kind, COMMON_HEADER_LEN, payload_len)?;
    bytes.put_slice(payload);
    Ok(bytes.freeze())
}

fn put_common_header(
    bytes: &mut BytesMut,
    kind: FrameKind,
    header_len: usize,
    payload_len: u32,
) -> Result<(), FrameEncodeError> {
    let header_len =
        u16::try_from(header_len).map_err(|_| FrameEncodeError::HeaderTooLarge(header_len))?;
    bytes.put_slice(FRAME_MAGIC);
    bytes.put_u8(PROTOCOL_MAJOR);
    bytes.put_u8(PROTOCOL_MINOR);
    bytes.put_u8(kind as u8);
    bytes.put_u8(0);
    bytes.put_u16(header_len);
    bytes.put_u16(0);
    bytes.put_u32(payload_len);
    Ok(())
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum FrameEncodeError {
    #[error("frame header is too large: {0} bytes")]
    HeaderTooLarge(usize),
    #[error("frame payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("data frame connection id must be non-zero")]
    ZeroConnectionId,
    #[error("data frame id must be non-zero")]
    ZeroFrameId,
    #[error("probe id must be non-zero")]
    ZeroProbeId,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum FrameDecodeError {
    #[error("frame is too short")]
    TooShort,
    #[error("frame magic is invalid")]
    BadMagic,
    #[error("unsupported protocol major version {0}")]
    UnsupportedMajor(u8),
    #[error("unsupported protocol minor version {0}")]
    UnsupportedMinor(u8),
    #[error("unknown frame kind {0}")]
    UnknownKind(u8),
    #[error("unsupported frame flags {0:#x}")]
    UnsupportedFlags(u8),
    #[error("reserved frame field is non-zero: {0:#x}")]
    NonZeroReserved(u16),
    #[error("bad frame header length {0}")]
    BadHeaderLength(usize),
    #[error("frame payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("frame has trailing bytes")]
    TrailingBytes,
    #[error("data frame connection id must be non-zero")]
    ZeroConnectionId,
    #[error("data frame id must be non-zero")]
    ZeroFrameId,
    #[error("probe id must be non-zero")]
    ZeroProbeId,
    #[error("unknown probe phase {0}")]
    UnknownProbePhase(u8),
    #[error("probe reserved field is non-zero")]
    NonZeroProbeReserved,
}
