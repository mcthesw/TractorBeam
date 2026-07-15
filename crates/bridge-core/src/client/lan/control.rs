use std::{
    collections::HashSet,
    io,
    net::{IpAddr, SocketAddr, SocketAddrV6},
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt as _, StreamExt as _, stream::FuturesUnordered};
use rand::RngExt as _;
use sha2::{Digest as _, Sha256};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Semaphore, mpsc},
    task::JoinHandle,
    time,
};
use tokio_util::{codec::LengthDelimitedCodec, sync::CancellationToken};
#[cfg(test)]
use tractor_beam_direct_protocol::LinkId;
use tractor_beam_direct_protocol::{
    CAP_HOST_CANDIDATES, ControlErrorCode, ControlMessage, HostCandidate, KNOWN_CAPABILITIES,
    MAX_CANDIDATES, MAX_CONTROL_PAYLOAD, PeerDescriptor, PeerIdentity, ProtocolRange,
    ProtocolVersion, SessionProof, TransactionId, decode_control, encode_control,
    select_capabilities, select_protocol,
};

use super::{
    LanAdapterAddress,
    link::{
        accept_inbound_join, establish_outbound, local_descriptor, membership_links,
        run_membership_dialer,
    },
    membership::Membership,
    path::{LanPeerPathState, PathManager},
};
use crate::client::{LanJoinCode, SessionCredential};

const CONTROL_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(1);
pub(super) const CONTROL_TOTAL_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PENDING_CONTROL_CONNECTIONS: usize = 32;
pub(super) const SUPPORTED_PROTOCOLS: [ProtocolRange; 1] = [ProtocolRange {
    major: tractor_beam_direct_protocol::PROTOCOL_MAJOR,
    min_minor: 0,
    max_minor: tractor_beam_direct_protocol::PROTOCOL_MINOR,
}];

pub(super) type ControlStream = tokio_util::codec::Framed<TcpStream, LengthDelimitedCodec>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanProbeResult {
    pub endpoint: SocketAddr,
    pub local_address: SocketAddr,
    pub selected_protocol: ProtocolVersion,
    pub enabled_capabilities: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LanPeerConnectionState {
    Discovered,
    Connected,
    Reconnecting,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanPeerState {
    pub peer: PeerDescriptor,
    pub connection: LanPeerConnectionState,
}

pub struct LanControlPlane {
    shared: Arc<ControlShared>,
    session_credential: SessionCredential,
    control_endpoints: Vec<SocketAddr>,
    cancellation: CancellationToken,
    listener_tasks: Vec<JoinHandle<()>>,
    background_tasks: Vec<JoinHandle<()>>,
}

pub(super) struct ControlShared {
    pub session_proof: SessionProof,
    pub membership: Mutex<Membership>,
    pub cancellation: CancellationToken,
    pub dial_tx: mpsc::UnboundedSender<PeerDescriptor>,
    pub paths: Arc<PathManager>,
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
            let data_socket = Arc::new(UdpSocket::bind(bind_address).await?);
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
        let descriptor = PeerDescriptor {
            identity,
            display_name: Some(display_name),
            control_candidates: bound.iter().map(|(_, _, candidate)| *candidate).collect(),
            capabilities: KNOWN_CAPABILITIES,
        };
        encode_control(&ControlMessage::PeerSnapshot {
            peers: vec![descriptor.clone()],
        })
        .map_err(io::Error::other)?;

        let cancellation = CancellationToken::new();
        let path_sockets = bound
            .iter()
            .enumerate()
            .map(|(index, (_, socket, _))| {
                let priority = u32::try_from(adapters.len() - index).unwrap_or(1).max(1);
                (socket.clone(), priority)
            })
            .collect();
        let paths = PathManager::new(identity, path_sockets, cancellation.clone()).await?;
        let (dial_tx, dial_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(ControlShared {
            session_proof: session_proof(session_credential),
            membership: Mutex::new(Membership::new(descriptor)),
            cancellation: cancellation.clone(),
            dial_tx,
            paths: paths.clone(),
        });
        let mut listener_tasks = Vec::with_capacity(bound.len());
        for (listener, data_socket, _) in bound {
            drop(data_socket);
            listener_tasks.push(tokio::spawn(run_listener(listener, shared.clone())));
        }
        let mut background_tasks = paths.start();
        background_tasks.push(tokio::spawn(run_membership_dialer(shared.clone(), dial_rx)));

        Ok(Self {
            shared,
            session_credential,
            control_endpoints,
            cancellation,
            listener_tasks,
            background_tasks,
        })
    }

    #[must_use]
    pub fn invitation(&self) -> LanJoinCode {
        let identity = self
            .shared
            .membership
            .lock()
            .expect("LAN membership lock poisoned")
            .local()
            .identity;
        LanJoinCode {
            introducer: identity,
            control_endpoints: self.control_endpoints.clone(),
            session_credential: self.session_credential,
        }
    }

    #[must_use]
    pub fn descriptor(&self) -> PeerDescriptor {
        self.shared
            .membership
            .lock()
            .expect("LAN membership lock poisoned")
            .local()
            .clone()
    }

    #[must_use]
    pub fn control_endpoints(&self) -> &[SocketAddr] {
        &self.control_endpoints
    }

    pub async fn join(&self, invitation: &LanJoinCode, endpoint: SocketAddr) -> io::Result<()> {
        if !invitation.control_endpoints.contains(&endpoint) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "selected Introducer endpoint is not in the LAN invitation",
            ));
        }
        if session_proof(invitation.session_credential) != self.shared.session_proof {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "LAN invitation credential does not match the local room",
            ));
        }
        establish_outbound(self.shared.clone(), invitation.introducer, vec![endpoint]).await
    }

    #[must_use]
    pub fn peer_states(&self) -> Vec<LanPeerState> {
        let membership = self
            .shared
            .membership
            .lock()
            .expect("LAN membership lock poisoned");
        let mut states = membership
            .connected_descriptors()
            .into_iter()
            .map(|peer| LanPeerState {
                peer,
                connection: LanPeerConnectionState::Connected,
            })
            .collect::<Vec<_>>();
        states.extend(
            membership
                .hinted_descriptors()
                .into_iter()
                .map(|peer| LanPeerState {
                    connection: if membership.is_recovering(peer.identity) {
                        LanPeerConnectionState::Reconnecting
                    } else {
                        LanPeerConnectionState::Discovered
                    },
                    peer,
                }),
        );
        states.sort_by_key(|state| state.peer.identity);
        states
    }

    #[must_use]
    pub fn path_states(&self) -> Vec<LanPeerPathState> {
        self.shared.paths.states()
    }

    #[cfg(test)]
    fn test_link_id(&self, identity: PeerIdentity) -> Option<LinkId> {
        self.shared
            .membership
            .lock()
            .expect("LAN membership lock poisoned")
            .links()
            .into_iter()
            .find(|link| link.descriptor.identity == identity)
            .map(|link| link.key.link_id)
    }

    #[cfg(test)]
    fn test_interrupt_peer(&self, identity: PeerIdentity) {
        if let Some(link) = self
            .shared
            .membership
            .lock()
            .expect("LAN membership lock poisoned")
            .links()
            .into_iter()
            .find(|link| link.descriptor.identity == identity)
        {
            link.cancellation.cancel();
        }
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
        for link in membership_links(&self.shared) {
            let _ = link.sender.try_send(ControlMessage::Leave);
        }
        tokio::task::yield_now().await;
        self.cancellation.cancel();
        for task in self.listener_tasks.drain(..) {
            let _ = task.await;
        }
        for task in self.background_tasks.drain(..) {
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
        for task in &self.background_tasks {
            task.abort();
        }
    }
}

async fn run_listener(listener: TcpListener, shared: Arc<ControlShared>) {
    let permits = Arc::new(Semaphore::new(MAX_PENDING_CONTROL_CONNECTIONS));
    loop {
        tokio::select! {
            () = shared.cancellation.cancelled() => return,
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
                    let _ = handle_initial_control(stream, shared).await;
                });
            }
        }
    }
}

async fn handle_initial_control(stream: TcpStream, shared: Arc<ControlShared>) -> io::Result<()> {
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
    match message {
        ControlMessage::ProbeRequest {
            transaction_id,
            introducer,
            supported_protocol_ranges,
            required_capabilities,
            optional_capabilities,
            session_proof,
        } if introducer == local_descriptor(&shared).identity
            && session_proof == shared.session_proof =>
        {
            let selected_protocol =
                select_protocol(&SUPPORTED_PROTOCOLS, &supported_protocol_ranges)
                    .map_err(io::Error::other)?;
            let enabled_capabilities = select_capabilities(
                required_capabilities,
                optional_capabilities,
                KNOWN_CAPABILITIES,
            )
            .map_err(io::Error::other)?;
            send_control(
                &mut framed,
                &ControlMessage::ProbeResponse {
                    transaction_id,
                    introducer: local_descriptor(&shared).identity,
                    selected_protocol,
                    enabled_capabilities,
                },
            )
            .await
        }
        ControlMessage::ProbeRequest { .. } => {
            send_control(
                &mut framed,
                &ControlMessage::Error {
                    code: ControlErrorCode::InvalidCredential,
                    message: "LAN invitation is not valid for this peer".to_owned(),
                    retryable: false,
                },
            )
            .await
        }
        ControlMessage::JoinRequest {
            link_id,
            peer,
            supported_protocol_ranges,
            required_capabilities,
            optional_capabilities,
            session_proof,
        } => {
            accept_inbound_join(
                framed,
                shared,
                link_id,
                peer,
                supported_protocol_ranges,
                required_capabilities,
                optional_capabilities,
                session_proof,
            )
            .await
        }
        _ => {
            send_control(
                &mut framed,
                &ControlMessage::JoinRejected {
                    code: ControlErrorCode::InvalidState,
                    message: "expected LAN probe or join request".to_owned(),
                    retryable: false,
                },
            )
            .await
        }
    }
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

pub(super) fn framed(stream: TcpStream) -> ControlStream {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_CONTROL_PAYLOAD)
        .new_codec();
    tokio_util::codec::Framed::new(stream, codec)
}

pub(super) async fn send_control(
    stream: &mut ControlStream,
    message: &ControlMessage,
) -> io::Result<()> {
    let payload = encode_control(message).map_err(io::Error::other)?;
    stream.send(payload).await.map_err(io::Error::other)
}

fn session_proof(credential: SessionCredential) -> SessionProof {
    let mut hash = Sha256::new();
    hash.update(b"tractor-beam-direct-session-proof-v1");
    hash.update(credential.as_bytes());
    SessionProof::from_bytes(hash.finalize().into())
}

pub(super) fn nonzero_random<const N: usize>() -> [u8; N] {
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
#[path = "control_tests.rs"]
mod tests;
