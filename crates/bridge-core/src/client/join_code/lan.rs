//! Bounded LAN Direct Join Code codec.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use tractor_beam_direct_protocol::{HostCandidate, InstanceId, MAX_CANDIDATES, PeerIdentity};

use super::{
    CHECKSUM_LEN, JOIN_CODE_MAGIC, JoinCodeError, LanJoinCode, SessionCredential, append_checksum,
    validate_checksum,
};

pub(super) const LAN_JOIN_CODE_VERSION: u8 = 6;
const LAN_DIRECT_ROUTE: u8 = 1;
const FIXED_PAYLOAD_LEN: usize = 48;
const CANDIDATE_LEN: usize = 24;
const MAX_LAN_PAYLOAD_LEN: usize = 256;
const IPV4_FAMILY: u8 = 4;
const IPV6_FAMILY: u8 = 6;

pub(super) fn encode_payload(code: &LanJoinCode) -> Result<Vec<u8>, JoinCodeError> {
    validate_identity(code.introducer)?;
    validate_endpoints(&code.control_endpoints)?;
    let candidate_count = u8::try_from(code.control_endpoints.len())
        .map_err(|_| JoinCodeError::TooManyLanCandidates(code.control_endpoints.len()))?;
    let expected_len =
        FIXED_PAYLOAD_LEN + CANDIDATE_LEN * code.control_endpoints.len() + CHECKSUM_LEN;
    if expected_len > MAX_LAN_PAYLOAD_LEN {
        return Err(JoinCodeError::PayloadTooLarge(expected_len));
    }

    let mut payload = Vec::with_capacity(expected_len);
    payload.extend_from_slice(JOIN_CODE_MAGIC);
    payload.push(LAN_JOIN_CODE_VERSION);
    payload.push(LAN_DIRECT_ROUTE);
    payload.push(0);
    payload.push(candidate_count);
    payload.extend_from_slice(&[0; 2]);
    payload.extend_from_slice(code.session_credential.as_bytes());
    payload.extend_from_slice(&code.introducer.steam_id64.to_be_bytes());
    payload.extend_from_slice(code.introducer.instance_id.as_bytes());
    for endpoint in &code.control_endpoints {
        encode_endpoint(&mut payload, *endpoint)?;
    }
    append_checksum(&mut payload);
    Ok(payload)
}

pub(super) fn decode_payload(bytes: &[u8]) -> Result<LanJoinCode, JoinCodeError> {
    if bytes.len() > MAX_LAN_PAYLOAD_LEN {
        return Err(JoinCodeError::PayloadTooLarge(bytes.len()));
    }
    if bytes.len() < FIXED_PAYLOAD_LEN + CANDIDATE_LEN + CHECKSUM_LEN {
        return Err(JoinCodeError::Truncated);
    }
    if bytes[3] != LAN_DIRECT_ROUTE {
        return Err(JoinCodeError::UnsupportedRoute(bytes[3]));
    }
    if bytes[4] != 0 {
        return Err(JoinCodeError::UnsupportedFlags(bytes[4]));
    }
    if bytes[6..8].iter().any(|byte| *byte != 0) {
        return Err(JoinCodeError::NonZeroReserved);
    }
    let candidate_count = usize::from(bytes[5]);
    if candidate_count == 0 {
        return Err(JoinCodeError::MissingLanCandidates);
    }
    if candidate_count > MAX_CANDIDATES {
        return Err(JoinCodeError::TooManyLanCandidates(candidate_count));
    }
    let expected_len = FIXED_PAYLOAD_LEN
        .checked_add(CANDIDATE_LEN * candidate_count)
        .and_then(|length| length.checked_add(CHECKSUM_LEN))
        .ok_or(JoinCodeError::PayloadTooLarge(bytes.len()))?;
    if bytes.len() < expected_len {
        return Err(JoinCodeError::Truncated);
    }
    if bytes.len() != expected_len {
        return Err(JoinCodeError::TrailingBytes);
    }
    validate_checksum(bytes, expected_len - CHECKSUM_LEN)?;

    let mut credential = [0_u8; 16];
    credential.copy_from_slice(&bytes[8..24]);
    let steam_id64 = u64::from_be_bytes(
        bytes[24..32]
            .try_into()
            .expect("SteamID64 slice length is fixed"),
    );
    let mut instance_id = [0_u8; 16];
    instance_id.copy_from_slice(&bytes[32..FIXED_PAYLOAD_LEN]);
    let introducer = PeerIdentity::new(steam_id64, InstanceId::from_bytes(instance_id));
    validate_identity(introducer)?;

    let content_len = expected_len - CHECKSUM_LEN;
    let mut control_endpoints = Vec::with_capacity(candidate_count);
    for candidate in bytes[FIXED_PAYLOAD_LEN..content_len].chunks_exact(CANDIDATE_LEN) {
        control_endpoints.push(decode_endpoint(candidate)?);
    }
    validate_endpoints(&control_endpoints)?;
    Ok(LanJoinCode {
        introducer,
        control_endpoints,
        session_credential: SessionCredential::from_bytes(credential),
    })
}

fn encode_endpoint(payload: &mut Vec<u8>, endpoint: SocketAddr) -> Result<(), JoinCodeError> {
    match endpoint {
        SocketAddr::V4(endpoint) => {
            payload.push(IPV4_FAMILY);
            payload.push(0);
            payload.extend_from_slice(&endpoint.port().to_be_bytes());
            payload.extend_from_slice(&0_u32.to_be_bytes());
            payload.extend_from_slice(&endpoint.ip().octets());
            payload.extend_from_slice(&[0; 12]);
        }
        SocketAddr::V6(endpoint) => {
            if endpoint.flowinfo() != 0 {
                return Err(JoinCodeError::UnsupportedIpv6FlowInfo);
            }
            payload.push(IPV6_FAMILY);
            payload.push(0);
            payload.extend_from_slice(&endpoint.port().to_be_bytes());
            payload.extend_from_slice(&endpoint.scope_id().to_be_bytes());
            payload.extend_from_slice(&endpoint.ip().octets());
        }
    }
    Ok(())
}

fn decode_endpoint(bytes: &[u8]) -> Result<SocketAddr, JoinCodeError> {
    if bytes[1] != 0 {
        return Err(JoinCodeError::NonZeroCandidateReserved);
    }
    let port = u16::from_be_bytes([bytes[2], bytes[3]]);
    let scope_id = u32::from_be_bytes(
        bytes[4..8]
            .try_into()
            .expect("candidate scope slice length is fixed"),
    );
    let endpoint = match bytes[0] {
        IPV4_FAMILY => {
            if scope_id != 0 || bytes[12..CANDIDATE_LEN].iter().any(|byte| *byte != 0) {
                return Err(JoinCodeError::NonZeroCandidateReserved);
            }
            SocketAddrV4::new(
                Ipv4Addr::new(bytes[8], bytes[9], bytes[10], bytes[11]),
                port,
            )
            .into()
        }
        IPV6_FAMILY => {
            let address = Ipv6Addr::from(
                <[u8; 16]>::try_from(&bytes[8..CANDIDATE_LEN])
                    .expect("IPv6 candidate slice length is fixed"),
            );
            SocketAddrV6::new(address, port, 0, scope_id).into()
        }
        family => return Err(JoinCodeError::UnsupportedAddressFamily(family)),
    };
    Ok(endpoint)
}

fn validate_identity(identity: PeerIdentity) -> Result<(), JoinCodeError> {
    if identity.steam_id64 == 0 {
        return Err(JoinCodeError::ZeroIntroducerSteamId);
    }
    if identity.instance_id.is_zero() {
        return Err(JoinCodeError::ZeroIntroducerInstanceId);
    }
    Ok(())
}

fn validate_endpoints(endpoints: &[SocketAddr]) -> Result<(), JoinCodeError> {
    if endpoints.is_empty() {
        return Err(JoinCodeError::MissingLanCandidates);
    }
    if endpoints.len() > MAX_CANDIDATES {
        return Err(JoinCodeError::TooManyLanCandidates(endpoints.len()));
    }
    for (index, endpoint) in endpoints.iter().enumerate() {
        HostCandidate::new(*endpoint, 1, 0)?;
        if endpoints[..index].contains(endpoint) {
            return Err(JoinCodeError::DuplicateLanCandidate(*endpoint));
        }
        if let SocketAddr::V6(endpoint) = endpoint
            && endpoint.flowinfo() != 0
        {
            return Err(JoinCodeError::UnsupportedIpv6FlowInfo);
        }
    }
    Ok(())
}
