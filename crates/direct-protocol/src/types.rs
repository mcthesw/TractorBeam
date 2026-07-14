//! Shared direct-protocol identities, candidates, and compatibility values.

use std::{fmt, net::SocketAddr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_CANDIDATES: usize = 8;
pub const MAX_PEERS: usize = 16;
pub const MAX_DISPLAY_NAME_LEN: usize = 64;
pub const MAX_PROTOCOL_RANGES: usize = 4;

pub const CAP_HOST_CANDIDATES: u64 = 1 << 0;
pub const CAP_DIRECT_UDP: u64 = 1 << 1;
pub const CAP_MEMBERSHIP_SNAPSHOT: u64 = 1 << 2;
pub const KNOWN_CAPABILITIES: u64 = CAP_HOST_CANDIDATES | CAP_DIRECT_UDP | CAP_MEMBERSHIP_SNAPSHOT;

macro_rules! opaque_bytes {
    ($name:ident, $length:expr) => {
        #[derive(Clone, Copy, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name([u8; $length]);

        impl $name {
            #[must_use]
            pub const fn from_bytes(bytes: [u8; $length]) -> Self {
                Self(bytes)
            }

            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; $length] {
                &self.0
            }

            #[must_use]
            pub fn is_zero(&self) -> bool {
                self.0.iter().all(|byte| *byte == 0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

opaque_bytes!(InstanceId, 16);
opaque_bytes!(LinkId, 16);
opaque_bytes!(PathId, 16);
opaque_bytes!(PathToken, 16);
opaque_bytes!(TransactionId, 16);
opaque_bytes!(SessionProof, 32);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PeerIdentity {
    pub steam_id64: u64,
    pub instance_id: InstanceId,
}

impl PeerIdentity {
    #[must_use]
    pub const fn new(steam_id64: u64, instance_id: InstanceId) -> Self {
        Self {
            steam_id64,
            instance_id,
        }
    }

    pub(crate) fn validate(self) -> Result<(), CandidateValidationError> {
        if self.steam_id64 == 0 {
            return Err(CandidateValidationError::ZeroSteamId);
        }
        if self.instance_id.is_zero() {
            return Err(CandidateValidationError::ZeroInstanceId);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProtocolVersion {
    pub major: u8,
    pub minor: u8,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProtocolRange {
    pub major: u8,
    pub min_minor: u8,
    pub max_minor: u8,
}

impl ProtocolRange {
    #[must_use]
    pub const fn contains(self, version: ProtocolVersion) -> bool {
        self.major == version.major
            && version.minor >= self.min_minor
            && version.minor <= self.max_minor
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostCandidate {
    pub endpoint: SocketAddr,
    pub priority: u32,
    pub generation: u32,
}

impl HostCandidate {
    pub fn new(
        endpoint: SocketAddr,
        priority: u32,
        generation: u32,
    ) -> Result<Self, CandidateValidationError> {
        let candidate = Self {
            endpoint,
            priority,
            generation,
        };
        candidate.validate()?;
        Ok(candidate)
    }

    pub(crate) fn validate(self) -> Result<(), CandidateValidationError> {
        if self.endpoint.port() == 0 {
            return Err(CandidateValidationError::ZeroPort);
        }
        if self.priority == 0 {
            return Err(CandidateValidationError::ZeroPriority);
        }
        match self.endpoint {
            SocketAddr::V4(endpoint) => {
                let address = endpoint.ip();
                if address.is_unspecified() {
                    return Err(CandidateValidationError::UnspecifiedAddress);
                }
                if address.is_multicast() || address.is_broadcast() {
                    return Err(CandidateValidationError::NonUnicastAddress);
                }
            }
            SocketAddr::V6(endpoint) => {
                let address = endpoint.ip();
                if address.is_unspecified() {
                    return Err(CandidateValidationError::UnspecifiedAddress);
                }
                if address.is_multicast() {
                    return Err(CandidateValidationError::NonUnicastAddress);
                }
                if address.is_unicast_link_local() && endpoint.scope_id() == 0 {
                    return Err(CandidateValidationError::MissingIpv6Scope);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PeerDescriptor {
    pub identity: PeerIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub control_candidates: Vec<HostCandidate>,
    pub capabilities: u64,
}

impl PeerDescriptor {
    pub(crate) fn validate(&self) -> Result<(), CandidateValidationError> {
        self.identity.validate()?;
        if self
            .display_name
            .as_ref()
            .is_some_and(|name| name.len() > MAX_DISPLAY_NAME_LEN)
        {
            return Err(CandidateValidationError::DisplayNameTooLong);
        }
        validate_candidates(&self.control_candidates)?;
        if self.capabilities & !KNOWN_CAPABILITIES != 0 {
            return Err(CandidateValidationError::UnknownCapabilities(
                self.capabilities & !KNOWN_CAPABILITIES,
            ));
        }
        Ok(())
    }
}

pub(crate) fn validate_candidates(
    candidates: &[HostCandidate],
) -> Result<(), CandidateValidationError> {
    if candidates.is_empty() {
        return Err(CandidateValidationError::NoCandidates);
    }
    if candidates.len() > MAX_CANDIDATES {
        return Err(CandidateValidationError::TooManyCandidates(
            candidates.len(),
        ));
    }
    for (index, candidate) in candidates.iter().enumerate() {
        candidate.validate()?;
        if candidates[..index]
            .iter()
            .any(|existing| existing.endpoint == candidate.endpoint)
        {
            return Err(CandidateValidationError::DuplicateCandidate(
                candidate.endpoint,
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_protocol_ranges(
    ranges: &[ProtocolRange],
) -> Result<(), CandidateValidationError> {
    if ranges.is_empty() {
        return Err(CandidateValidationError::NoProtocolRanges);
    }
    if ranges.len() > MAX_PROTOCOL_RANGES {
        return Err(CandidateValidationError::TooManyProtocolRanges(
            ranges.len(),
        ));
    }
    for range in ranges {
        if range.min_minor > range.max_minor {
            return Err(CandidateValidationError::InvalidProtocolRange(*range));
        }
    }
    Ok(())
}

pub fn select_protocol(
    local: &[ProtocolRange],
    remote: &[ProtocolRange],
) -> Result<ProtocolVersion, ProtocolSelectionError> {
    validate_selection_ranges(local)?;
    validate_selection_ranges(remote)?;
    let mut selected = None;
    for local_range in local {
        for remote_range in remote {
            if local_range.major != remote_range.major {
                continue;
            }
            let min_minor = local_range.min_minor.max(remote_range.min_minor);
            let max_minor = local_range.max_minor.min(remote_range.max_minor);
            if min_minor > max_minor {
                continue;
            }
            let candidate = ProtocolVersion {
                major: local_range.major,
                minor: max_minor,
            };
            if selected.is_none_or(|current: ProtocolVersion| {
                (candidate.major, candidate.minor) > (current.major, current.minor)
            }) {
                selected = Some(candidate);
            }
        }
    }
    selected.ok_or(ProtocolSelectionError::NoCommonProtocol)
}

/// Selects capabilities understood and enabled by one direct control pair.
///
/// Unknown optional bits are ignored. Unknown or unavailable required bits are
/// rejected before admission.
pub fn select_capabilities(
    required: u64,
    optional: u64,
    available: u64,
) -> Result<u64, CapabilityError> {
    let available = available & KNOWN_CAPABILITIES;
    let missing = required & !available;
    if missing != 0 {
        return Err(CapabilityError::MissingRequired(missing));
    }
    Ok(required | (optional & available))
}

fn validate_selection_ranges(ranges: &[ProtocolRange]) -> Result<(), ProtocolSelectionError> {
    if ranges.is_empty() || ranges.len() > MAX_PROTOCOL_RANGES {
        return Err(ProtocolSelectionError::InvalidRangeCount(ranges.len()));
    }
    for range in ranges {
        if range.min_minor > range.max_minor {
            return Err(ProtocolSelectionError::InvalidRange(*range));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CandidateValidationError {
    #[error("SteamID64 must be non-zero")]
    ZeroSteamId,
    #[error("peer instance id must be non-zero")]
    ZeroInstanceId,
    #[error("candidate port must be non-zero")]
    ZeroPort,
    #[error("candidate priority must be non-zero")]
    ZeroPriority,
    #[error("candidate address must not be unspecified")]
    UnspecifiedAddress,
    #[error("candidate address must be unicast")]
    NonUnicastAddress,
    #[error("link-local IPv6 candidate requires a scope id")]
    MissingIpv6Scope,
    #[error("at least one candidate is required")]
    NoCandidates,
    #[error("too many candidates: {0}")]
    TooManyCandidates(usize),
    #[error("duplicate candidate endpoint: {0}")]
    DuplicateCandidate(SocketAddr),
    #[error("peer display name is too long")]
    DisplayNameTooLong,
    #[error("unknown capability bits: {0:#x}")]
    UnknownCapabilities(u64),
    #[error("at least one protocol range is required")]
    NoProtocolRanges,
    #[error("too many protocol ranges: {0}")]
    TooManyProtocolRanges(usize),
    #[error("invalid protocol range: {0:?}")]
    InvalidProtocolRange(ProtocolRange),
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ProtocolSelectionError {
    #[error("invalid protocol range count: {0}")]
    InvalidRangeCount(usize),
    #[error("protocol range has minimum minor above maximum minor: {0:?}")]
    InvalidRange(ProtocolRange),
    #[error("peers have no common direct protocol version")]
    NoCommonProtocol,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CapabilityError {
    #[error("required direct capabilities are unavailable: {0:#x}")]
    MissingRequired(u64),
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

    use super::*;

    #[test]
    fn candidate_validation_rejects_invalid_unicast_boundaries() {
        assert_eq!(
            HostCandidate::new(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1).into(), 1, 0)
                .unwrap_err(),
            CandidateValidationError::UnspecifiedAddress
        );
        assert_eq!(
            HostCandidate::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into(), 1, 0).unwrap_err(),
            CandidateValidationError::ZeroPort
        );
        assert_eq!(
            HostCandidate::new(
                SocketAddrV6::new("fe80::1".parse::<Ipv6Addr>().unwrap(), 1, 0, 0).into(),
                1,
                0,
            )
            .unwrap_err(),
            CandidateValidationError::MissingIpv6Scope
        );
    }

    #[test]
    fn scoped_ipv6_and_loopback_are_valid_wire_candidates() {
        assert!(
            HostCandidate::new(
                SocketAddrV6::new("fe80::1".parse().unwrap(), 25910, 0, 7).into(),
                1,
                0,
            )
            .is_ok()
        );
        assert!(
            HostCandidate::new("127.0.0.1:25910".parse().unwrap(), 1, 0).is_ok(),
            "same-process loopback policy belongs to bridge-core"
        );
    }

    #[test]
    fn protocol_selection_chooses_newest_common_version() {
        let local = [ProtocolRange {
            major: 1,
            min_minor: 0,
            max_minor: 2,
        }];
        let remote = [ProtocolRange {
            major: 1,
            min_minor: 1,
            max_minor: 1,
        }];
        assert_eq!(
            select_protocol(&local, &remote).unwrap(),
            ProtocolVersion { major: 1, minor: 1 }
        );
    }

    #[test]
    fn capability_selection_rejects_required_and_ignores_unknown_optional_bits() {
        assert_eq!(
            select_capabilities(
                CAP_DIRECT_UDP,
                CAP_HOST_CANDIDATES | (1 << 63),
                KNOWN_CAPABILITIES,
            )
            .unwrap(),
            CAP_DIRECT_UDP | CAP_HOST_CANDIDATES
        );
        assert_eq!(
            select_capabilities(1 << 63, 0, KNOWN_CAPABILITIES).unwrap_err(),
            CapabilityError::MissingRequired(1 << 63)
        );
    }
}
