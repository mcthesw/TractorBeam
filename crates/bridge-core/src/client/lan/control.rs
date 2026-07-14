use std::{
    collections::HashSet,
    io,
    net::{IpAddr, SocketAddr, SocketAddrV6},
    sync::Arc,
    time::Duration,
};

use futures_util::{SinkExt as _, StreamExt as _, stream::FuturesUnordered};
use rand::RngExt as _;
use sha2::{Digest as _, Sha256};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    sync::Semaphore,
    task::JoinHandle,
    time,
};
use tokio_util::{codec::LengthDelimitedCodec, sync::CancellationToken};
use tractor_beam_direct_protocol::{
    CAP_HOST_CANDIDATES, ControlErrorCode, ControlMessage, HostCandidate, KNOWN_CAPABILITIES,
    MAX_CANDIDATES, MAX_CONTROL_PAYLOAD, PeerDescriptor, PeerIdentity, ProtocolRange,
    ProtocolVersion, SessionProof, TransactionId, decode_control, encode_control,
    select_capabilities, select_protocol,
};

use super::LanAdapterAddress;
use crate::client::{LanJoinCode, SessionCredential};

const CONTROL_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(1);
const CONTROL_TOTAL_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PENDING_CONTROL_CONNECTIONS: usize = 32;
const SUPPORTED_PROTOCOLS: [ProtocolRange; 1] = [ProtocolRange {
    major: tractor_beam_direct_protocol::PROTOCOL_MAJOR,
    min_minor: 0,
    max_minor: tractor_beam_direct_protocol::PROTOCOL_MINOR,
}];

type ControlStream = tokio_util::codec::Framed<TcpStream, LengthDelimitedCodec>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanProbeResult {
    pub endpoint: SocketAddr,
    pub local_address: SocketAddr,
    pub selected_protocol: ProtocolVersion,
    pub enabled_capabilities: u64,
}

pub struct LanControlPlane {
    shared: Arc<ControlShared>,
    session_credential: SessionCredential,
    control_endpoints: Vec<SocketAddr>,
    _data_sockets: Vec<Arc<UdpSocket>>,
    cancellation: CancellationToken,
    listener_tasks: Vec<JoinHandle<()>>,
}

struct ControlShared {
    descriptor: PeerDescriptor,
    session_proof: SessionProof,
}

impl LanControlPlane {
    pub async fn create(
        identity: PeerIdentity,
        display_name: String,
        session_credential: SessionCredential,
        adapters: &[LanAdapterAddress],
    ) -> io::Result<Self> {
        if adapters.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "at least one LAN adapter address is required",
            ));
        }
        if adapters.len() > MAX_CANDIDATES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("too many LAN adapter addresses: {}", adapters.len()),
            ));
        }

        let mut seen = HashSet::new();
        let mut bound = Vec::with_capacity(adapters.len());
        for (index, adapter) in adapters.iter().enumerate() {
            let bind_address = scoped_address(adapter, 0);
            if !seen.insert(bind_address) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("duplicate LAN adapter address: {}", adapter.address),
                ));
            }
            let listener = TcpListener::bind(bind_address).await?;
            let endpoint = listener.local_addr()?;
            let data_socket = Arc::new(UdpSocket::bind(endpoint).await?);
            let priority = u32::try_from(adapters.len() - index).unwrap_or(1).max(1);
            bound.push((
                listener,
                data_socket,
                HostCandidate::new(endpoint, priority, 0).map_err(io::Error::other)?,
            ));
        }

        let control_endpoints = bound
            .iter()
            .map(|(_, _, candidate)| candidate.endpoint)
            .collect::<Vec<_>>();
        let shared = Arc::new(ControlShared {
            descriptor: PeerDescriptor {
                identity,
                display_name: Some(display_name),
                control_candidates: bound.iter().map(|(_, _, candidate)| *candidate).collect(),
                capabilities: KNOWN_CAPABILITIES,
            },
            session_proof: session_proof(session_credential),
        });
        encode_control(&ControlMessage::PeerSnapshot {
            peers: vec![shared.descriptor.clone()],
        })
        .map_err(io::Error::other)?;

        let cancellation = CancellationToken::new();
        let mut data_sockets = Vec::with_capacity(bound.len());
        let mut listener_tasks = Vec::with_capacity(bound.len());
        for (listener, data_socket, _) in bound {
            data_sockets.push(data_socket);
            listener_tasks.push(tokio::spawn(run_listener(
                listener,
                shared.clone(),
                cancellation.clone(),
            )));
        }

        Ok(Self {
            shared,
            session_credential,
            control_endpoints,
            _data_sockets: data_sockets,
            cancellation,
            listener_tasks,
        })
    }

    #[must_use]
    pub fn invitation(&self) -> LanJoinCode {
        LanJoinCode {
            introducer: self.shared.descriptor.identity,
            control_endpoints: self.control_endpoints.clone(),
            session_credential: self.session_credential,
        }
    }

    #[must_use]
    pub fn descriptor(&self) -> &PeerDescriptor {
        &self.shared.descriptor
    }

    #[must_use]
    pub fn control_endpoints(&self) -> &[SocketAddr] {
        &self.control_endpoints
    }

    pub async fn probe(invitation: &LanJoinCode) -> Vec<LanProbeResult> {
        let mut probes = FuturesUnordered::new();
        for endpoint in invitation.control_endpoints.iter().copied() {
            probes.push(probe_endpoint(
                endpoint,
                invitation.introducer,
                invitation.session_credential,
            ));
        }
        let collect = async move {
            let mut results = Vec::new();
            while let Some(result) = probes.next().await {
                if let Ok(result) = result {
                    results.push(result);
                }
            }
            results.sort_by_key(|result| result.endpoint);
            results
        };
        time::timeout(CONTROL_TOTAL_TIMEOUT, collect)
            .await
            .unwrap_or_default()
    }

    pub async fn shutdown(mut self) {
        self.cancellation.cancel();
        for task in self.listener_tasks.drain(..) {
            let _ = task.await;
        }
    }
}

impl Drop for LanControlPlane {
    fn drop(&mut self) {
        self.cancellation.cancel();
        for task in &self.listener_tasks {
            task.abort();
        }
    }
}

async fn run_listener(
    listener: TcpListener,
    shared: Arc<ControlShared>,
    cancellation: CancellationToken,
) {
    let permits = Arc::new(Semaphore::new(MAX_PENDING_CONTROL_CONNECTIONS));
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    return;
                };
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    continue;
                };
                let shared = shared.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = handle_initial_control(stream, &shared).await;
                });
            }
        }
    }
}

async fn handle_initial_control(stream: TcpStream, shared: &ControlShared) -> io::Result<()> {
    let mut framed = framed(stream);
    let Some(payload) = time::timeout(CONTROL_ATTEMPT_TIMEOUT, framed.next())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LAN control request timed out"))?
    else {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "LAN control connection closed",
        ));
    };
    let message = decode_control(&payload.map_err(io::Error::other)?).map_err(io::Error::other)?;
    let response = match message {
        ControlMessage::ProbeRequest {
            transaction_id,
            introducer,
            supported_protocol_ranges,
            required_capabilities,
            optional_capabilities,
            session_proof,
        } if introducer == shared.descriptor.identity && session_proof == shared.session_proof => {
            let selected_protocol =
                select_protocol(&SUPPORTED_PROTOCOLS, &supported_protocol_ranges)
                    .map_err(io::Error::other)?;
            let enabled_capabilities = select_capabilities(
                required_capabilities,
                optional_capabilities,
                KNOWN_CAPABILITIES,
            )
            .map_err(io::Error::other)?;
            ControlMessage::ProbeResponse {
                transaction_id,
                introducer: shared.descriptor.identity,
                selected_protocol,
                enabled_capabilities,
            }
        }
        ControlMessage::ProbeRequest { .. } => ControlMessage::Error {
            code: ControlErrorCode::InvalidCredential,
            message: "LAN invitation is not valid for this peer".to_owned(),
            retryable: false,
        },
        _ => ControlMessage::JoinRejected {
            code: ControlErrorCode::AdmissionRejected,
            message: "LAN membership admission is not available".to_owned(),
            retryable: false,
        },
    };
    send_control(&mut framed, &response).await
}

async fn probe_endpoint(
    endpoint: SocketAddr,
    introducer: PeerIdentity,
    credential: SessionCredential,
) -> io::Result<LanProbeResult> {
    time::timeout(CONTROL_ATTEMPT_TIMEOUT, async move {
        let stream = TcpStream::connect(endpoint).await?;
        let local_address = stream.local_addr()?;
        let mut framed = framed(stream);
        let transaction_id = TransactionId::from_bytes(nonzero_random());
        send_control(
            &mut framed,
            &ControlMessage::ProbeRequest {
                transaction_id,
                introducer,
                supported_protocol_ranges: SUPPORTED_PROTOCOLS.to_vec(),
                required_capabilities: CAP_HOST_CANDIDATES,
                optional_capabilities: KNOWN_CAPABILITIES,
                session_proof: session_proof(credential),
            },
        )
        .await?;
        let Some(payload) = framed.next().await else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "LAN probe connection closed",
            ));
        };
        match decode_control(&payload.map_err(io::Error::other)?).map_err(io::Error::other)? {
            ControlMessage::ProbeResponse {
                transaction_id: returned,
                introducer: returned_introducer,
                selected_protocol,
                enabled_capabilities,
            } if returned == transaction_id && returned_introducer == introducer => {
                Ok(LanProbeResult {
                    endpoint,
                    local_address,
                    selected_protocol,
                    enabled_capabilities,
                })
            }
            _ => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "LAN probe response did not match the invitation",
            )),
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LAN probe timed out"))?
}

fn framed(stream: TcpStream) -> ControlStream {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_CONTROL_PAYLOAD)
        .new_codec();
    tokio_util::codec::Framed::new(stream, codec)
}

async fn send_control(stream: &mut ControlStream, message: &ControlMessage) -> io::Result<()> {
    let payload = encode_control(message).map_err(io::Error::other)?;
    stream.send(payload).await.map_err(io::Error::other)
}

fn session_proof(credential: SessionCredential) -> SessionProof {
    let mut hash = Sha256::new();
    hash.update(b"tractor-beam-direct-session-proof-v1");
    hash.update(credential.as_bytes());
    SessionProof::from_bytes(hash.finalize().into())
}

fn nonzero_random<const N: usize>() -> [u8; N] {
    loop {
        let value = rand::rng().random::<[u8; N]>();
        if value.iter().any(|byte| *byte != 0) {
            return value;
        }
    }
}

fn scoped_address(adapter: &LanAdapterAddress, port: u16) -> SocketAddr {
    match adapter.address {
        IpAddr::V4(address) => SocketAddr::new(IpAddr::V4(address), port),
        IpAddr::V6(address) => SocketAddrV6::new(address, port, 0, adapter.interface_index).into(),
    }
}

#[cfg(test)]
mod tests {
    use tractor_beam_direct_protocol::InstanceId;

    use super::*;

    fn loopback_adapter(id: u32) -> LanAdapterAddress {
        LanAdapterAddress {
            adapter_id: format!("test-{id}"),
            name: format!("Loopback {id}"),
            address: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            interface_index: id,
        }
    }

    fn identity(id: u8) -> PeerIdentity {
        PeerIdentity::new(u64::from(id), InstanceId::from_bytes([id; 16]))
    }

    #[tokio::test]
    async fn invitation_is_created_only_after_tcp_and_udp_bind() {
        let credential = SessionCredential::from_bytes([7; 16]);
        let room = LanControlPlane::create(
            identity(1),
            "Alice".to_owned(),
            credential,
            &[loopback_adapter(1)],
        )
        .await
        .unwrap();
        let invitation = room.invitation();

        assert_eq!(invitation.introducer, identity(1));
        assert_eq!(invitation.control_endpoints, room.control_endpoints());
        assert_ne!(invitation.control_endpoints[0].port(), 0);
        room.shutdown().await;
    }

    #[tokio::test]
    async fn probe_is_bounded_non_mutating_and_credential_scoped() {
        let credential = SessionCredential::from_bytes([7; 16]);
        let room = LanControlPlane::create(
            identity(1),
            "Alice".to_owned(),
            credential,
            &[loopback_adapter(1)],
        )
        .await
        .unwrap();
        let invitation = room.invitation();

        let results = LanControlPlane::probe(&invitation).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].endpoint, invitation.control_endpoints[0]);
        assert_eq!(room.descriptor().identity, identity(1));

        let mut wrong = invitation;
        wrong.session_credential = SessionCredential::from_bytes([8; 16]);
        assert!(LanControlPlane::probe(&wrong).await.is_empty());
        room.shutdown().await;
    }

    #[tokio::test]
    async fn probe_returns_zero_or_many_results_in_endpoint_order() {
        let credential = SessionCredential::from_bytes([7; 16]);
        let room = LanControlPlane::create(
            identity(1),
            "Alice".to_owned(),
            credential,
            &[
                loopback_adapter(1),
                LanAdapterAddress {
                    adapter_id: "test-2".to_owned(),
                    name: "Loopback 2".to_owned(),
                    address: "127.0.0.2".parse().unwrap(),
                    interface_index: 2,
                },
            ],
        )
        .await
        .unwrap();

        let results = LanControlPlane::probe(&room.invitation()).await;
        assert_eq!(results.len(), 2);
        assert!(results[0].endpoint < results[1].endpoint);

        let unreachable = LanJoinCode {
            introducer: identity(1),
            control_endpoints: vec!["127.0.0.1:1".parse().unwrap()],
            session_credential: credential,
        };
        assert!(LanControlPlane::probe(&unreachable).await.is_empty());
        room.shutdown().await;
    }
}
