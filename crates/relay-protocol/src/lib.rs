//! Tractor Beam Relay Protocol wire contract.

mod bootstrap;
mod control;
mod duplicate;
mod frame;

pub use bootstrap::{
    BOOTSTRAP_SCHEMA, BootstrapDecodeError, BootstrapEncodeError, BootstrapMessage, BuildMetadata,
    CapabilityError, CompatibilityReject, MAX_BOOTSTRAP_PAYLOAD, ProtocolRange,
    ProtocolSelectionError, ProtocolVersion, RejectCode, decode_bootstrap, encode_bootstrap,
    select_capabilities, select_protocol,
};
pub use control::{
    ClientControl, ControlDecodeError, ControlEncodeError, ControlErrorCode, DataProfile,
    PeerPresence, PeerPresenceInfo, ResumeRejectCode, SecretString, ServerControl,
    decode_client_control, decode_server_control, encode_client_control, encode_server_control,
};
pub use duplicate::{DuplicateDecision, FrameIdWindow};
pub use frame::{
    COMMON_HEADER_LEN, DATA_FRAME_HEADER_LEN, DATA_FRAME_OVERHEAD, DataFrame, Frame,
    FrameDecodeError, FrameEncodeError, FrameKind, IPV4_SAFE_DATA_PAYLOAD, MAX_CONTROL_PAYLOAD,
    MAX_DATA_PAYLOAD, MAX_FRAME_LEN, PROBE_FRAME_HEADER_LEN, ProbeFrame, ProbePhase, decode_frame,
};

pub const PROTOCOL_MAJOR: u8 = 2;
pub const PROTOCOL_MINOR: u8 = 0;
pub const FRAME_MAGIC: &[u8; 4] = b"TBR2";
pub const IPV4_UDP_DATAGRAM_BUDGET: usize = 1_472;

pub const CAP_TCP_DATA: u64 = 1 << 0;
pub const CAP_UDP_DATA: u64 = 1 << 1;
pub const CAP_RESUME: u64 = 1 << 2;
pub const CAP_ROOM_PATH_PROBE: u64 = 1 << 3;
pub const KNOWN_CAPABILITIES: u64 = CAP_TCP_DATA | CAP_UDP_DATA | CAP_RESUME | CAP_ROOM_PATH_PROBE;
