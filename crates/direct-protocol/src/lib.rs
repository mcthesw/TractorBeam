//! Tractor Beam direct Peer protocol wire contract.

mod control;
mod frame;
mod types;

pub use control::{
    ControlDecodeError, ControlEncodeError, ControlErrorCode, ControlMessage,
    ControlValidationError, decode_control, encode_control,
};
pub use frame::{
    CHECK_FRAME_HEADER_LEN, CheckFrame, CheckPhase, DATA_FRAME_HEADER_LEN, DATA_FRAME_OVERHEAD,
    DataFrame, DirectFrame, FrameDecodeError, FrameEncodeError, FrameKind,
    HEARTBEAT_FRAME_HEADER_LEN, HeartbeatFrame, HeartbeatPhase, IPV4_SAFE_DATA_PAYLOAD,
    MAX_DATA_PAYLOAD, MAX_FRAME_LEN, PathContext, decode_frame,
};
pub use types::{
    CAP_DIRECT_UDP, CAP_HOST_CANDIDATES, CAP_MEMBERSHIP_SNAPSHOT, CandidateValidationError,
    CapabilityError, HostCandidate, InstanceId, KNOWN_CAPABILITIES, LinkId, MAX_CANDIDATES,
    MAX_DISPLAY_NAME_LEN, MAX_PEERS, MAX_PROTOCOL_RANGES, PathId, PathToken, PeerDescriptor,
    PeerIdentity, ProtocolRange, ProtocolSelectionError, ProtocolVersion, SessionProof,
    TransactionId, select_capabilities, select_protocol,
};

pub const PROTOCOL_MAJOR: u8 = 1;
pub const PROTOCOL_MINOR: u8 = 0;
pub const FRAME_MAGIC: &[u8; 4] = b"TBD1";
pub const IPV4_UDP_DATAGRAM_BUDGET: usize = 1_472;
pub const MAX_CONTROL_PAYLOAD: usize = 16 * 1024;
