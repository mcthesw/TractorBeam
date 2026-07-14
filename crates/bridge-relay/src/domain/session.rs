use std::{fmt, net::SocketAddr, time::Instant};

use super::PeerId;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct SessionKey(pub(crate) [u8; 16]);

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct ResumeKey(pub(crate) [u8; 16]);

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct PathKey(pub(crate) [u8; 16]);

macro_rules! redacted_debug {
    ($name:ident) => {
        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

redacted_debug!(SessionKey);
redacted_debug!(ResumeKey);
redacted_debug!(PathKey);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DataProfile {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Presence {
    Connected,
    Reconnecting,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PeerView {
    pub(crate) steam_id64: u64,
    pub(crate) display_name: Option<String>,
    pub(crate) presence: Presence,
    pub(crate) capabilities: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct JoinBegin {
    pub(crate) control_peer: PeerId,
    pub(crate) session: SessionKey,
    pub(crate) steam_id64: u64,
    pub(crate) display_name: Option<String>,
    pub(crate) profile: DataProfile,
    pub(crate) capabilities: u64,
    pub(crate) now: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct JoinChallenge {
    pub(crate) challenge_id: [u8; 16],
    pub(crate) pow_nonce: [u8; 16],
    pub(crate) difficulty_bits: u8,
}

#[derive(Clone, Debug)]
pub(crate) struct JoinReady {
    pub(crate) connection_id: u64,
    pub(crate) resume_key: ResumeKey,
    pub(crate) peers: Vec<PeerView>,
    pub(crate) profile: DataProfile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResumeFailure {
    UnknownConnection,
    InvalidCredential,
    Expired,
}

#[derive(Clone, Debug)]
pub(crate) struct ResumeReady {
    pub(crate) connection_id: u64,
    pub(crate) peers: Vec<PeerView>,
    pub(crate) profile: DataProfile,
    pub(crate) udp_path_valid: bool,
    pub(crate) broadcast: Option<PresenceBroadcast>,
}

#[derive(Clone, Debug)]
pub(crate) struct PresenceBroadcast {
    pub(crate) recipients: Vec<PeerId>,
    pub(crate) peers: Vec<PeerView>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DataSource {
    Tcp(PeerId),
    Udp(SocketAddr),
}

impl DataSource {
    pub(crate) const fn transport_name(self) -> &'static str {
        match self {
            Self::Tcp(_) => "tcp",
            Self::Udp(_) => "udp",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DataDestination {
    Tcp(PeerId),
    Udp(SocketAddr),
}

impl DataDestination {
    pub(crate) const fn transport_name(self) -> &'static str {
        match self {
            Self::Tcp(_) => "tcp",
            Self::Udp(_) => "udp",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RouteData {
    pub(crate) connection_id: u64,
    pub(crate) frame_id: u64,
    pub(crate) from_steam_id64: u64,
    pub(crate) to_steam_id64: u64,
    pub(crate) source: DataSource,
    pub(crate) frame_bytes: usize,
    pub(crate) now: Instant,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RouteProbe {
    pub(crate) connection_id: u64,
    pub(crate) from_steam_id64: u64,
    pub(crate) to_steam_id64: u64,
    pub(crate) source: DataSource,
    pub(crate) frame_bytes: usize,
    pub(crate) now: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StateError {
    RoomFull,
    RelayFull,
    MissingChallenge,
    InvalidChallenge,
    InvalidProof,
    UnknownConnection,
    SenderMismatch,
    ProfileMismatch,
    PathNotValidated,
    DuplicateFrame,
    FrameTooOld,
    RateLimited,
    TargetNotJoined,
    TargetUnavailable,
    ProbeUnsupported,
    ProbeRateLimited,
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}
