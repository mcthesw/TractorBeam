//! Fixed binary direct UDP check, heartbeat, and gameplay frames.

use bytes::{Buf as _, BufMut as _, Bytes, BytesMut};
use thiserror::Error;

use super::{
    FRAME_MAGIC, IPV4_UDP_DATAGRAM_BUDGET, InstanceId, PROTOCOL_MAJOR, PROTOCOL_MINOR, PathId,
    PathToken, PeerIdentity, TransactionId,
};

const COMMON_HEADER_LEN: usize = 16;
const PATH_CONTEXT_LEN: usize = 64;
const PATH_HEADER_LEN: usize = COMMON_HEADER_LEN + PATH_CONTEXT_LEN;
pub const CHECK_FRAME_HEADER_LEN: usize = PATH_HEADER_LEN + 24;
pub const HEARTBEAT_FRAME_HEADER_LEN: usize = PATH_HEADER_LEN + 16;
pub const DATA_FRAME_HEADER_LEN: usize = PATH_HEADER_LEN + 20;
pub const DATA_FRAME_OVERHEAD: usize = DATA_FRAME_HEADER_LEN;
pub const MAX_FRAME_LEN: usize = IPV4_UDP_DATAGRAM_BUDGET;
pub const MAX_DATA_PAYLOAD: usize = MAX_FRAME_LEN - DATA_FRAME_HEADER_LEN;
pub const IPV4_SAFE_DATA_PAYLOAD: usize = MAX_DATA_PAYLOAD;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FrameKind {
    Check = 1,
    Heartbeat = 2,
    Data = 3,
}

impl TryFrom<u8> for FrameKind {
    type Error = FrameDecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Check),
            2 => Ok(Self::Heartbeat),
            3 => Ok(Self::Data),
            other => Err(FrameDecodeError::UnknownKind(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CheckPhase {
    Request = 1,
    Response = 2,
}

impl TryFrom<u8> for CheckPhase {
    type Error = FrameDecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Request),
            2 => Ok(Self::Response),
            other => Err(FrameDecodeError::UnknownCheckPhase(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum HeartbeatPhase {
    Request = 1,
    Response = 2,
}

impl TryFrom<u8> for HeartbeatPhase {
    type Error = FrameDecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Request),
            2 => Ok(Self::Response),
            other => Err(FrameDecodeError::UnknownHeartbeatPhase(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathContext {
    pub path_id: PathId,
    pub path_token: PathToken,
    pub from: PeerIdentity,
    pub to_steam_id64: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckFrame {
    pub path: PathContext,
    pub transaction_id: TransactionId,
    pub phase: CheckPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeartbeatFrame {
    pub path: PathContext,
    pub heartbeat_id: u64,
    pub phase: HeartbeatPhase,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataFrame {
    pub path: PathContext,
    pub frame_id: u64,
    pub source_sequence: u32,
    pub channel: i32,
    pub send_type: i32,
    pub payload: Bytes,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectFrame {
    Check(CheckFrame),
    Heartbeat(HeartbeatFrame),
    Data(DataFrame),
}

impl DirectFrame {
    pub fn encode(&self) -> Result<Bytes, FrameEncodeError> {
        match self {
            Self::Check(frame) => frame.encode(),
            Self::Heartbeat(frame) => frame.encode(),
            Self::Data(frame) => frame.encode(),
        }
    }
}

impl CheckFrame {
    pub fn encode(self) -> Result<Bytes, FrameEncodeError> {
        validate_path(self.path)?;
        if self.transaction_id.is_zero() {
            return Err(FrameEncodeError::ZeroTransactionId);
        }
        let mut bytes = BytesMut::with_capacity(CHECK_FRAME_HEADER_LEN);
        put_common_header(&mut bytes, FrameKind::Check, CHECK_FRAME_HEADER_LEN, 0)?;
        put_path(&mut bytes, self.path);
        bytes.put_slice(self.transaction_id.as_bytes());
        bytes.put_u8(self.phase as u8);
        bytes.put_slice(&[0; 7]);
        Ok(bytes.freeze())
    }
}

impl HeartbeatFrame {
    pub fn encode(self) -> Result<Bytes, FrameEncodeError> {
        validate_path(self.path)?;
        if self.heartbeat_id == 0 {
            return Err(FrameEncodeError::ZeroHeartbeatId);
        }
        let mut bytes = BytesMut::with_capacity(HEARTBEAT_FRAME_HEADER_LEN);
        put_common_header(
            &mut bytes,
            FrameKind::Heartbeat,
            HEARTBEAT_FRAME_HEADER_LEN,
            0,
        )?;
        put_path(&mut bytes, self.path);
        bytes.put_u64(self.heartbeat_id);
        bytes.put_u8(self.phase as u8);
        bytes.put_slice(&[0; 7]);
        Ok(bytes.freeze())
    }
}

impl DataFrame {
    pub fn encode(&self) -> Result<Bytes, FrameEncodeError> {
        validate_path(self.path)?;
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
        put_path(&mut bytes, self.path);
        bytes.put_u64(self.frame_id);
        bytes.put_u32(self.source_sequence);
        bytes.put_i32(self.channel);
        bytes.put_i32(self.send_type);
        bytes.put_slice(&self.payload);
        Ok(bytes.freeze())
    }
}

pub fn decode_frame(mut bytes: Bytes) -> Result<DirectFrame, FrameDecodeError> {
    let original_len = bytes.len();
    if original_len < COMMON_HEADER_LEN {
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
        FrameKind::Check => CHECK_FRAME_HEADER_LEN,
        FrameKind::Heartbeat => HEARTBEAT_FRAME_HEADER_LEN,
        FrameKind::Data => DATA_FRAME_HEADER_LEN,
    };
    if header_len < minimum_header {
        return Err(FrameDecodeError::BadHeaderLength(header_len));
    }
    let max_payload = match kind {
        FrameKind::Data => MAX_DATA_PAYLOAD,
        FrameKind::Check | FrameKind::Heartbeat => 0,
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

    let path = read_path(&mut bytes)?;
    match kind {
        FrameKind::Check => {
            let transaction_id = TransactionId::from_bytes(read_array::<16>(&mut bytes)?);
            if transaction_id.is_zero() {
                return Err(FrameDecodeError::ZeroTransactionId);
            }
            let phase = CheckPhase::try_from(bytes.get_u8())?;
            reject_reserved(&mut bytes)?;
            bytes.advance(header_len - CHECK_FRAME_HEADER_LEN);
            Ok(DirectFrame::Check(CheckFrame {
                path,
                transaction_id,
                phase,
            }))
        }
        FrameKind::Heartbeat => {
            let heartbeat_id = bytes.get_u64();
            if heartbeat_id == 0 {
                return Err(FrameDecodeError::ZeroHeartbeatId);
            }
            let phase = HeartbeatPhase::try_from(bytes.get_u8())?;
            reject_reserved(&mut bytes)?;
            bytes.advance(header_len - HEARTBEAT_FRAME_HEADER_LEN);
            Ok(DirectFrame::Heartbeat(HeartbeatFrame {
                path,
                heartbeat_id,
                phase,
            }))
        }
        FrameKind::Data => {
            let frame_id = bytes.get_u64();
            if frame_id == 0 {
                return Err(FrameDecodeError::ZeroFrameId);
            }
            let source_sequence = bytes.get_u32();
            let channel = bytes.get_i32();
            let send_type = bytes.get_i32();
            bytes.advance(header_len - DATA_FRAME_HEADER_LEN);
            Ok(DirectFrame::Data(DataFrame {
                path,
                frame_id,
                source_sequence,
                channel,
                send_type,
                payload: bytes.copy_to_bytes(payload_len),
            }))
        }
    }
}

fn validate_path(path: PathContext) -> Result<(), FrameEncodeError> {
    if path.path_id.is_zero() {
        return Err(FrameEncodeError::ZeroPathId);
    }
    if path.path_token.is_zero() {
        return Err(FrameEncodeError::ZeroPathToken);
    }
    if path.from.steam_id64 == 0 || path.to_steam_id64 == 0 {
        return Err(FrameEncodeError::ZeroSteamId);
    }
    if path.from.instance_id.is_zero() {
        return Err(FrameEncodeError::ZeroInstanceId);
    }
    if path.from.steam_id64 == path.to_steam_id64 {
        return Err(FrameEncodeError::SelfTarget);
    }
    Ok(())
}

fn put_path(bytes: &mut BytesMut, path: PathContext) {
    bytes.put_slice(path.path_id.as_bytes());
    bytes.put_slice(path.path_token.as_bytes());
    bytes.put_u64(path.from.steam_id64);
    bytes.put_slice(path.from.instance_id.as_bytes());
    bytes.put_u64(path.to_steam_id64);
}

fn read_path(bytes: &mut Bytes) -> Result<PathContext, FrameDecodeError> {
    let path_id = PathId::from_bytes(read_array::<16>(bytes)?);
    if path_id.is_zero() {
        return Err(FrameDecodeError::ZeroPathId);
    }
    let path_token = PathToken::from_bytes(read_array::<16>(bytes)?);
    if path_token.is_zero() {
        return Err(FrameDecodeError::ZeroPathToken);
    }
    let steam_id64 = bytes.get_u64();
    if steam_id64 == 0 {
        return Err(FrameDecodeError::ZeroSteamId);
    }
    let instance_id = InstanceId::from_bytes(read_array::<16>(bytes)?);
    if instance_id.is_zero() {
        return Err(FrameDecodeError::ZeroInstanceId);
    }
    let to_steam_id64 = bytes.get_u64();
    if to_steam_id64 == 0 {
        return Err(FrameDecodeError::ZeroSteamId);
    }
    if steam_id64 == to_steam_id64 {
        return Err(FrameDecodeError::SelfTarget);
    }
    Ok(PathContext {
        path_id,
        path_token,
        from: PeerIdentity::new(steam_id64, instance_id),
        to_steam_id64,
    })
}

fn read_array<const N: usize>(bytes: &mut Bytes) -> Result<[u8; N], FrameDecodeError> {
    if bytes.remaining() < N {
        return Err(FrameDecodeError::TooShort);
    }
    let mut value = [0_u8; N];
    bytes.copy_to_slice(&mut value);
    Ok(value)
}

fn reject_reserved(bytes: &mut Bytes) -> Result<(), FrameDecodeError> {
    if bytes.remaining() < 7 {
        return Err(FrameDecodeError::TooShort);
    }
    if bytes[..7].iter().any(|byte| *byte != 0) {
        return Err(FrameDecodeError::NonZeroFrameReserved);
    }
    bytes.advance(7);
    Ok(())
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
    #[error("direct frame header is too large: {0} bytes")]
    HeaderTooLarge(usize),
    #[error("direct frame payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("path id must be non-zero")]
    ZeroPathId,
    #[error("path token must be non-zero")]
    ZeroPathToken,
    #[error("SteamID64 must be non-zero")]
    ZeroSteamId,
    #[error("peer instance id must be non-zero")]
    ZeroInstanceId,
    #[error("direct frame source and target must differ")]
    SelfTarget,
    #[error("transaction id must be non-zero")]
    ZeroTransactionId,
    #[error("heartbeat id must be non-zero")]
    ZeroHeartbeatId,
    #[error("gameplay frame id must be non-zero")]
    ZeroFrameId,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum FrameDecodeError {
    #[error("direct frame is too short")]
    TooShort,
    #[error("direct frame magic is invalid")]
    BadMagic,
    #[error("unsupported direct protocol major version {0}")]
    UnsupportedMajor(u8),
    #[error("unsupported direct protocol minor version {0}")]
    UnsupportedMinor(u8),
    #[error("unknown direct frame kind {0}")]
    UnknownKind(u8),
    #[error("unsupported direct frame flags {0:#x}")]
    UnsupportedFlags(u8),
    #[error("reserved direct frame field is non-zero: {0:#x}")]
    NonZeroReserved(u16),
    #[error("direct frame reserved bytes are non-zero")]
    NonZeroFrameReserved,
    #[error("bad direct frame header length {0}")]
    BadHeaderLength(usize),
    #[error("direct frame payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("direct frame has trailing bytes")]
    TrailingBytes,
    #[error("path id must be non-zero")]
    ZeroPathId,
    #[error("path token must be non-zero")]
    ZeroPathToken,
    #[error("SteamID64 must be non-zero")]
    ZeroSteamId,
    #[error("peer instance id must be non-zero")]
    ZeroInstanceId,
    #[error("direct frame source and target must differ")]
    SelfTarget,
    #[error("transaction id must be non-zero")]
    ZeroTransactionId,
    #[error("heartbeat id must be non-zero")]
    ZeroHeartbeatId,
    #[error("gameplay frame id must be non-zero")]
    ZeroFrameId,
    #[error("unknown path-check phase {0}")]
    UnknownCheckPhase(u8),
    #[error("unknown heartbeat phase {0}")]
    UnknownHeartbeatPhase(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> PathContext {
        PathContext {
            path_id: PathId::from_bytes([1; 16]),
            path_token: PathToken::from_bytes([2; 16]),
            from: PeerIdentity::new(3, InstanceId::from_bytes([4; 16])),
            to_steam_id64: 5,
        }
    }

    #[test]
    fn frame_families_round_trip() {
        let frames = [
            DirectFrame::Check(CheckFrame {
                path: path(),
                transaction_id: TransactionId::from_bytes([6; 16]),
                phase: CheckPhase::Request,
            }),
            DirectFrame::Heartbeat(HeartbeatFrame {
                path: path(),
                heartbeat_id: 7,
                phase: HeartbeatPhase::Response,
            }),
            DirectFrame::Data(DataFrame {
                path: path(),
                frame_id: 8,
                source_sequence: 9,
                channel: -2,
                send_type: 3,
                payload: Bytes::from_static(b"payload"),
            }),
        ];
        for frame in frames {
            let encoded = frame.encode().unwrap();
            assert_eq!(decode_frame(encoded).unwrap(), frame);
        }
    }

    #[test]
    fn data_frame_accepts_maximum_and_rejects_one_byte_more() {
        let mut frame = DataFrame {
            path: path(),
            frame_id: 1,
            source_sequence: 1,
            channel: 0,
            send_type: 0,
            payload: Bytes::from(vec![0; MAX_DATA_PAYLOAD]),
        };
        let encoded = frame.encode().unwrap();
        assert_eq!(encoded.len(), MAX_FRAME_LEN);
        assert_eq!(
            decode_frame(encoded).unwrap(),
            DirectFrame::Data(frame.clone())
        );

        frame.payload = Bytes::from(vec![0; MAX_DATA_PAYLOAD + 1]);
        assert_eq!(
            frame.encode().unwrap_err(),
            FrameEncodeError::PayloadTooLarge(MAX_DATA_PAYLOAD + 1)
        );
    }

    #[test]
    fn decode_rejects_magic_flags_header_and_trailing_bytes() {
        let frame = HeartbeatFrame {
            path: path(),
            heartbeat_id: 1,
            phase: HeartbeatPhase::Request,
        };
        let good = frame.encode().unwrap();

        let mut bad_magic = good.to_vec();
        bad_magic[0] = b'X';
        assert_eq!(
            decode_frame(Bytes::from(bad_magic)).unwrap_err(),
            FrameDecodeError::BadMagic
        );

        let mut flags = good.to_vec();
        flags[7] = 1;
        assert_eq!(
            decode_frame(Bytes::from(flags)).unwrap_err(),
            FrameDecodeError::UnsupportedFlags(1)
        );

        let mut short_header = good.to_vec();
        short_header[8..10].copy_from_slice(&16_u16.to_be_bytes());
        assert_eq!(
            decode_frame(Bytes::from(short_header)).unwrap_err(),
            FrameDecodeError::BadHeaderLength(16)
        );

        let mut trailing = good.to_vec();
        trailing.push(0);
        assert_eq!(
            decode_frame(Bytes::from(trailing)).unwrap_err(),
            FrameDecodeError::TrailingBytes
        );
    }
}
