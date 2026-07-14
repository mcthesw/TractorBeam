use std::{
    io,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt as _;
use tokio::{net::TcpStream, sync::mpsc, time};
use tokio_util::sync::CancellationToken;
use tractor_beam_direct_protocol::{
    CAP_HOST_CANDIDATES, ControlErrorCode, ControlMessage, KNOWN_CAPABILITIES, LinkId,
    PeerDescriptor, PeerIdentity, ProtocolRange, SessionProof, decode_control, select_capabilities,
    select_protocol,
};

use super::{
    control::{
        CONTROL_TOTAL_TIMEOUT, ControlShared, ControlStream, SUPPORTED_PROTOCOLS, framed,
        nonzero_random, send_control,
    },
    membership::{ActiveLink, LinkKey, RegisterResult},
};

const ANTI_ENTROPY_INTERVAL: Duration = Duration::from_secs(5);
const MEMBERSHIP_MAINTENANCE_INTERVAL: Duration = Duration::from_millis(500);
const LINK_MESSAGE_CAPACITY: usize = 32;

#[allow(clippy::too_many_arguments)]
pub(super) async fn accept_inbound_join(
    mut framed: ControlStream,
    shared: Arc<ControlShared>,
    link_id: LinkId,
    peer: PeerDescriptor,
    supported_protocol_ranges: Vec<ProtocolRange>,
    required_capabilities: u64,
    optional_capabilities: u64,
    session_proof: SessionProof,
) -> io::Result<()> {
    if session_proof != shared.session_proof {
        return reject_join(
            &mut framed,
            ControlErrorCode::InvalidCredential,
            "LAN session credential is invalid",
        )
        .await;
    }
    let selected_protocol = match select_protocol(&SUPPORTED_PROTOCOLS, &supported_protocol_ranges)
    {
        Ok(selected) => selected,
        Err(_) => {
            return reject_join(
                &mut framed,
                ControlErrorCode::UnsupportedProtocol,
                "no compatible direct protocol",
            )
            .await;
        }
    };
    let enabled_capabilities = match select_capabilities(
        required_capabilities,
        optional_capabilities,
        KNOWN_CAPABILITIES,
    ) {
        Ok(enabled) => enabled,
        Err(_) => {
            return reject_join(
                &mut framed,
                ControlErrorCode::UnsupportedProtocol,
                "required direct capabilities are unavailable",
            )
            .await;
        }
    };
    let key = LinkKey {
        link_id,
        initiator: peer.identity,
    };
    let (sender, receiver) = mpsc::channel(LINK_MESSAGE_CAPACITY);
    let link_cancellation = CancellationToken::new();
    match register_link(
        &shared,
        ActiveLink {
            key,
            descriptor: peer.clone(),
            sender,
            cancellation: link_cancellation.clone(),
        },
    ) {
        RegisterResult::Accepted { replaced } => {
            if let Some(replaced) = replaced {
                replaced.cancellation.cancel();
            }
        }
        RegisterResult::DuplicateLost => {
            return reject_join(
                &mut framed,
                ControlErrorCode::InvalidState,
                "a preferred control link is already active",
            )
            .await;
        }
        RegisterResult::DuplicateSteamIdentity => {
            return reject_join(
                &mut framed,
                ControlErrorCode::DuplicateIdentity,
                "this Steam identity is already connected",
            )
            .await;
        }
        RegisterResult::SelfConnection => {
            return reject_join(
                &mut framed,
                ControlErrorCode::IdentityMismatch,
                "cannot connect a LAN peer to itself",
            )
            .await;
        }
    }

    send_control(
        &mut framed,
        &ControlMessage::JoinAccepted {
            selected_protocol,
            enabled_capabilities,
            peers: membership_snapshot(&shared),
        },
    )
    .await?;
    broadcast_snapshot(&shared);
    run_link(framed, shared, peer, key, receiver, link_cancellation).await;
    Ok(())
}

async fn reject_join(
    framed: &mut ControlStream,
    code: ControlErrorCode,
    message: &str,
) -> io::Result<()> {
    send_control(
        framed,
        &ControlMessage::JoinRejected {
            code,
            message: message.to_owned(),
            retryable: false,
        },
    )
    .await
}

pub(super) async fn establish_outbound(
    shared: Arc<ControlShared>,
    expected: PeerIdentity,
    endpoints: Vec<SocketAddr>,
) -> io::Result<()> {
    if !begin_dial(&shared, expected) {
        return Ok(());
    }
    let result = establish_outbound_inner(shared.clone(), expected, endpoints).await;
    end_dial(&shared, expected, result.is_err());
    result
}

async fn establish_outbound_inner(
    shared: Arc<ControlShared>,
    expected: PeerIdentity,
    endpoints: Vec<SocketAddr>,
) -> io::Result<()> {
    let mut last_error = None;
    for endpoint in endpoints {
        match time::timeout(
            CONTROL_TOTAL_TIMEOUT,
            establish_outbound_endpoint(shared.clone(), expected, endpoint),
        )
        .await
        {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => last_error = Some(error),
            Err(_) => {
                last_error = Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "LAN admission timed out",
                ));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "LAN peer has no control candidates",
        )
    }))
}

async fn establish_outbound_endpoint(
    shared: Arc<ControlShared>,
    expected: PeerIdentity,
    endpoint: SocketAddr,
) -> io::Result<()> {
    let stream = TcpStream::connect(endpoint).await?;
    let mut framed = framed(stream);
    let local = local_descriptor(&shared);
    let link_id = LinkId::from_bytes(nonzero_random());
    send_control(
        &mut framed,
        &ControlMessage::JoinRequest {
            link_id,
            peer: local.clone(),
            supported_protocol_ranges: SUPPORTED_PROTOCOLS.to_vec(),
            required_capabilities: CAP_HOST_CANDIDATES,
            optional_capabilities: KNOWN_CAPABILITIES,
            session_proof: shared.session_proof,
        },
    )
    .await?;
    let Some(payload) = framed.next().await else {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "LAN admission connection closed",
        ));
    };
    let peers = match decode_control(&payload.map_err(io::Error::other)?)
        .map_err(io::Error::other)?
    {
        ControlMessage::JoinAccepted { peers, .. } => peers,
        ControlMessage::JoinRejected { message, .. } | ControlMessage::Error { message, .. } => {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, message));
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected LAN admission response",
            ));
        }
    };
    let remote = peers
        .iter()
        .find(|peer| peer.identity == expected)
        .cloned()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "LAN admission response omitted the expected peer",
            )
        })?;
    let discovered = merge_hints(&shared, peers);
    schedule_discovered(&shared, discovered);

    let key = LinkKey {
        link_id,
        initiator: local.identity,
    };
    let (sender, receiver) = mpsc::channel(LINK_MESSAGE_CAPACITY);
    let link_cancellation = CancellationToken::new();
    match register_link(
        &shared,
        ActiveLink {
            key,
            descriptor: remote.clone(),
            sender,
            cancellation: link_cancellation.clone(),
        },
    ) {
        RegisterResult::Accepted { replaced } => {
            if let Some(replaced) = replaced {
                replaced.cancellation.cancel();
            }
        }
        RegisterResult::DuplicateLost => return Ok(()),
        RegisterResult::DuplicateSteamIdentity => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "this Steam identity is already connected",
            ));
        }
        RegisterResult::SelfConnection => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot connect a LAN peer to itself",
            ));
        }
    }
    broadcast_snapshot(&shared);
    tokio::spawn(run_link(
        framed,
        shared,
        remote,
        key,
        receiver,
        link_cancellation,
    ));
    Ok(())
}

async fn run_link(
    mut framed: ControlStream,
    shared: Arc<ControlShared>,
    remote: PeerDescriptor,
    key: LinkKey,
    mut outbound: mpsc::Receiver<ControlMessage>,
    link_cancellation: CancellationToken,
) {
    let mut anti_entropy = time::interval(ANTI_ENTROPY_INTERVAL);
    anti_entropy.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    anti_entropy.tick().await;
    let mut graceful = false;
    loop {
        tokio::select! {
            () = shared.cancellation.cancelled() => {
                let _ = send_control(&mut framed, &ControlMessage::Leave).await;
                graceful = true;
                break;
            }
            () = link_cancellation.cancelled() => break,
            message = outbound.recv() => {
                let Some(message) = message else { break; };
                if send_control(&mut framed, &message).await.is_err() {
                    break;
                }
            }
            incoming = framed.next() => {
                let Some(Ok(payload)) = incoming else { break; };
                let Ok(message) = decode_control(&payload) else { break; };
                match message {
                    ControlMessage::PeerSnapshot { peers } => {
                        let discovered = merge_hints(&shared, peers);
                        schedule_discovered(&shared, discovered);
                    }
                    ControlMessage::Leave => {
                        graceful = true;
                        break;
                    }
                    ControlMessage::ControlPing { id } => {
                        if send_control(&mut framed, &ControlMessage::ControlPong { id }).await.is_err() {
                            break;
                        }
                    }
                    ControlMessage::ControlPong { .. } => {}
                    _ => {}
                }
            }
            _ = anti_entropy.tick() => {
                if send_control(
                    &mut framed,
                    &ControlMessage::PeerSnapshot { peers: membership_snapshot(&shared) },
                ).await.is_err() {
                    break;
                }
            }
        }
    }
    if remove_link(&shared, remote.identity, key, graceful) {
        broadcast_snapshot(&shared);
        if !graceful {
            schedule_discovered(&shared, vec![remote]);
        }
    }
}

pub(super) async fn run_membership_dialer(
    shared: Arc<ControlShared>,
    mut dial_rx: mpsc::UnboundedReceiver<PeerDescriptor>,
) {
    let mut maintenance = time::interval(MEMBERSHIP_MAINTENANCE_INTERVAL);
    maintenance.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = shared.cancellation.cancelled() => return,
            Some(peer) = dial_rx.recv() => spawn_peer_dial(shared.clone(), peer),
            _ = maintenance.tick() => {
                let peers = {
                    let mut membership = shared.membership.lock().expect("LAN membership lock poisoned");
                    membership.expire(Instant::now());
                    membership.retry_candidates()
                };
                for peer in peers {
                    spawn_peer_dial(shared.clone(), peer);
                }
            }
        }
    }
}

fn spawn_peer_dial(shared: Arc<ControlShared>, peer: PeerDescriptor) {
    tokio::spawn(async move {
        let endpoints = peer
            .control_candidates
            .iter()
            .map(|candidate| candidate.endpoint)
            .collect();
        let _ = establish_outbound(shared, peer.identity, endpoints).await;
    });
}

pub(super) fn local_descriptor(shared: &ControlShared) -> PeerDescriptor {
    shared
        .membership
        .lock()
        .expect("LAN membership lock poisoned")
        .local()
        .clone()
}

fn membership_snapshot(shared: &ControlShared) -> Vec<PeerDescriptor> {
    shared
        .membership
        .lock()
        .expect("LAN membership lock poisoned")
        .snapshot()
}

pub(super) fn membership_links(shared: &ControlShared) -> Vec<ActiveLink> {
    shared
        .membership
        .lock()
        .expect("LAN membership lock poisoned")
        .links()
}

fn merge_hints(shared: &ControlShared, peers: Vec<PeerDescriptor>) -> Vec<PeerDescriptor> {
    shared
        .membership
        .lock()
        .expect("LAN membership lock poisoned")
        .merge_hints(peers)
}

fn schedule_discovered(shared: &ControlShared, peers: Vec<PeerDescriptor>) {
    for peer in peers {
        let _ = shared.dial_tx.send(peer);
    }
}

fn begin_dial(shared: &ControlShared, identity: PeerIdentity) -> bool {
    shared
        .membership
        .lock()
        .expect("LAN membership lock poisoned")
        .begin_dial(identity)
}

fn end_dial(shared: &ControlShared, identity: PeerIdentity, failed: bool) {
    shared
        .membership
        .lock()
        .expect("LAN membership lock poisoned")
        .end_dial(identity, failed);
}

fn register_link(shared: &ControlShared, link: ActiveLink) -> RegisterResult {
    shared
        .membership
        .lock()
        .expect("LAN membership lock poisoned")
        .register(link)
}

fn remove_link(
    shared: &ControlShared,
    identity: PeerIdentity,
    key: LinkKey,
    graceful: bool,
) -> bool {
    shared
        .membership
        .lock()
        .expect("LAN membership lock poisoned")
        .remove_link(identity, key, graceful)
}

fn broadcast_snapshot(shared: &ControlShared) {
    let message = ControlMessage::PeerSnapshot {
        peers: membership_snapshot(shared),
    };
    for link in membership_links(shared) {
        let _ = link.sender.try_send(message.clone());
    }
}
