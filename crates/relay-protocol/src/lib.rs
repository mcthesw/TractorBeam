//! Byte-stable Relay Protocol v1 wire contract.

mod control;
mod envelope;
mod game;

pub use control::{
    ClientMetadata, ControlMessage, ControlMessageError, PeerInfo, PeerTransport, PowChallenge,
    PowProof,
};
pub use envelope::{DecodeError, EncodeError, Envelope, MessageType};
pub use game::{GamePacket, GamePacketError};

pub const PROTOCOL_MAJOR: u8 = 1;
pub const PROTOCOL_MINOR: u8 = 0;
pub const ENVELOPE_MAGIC: &[u8; 4] = b"BBR1";
pub const ENVELOPE_HEADER_LEN: usize = 42;
pub const NONCE_LEN: usize = 12;
pub const GAME_PACKET_MAGIC: &[u8; 4] = b"BBG1";
pub const GAME_PACKET_BASE_HEADER_LEN: usize = 36;
pub const GAME_PACKET_HEADER_LEN: usize = 40;
pub const IPV4_UDP_DATAGRAM_BUDGET: usize = 1_472;
pub const V1_GAME_WIRE_OVERHEAD: usize = ENVELOPE_HEADER_LEN + GAME_PACKET_HEADER_LEN;
pub const IPV4_SAFE_GAME_PAYLOAD: usize = IPV4_UDP_DATAGRAM_BUDGET - V1_GAME_WIRE_OVERHEAD;

pub const CAP_PATH_VALIDATION: u64 = 1 << 0;
pub const CAP_ENCRYPTION_RESERVED: u64 = 1 << 1;
pub const CAP_POW_ADMISSION: u64 = 1 << 2;
pub const CAP_ADMISSION_MATERIAL: u64 = 1 << 3;

#[cfg(test)]
#[path = "golden_tests.rs"]
mod tests;
