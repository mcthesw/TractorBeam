//! Typed, transport-independent Native Hook <-> Bridge Client IPC contract.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const PROTOCOL_MAGIC: [u8; 4] = *b"TBI2";
pub const PROTOCOL_MAJOR: u16 = 2;
pub const PROTOCOL_MINOR: u16 = 0;
pub const FEATURE_GAME_PACKETS: u32 = 1 << 0;
pub const FEATURE_INPUT_DELAY: u32 = 1 << 1;
pub const REQUIRED_FEATURES: u32 = FEATURE_GAME_PACKETS | FEATURE_INPUT_DELAY;
pub const MAX_GAME_PAYLOAD_SIZE: usize = 65_535;
pub const MAX_SERIALIZED_FRAME_LEN: usize = MAX_GAME_PAYLOAD_SIZE + 128;
pub const MAX_ENCODED_FRAME_LEN: usize =
    MAX_SERIALIZED_FRAME_LEN + MAX_SERIALIZED_FRAME_LEN / 254 + 2;
pub const HOOK_DATA_QUEUE_CAPACITY: usize = 1_024;
pub const CLIENT_DATA_QUEUE_CAPACITY: usize = 1_024;
pub const CONTROL_QUEUE_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SessionId([u8; 16]);

impl SessionId {
    #[must_use]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(self) -> [u8; 16] {
        self.0
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        let mut output = String::with_capacity(32);
        for byte in self.0 {
            use fmt::Write as _;
            let _ = write!(output, "{byte:02x}");
        }
        output
    }
}

impl FromStr for SessionId {
    type Err = ProtocolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 32 {
            return Err(ProtocolError::InvalidSessionId);
        }
        let mut bytes = [0_u8; 16];
        for (index, slot) in bytes.iter_mut().enumerate() {
            let offset = index * 2;
            *slot = u8::from_str_radix(&value[offset..offset + 2], 16)
                .map_err(|_| ProtocolError::InvalidSessionId)?;
        }
        Ok(Self(bytes))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PeerRole {
    BridgeClient,
    NativeHook,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Handshake {
    pub magic: [u8; 4],
    pub major: u16,
    pub minor: u16,
    pub role: PeerRole,
    pub session_id: SessionId,
    pub required_features: u32,
    pub max_game_payload: u32,
}

impl Handshake {
    #[must_use]
    pub const fn new(role: PeerRole, session_id: SessionId) -> Self {
        Self {
            magic: PROTOCOL_MAGIC,
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            role,
            session_id,
            required_features: REQUIRED_FEATURES,
            max_game_payload: MAX_GAME_PAYLOAD_SIZE as u32,
        }
    }

    pub fn validate(
        self,
        expected_role: PeerRole,
        expected_session: SessionId,
    ) -> Result<NegotiatedProtocol, ProtocolError> {
        if self.magic != PROTOCOL_MAGIC {
            return Err(ProtocolError::BadMagic);
        }
        if self.major != PROTOCOL_MAJOR {
            return Err(ProtocolError::UnsupportedMajor {
                expected: PROTOCOL_MAJOR,
                actual: self.major,
            });
        }
        if self.role != expected_role {
            return Err(ProtocolError::WrongRole {
                expected: expected_role,
                actual: self.role,
            });
        }
        if self.session_id != expected_session {
            return Err(ProtocolError::SessionMismatch);
        }
        if self.required_features & REQUIRED_FEATURES != REQUIRED_FEATURES {
            return Err(ProtocolError::MissingFeatures {
                required: REQUIRED_FEATURES,
                actual: self.required_features,
            });
        }
        if self.max_game_payload != MAX_GAME_PAYLOAD_SIZE as u32 {
            return Err(ProtocolError::PayloadLimitMismatch {
                expected: MAX_GAME_PAYLOAD_SIZE as u32,
                actual: self.max_game_payload,
            });
        }
        Ok(NegotiatedProtocol {
            major: self.major,
            minor: PROTOCOL_MINOR,
            features: self.required_features,
            max_game_payload: self.max_game_payload,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NegotiatedProtocol {
    pub major: u16,
    pub minor: u16,
    pub features: u32,
    pub max_game_payload: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GamePacket {
    pub peer: u64,
    pub sequence: u32,
    pub channel: i32,
    pub send_type: i32,
    pub payload: Vec<u8>,
}

impl GamePacket {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.payload.len() > MAX_GAME_PAYLOAD_SIZE {
            return Err(ProtocolError::PayloadTooLarge {
                actual: self.payload.len(),
                maximum: MAX_GAME_PAYLOAD_SIZE,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum InputDelayCommand {
    Read,
    Write(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ErrorCode {
    InvalidRequest,
    TargetNotFound,
    ReadFailed,
    WriteFailed,
    InternalError,
    NotConnected,
}

impl ErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::TargetNotFound => "target_not_found",
            Self::ReadFailed => "read_failed",
            Self::WriteFailed => "write_failed",
            Self::InternalError => "internal_error",
            Self::NotConnected => "not_connected",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct IpcHealth {
    pub hook_data_dropped: u64,
    pub client_data_dropped: u64,
    pub malformed_frames: u64,
    pub reconnects: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum HookToClient {
    Handshake(Handshake),
    Ready,
    Game(GamePacket),
    InputDelayResult {
        id: u32,
        result: Result<i32, ErrorCode>,
    },
    Pong {
        id: u32,
    },
    Health(IpcHealth),
    Goodbye,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClientToHook {
    Handshake(Handshake),
    Game(GamePacket),
    InputDelay { id: u32, command: InputDelayCommand },
    Ping { id: u32 },
    Shutdown,
}

pub trait WireMessage: Serialize + DeserializeOwned {
    fn validate_message(&self) -> Result<(), ProtocolError>;
}

impl WireMessage for HookToClient {
    fn validate_message(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Game(packet) => packet.validate(),
            _ => Ok(()),
        }
    }
}

impl WireMessage for ClientToHook {
    fn validate_message(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Game(packet) => packet.validate(),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("bad local IPC protocol magic")]
    BadMagic,
    #[error("unsupported local IPC major version: expected {expected}, got {actual}")]
    UnsupportedMajor { expected: u16, actual: u16 },
    #[error("wrong local IPC peer role: expected {expected:?}, got {actual:?}")]
    WrongRole {
        expected: PeerRole,
        actual: PeerRole,
    },
    #[error("local IPC session identity mismatch")]
    SessionMismatch,
    #[error("local IPC peer is missing required features: required {required:#x}, got {actual:#x}")]
    MissingFeatures { required: u32, actual: u32 },
    #[error("local IPC payload limit mismatch: expected {expected}, got {actual}")]
    PayloadLimitMismatch { expected: u32, actual: u32 },
    #[error("local IPC payload is too large: {actual} bytes, maximum {maximum}")]
    PayloadTooLarge { actual: usize, maximum: usize },
    #[error("local IPC encoded frame is too large: {actual} bytes, maximum {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },
    #[error("local IPC stream ended with an incomplete frame")]
    TruncatedFrame,
    #[error("unexpected local IPC message: {0}")]
    UnexpectedMessage(&'static str),
    #[error("invalid local IPC session identity")]
    InvalidSessionId,
    #[error("local IPC postcard error: {0}")]
    Postcard(#[from] postcard::Error),
}

pub fn encode<T: WireMessage>(message: &T) -> Result<Vec<u8>, ProtocolError> {
    message.validate_message()?;
    let encoded = postcard::to_stdvec_cobs(message)?;
    if encoded.len() > MAX_ENCODED_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge {
            actual: encoded.len(),
            maximum: MAX_ENCODED_FRAME_LEN,
        });
    }
    Ok(encoded)
}

pub fn decode<T: WireMessage>(frame: &mut [u8]) -> Result<T, ProtocolError> {
    if frame.len() > MAX_ENCODED_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge {
            actual: frame.len(),
            maximum: MAX_ENCODED_FRAME_LEN,
        });
    }
    let message: T = postcard::from_bytes_cobs(frame)?;
    message.validate_message()?;
    Ok(message)
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    frame: Vec<u8>,
}

impl FrameDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            frame: Vec::with_capacity(4_096),
        }
    }

    pub fn push<T: WireMessage>(&mut self, input: &[u8]) -> Result<Vec<T>, ProtocolError> {
        let mut messages = Vec::new();
        for &byte in input {
            self.frame.push(byte);
            if self.frame.len() > MAX_ENCODED_FRAME_LEN {
                self.frame.clear();
                return Err(ProtocolError::FrameTooLarge {
                    actual: MAX_ENCODED_FRAME_LEN.saturating_add(1),
                    maximum: MAX_ENCODED_FRAME_LEN,
                });
            }
            if byte == 0 {
                let decoded = decode(&mut self.frame);
                self.frame.clear();
                messages.push(decoded?);
            }
        }
        Ok(messages)
    }

    pub fn finish(&self) -> Result<(), ProtocolError> {
        if self.frame.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError::TruncatedFrame)
        }
    }
}

#[must_use]
pub fn endpoint_name(session_id: SessionId) -> String {
    format!("tractor-beam-{}", session_id.to_hex())
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    const SESSION: SessionId = SessionId::new([0xAB; 16]);

    #[test]
    fn session_id_roundtrips_config_representation() {
        let encoded = SESSION.to_hex();
        assert_eq!(encoded.parse::<SessionId>().unwrap(), SESSION);
        assert_eq!(endpoint_name(SESSION), format!("tractor-beam-{encoded}"));
    }

    #[test]
    fn handshake_accepts_matching_peer_and_rejects_role_version_and_session() {
        let handshake = Handshake::new(PeerRole::NativeHook, SESSION);
        assert_eq!(
            handshake
                .validate(PeerRole::NativeHook, SESSION)
                .unwrap()
                .major,
            PROTOCOL_MAJOR
        );

        let mut wrong_role = handshake;
        wrong_role.role = PeerRole::BridgeClient;
        assert!(matches!(
            wrong_role.validate(PeerRole::NativeHook, SESSION),
            Err(ProtocolError::WrongRole { .. })
        ));
        let mut wrong_version = handshake;
        wrong_version.major = PROTOCOL_MAJOR + 1;
        assert!(matches!(
            wrong_version.validate(PeerRole::NativeHook, SESSION),
            Err(ProtocolError::UnsupportedMajor { .. })
        ));
        assert!(matches!(
            handshake.validate(PeerRole::NativeHook, SessionId::new([0xCD; 16])),
            Err(ProtocolError::SessionMismatch)
        ));
    }

    #[test]
    fn directional_messages_roundtrip_through_incremental_cobs_decoder() {
        let first = HookToClient::Handshake(Handshake::new(PeerRole::NativeHook, SESSION));
        let second = HookToClient::Game(game_packet(40));
        let mut bytes = encode(&first).unwrap();
        bytes.extend(encode(&second).unwrap());
        let split = bytes.len() / 3;
        let mut decoder = FrameDecoder::new();

        let mut decoded = decoder.push::<HookToClient>(&bytes[..split]).unwrap();
        decoded.extend(decoder.push::<HookToClient>(&bytes[split..]).unwrap());

        assert_eq!(decoded, vec![first, second]);
        decoder.finish().unwrap();
    }

    #[test]
    fn malformed_unknown_and_oversized_frames_fail_deterministically() {
        let mut unknown = postcard::to_stdvec_cobs(&u32::MAX).unwrap();
        assert!(matches!(
            decode::<HookToClient>(&mut unknown),
            Err(ProtocolError::Postcard(_))
        ));

        let mut decoder = FrameDecoder::new();
        let oversized = vec![1_u8; MAX_ENCODED_FRAME_LEN + 1];
        assert!(matches!(
            decoder.push::<HookToClient>(&oversized),
            Err(ProtocolError::FrameTooLarge { .. })
        ));

        let mut truncated = FrameDecoder::new();
        truncated.push::<HookToClient>(&[1, 2, 3]).unwrap();
        assert!(matches!(
            truncated.finish(),
            Err(ProtocolError::TruncatedFrame)
        ));
    }

    #[test]
    fn encoded_size_snapshots_are_below_the_legacy_header_for_common_packets() {
        let sizes = [16_usize, 40, 1_100, MAX_GAME_PAYLOAD_SIZE].map(|size| {
            (
                size,
                encode(&HookToClient::Game(game_packet(size)))
                    .unwrap()
                    .len(),
            )
        });

        assert_eq!(
            sizes,
            [(16, 37), (40, 61), (1_100, 1_126), (65_535, 65_816)]
        );
        assert!(sizes[0].1 - sizes[0].0 < 32);
        assert!(sizes[1].1 - sizes[1].0 < 32);
        assert!(sizes[2].1 - sizes[2].0 < 32);
        assert!(sizes[3].1 <= MAX_ENCODED_FRAME_LEN);
    }

    #[test]
    fn codec_throughput_smoke_covers_common_and_maximum_payloads() {
        for (size, iterations) in [
            (16_usize, 10_000),
            (40, 10_000),
            (1_100, 10_000),
            (65_535, 128),
        ] {
            let started = Instant::now();
            for _ in 0..iterations {
                roundtrip_game(size);
            }
            let elapsed = started.elapsed();
            let mebibytes = (size * iterations) as f64 / (1024.0 * 1024.0);
            eprintln!(
                "postcard_cobs_roundtrip payload_bytes={size} iterations={iterations} elapsed_ms={} payload_mib_per_second={:.2}",
                elapsed.as_millis(),
                mebibytes / elapsed.as_secs_f64()
            );
            assert!(elapsed.as_secs() < 10);
        }
    }

    fn roundtrip_game(size: usize) {
        let message = HookToClient::Game(game_packet(size));
        let mut encoded = encode(&message).unwrap();
        let decoded = decode::<HookToClient>(&mut encoded).unwrap();
        assert_eq!(decoded, message);
    }

    fn game_packet(size: usize) -> GamePacket {
        GamePacket {
            peer: u64::MAX,
            sequence: u32::MAX,
            channel: -1,
            send_type: 3,
            payload: vec![0xA5; size],
        }
    }
}
