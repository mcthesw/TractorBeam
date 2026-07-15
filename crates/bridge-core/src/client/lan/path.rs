use std::{
    collections::{BTreeMap, HashMap},
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use rand::RngExt as _;
use tokio::{
    net::UdpSocket,
    sync::mpsc,
    task::JoinHandle,
    time::{self, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tractor_beam_direct_protocol::{
    CheckFrame, CheckPhase, ControlMessage, DirectFrame, HeartbeatFrame, HeartbeatPhase,
    HostCandidate, PathContext, PathId, PathToken, PeerDescriptor, PeerIdentity, TransactionId,
    decode_frame,
};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const PATH_LIVENESS_TIMEOUT: Duration = Duration::from_secs(3);
const PATH_UNAVAILABLE_AFTER: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LanPeerPathStatus {
    Checking,
    Usable,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanPeerPathState {
    pub peer: PeerIdentity,
    pub status: LanPeerPathStatus,
    pub local_endpoint: Option<SocketAddr>,
    pub remote_endpoint: Option<SocketAddr>,
}

pub(super) struct PathManager {
    local: PeerIdentity,
    candidates: Vec<LocalCandidate>,
    cancellation: CancellationToken,
    inner: Mutex<PathState>,
}

#[derive(Clone)]
struct LocalCandidate {
    wire: HostCandidate,
    socket: Arc<UdpSocket>,
}

#[derive(Default)]
struct PathState {
    peers: HashMap<PeerIdentity, PeerPath>,
    transactions: HashMap<TransactionId, PendingCheck>,
}

struct PeerPath {
    control: mpsc::Sender<ControlMessage>,
    material: Option<PathMaterial>,
    remote_candidates: Vec<HostCandidate>,
    checks: BTreeMap<(SocketAddr, SocketAddr), CheckState>,
    nominated: Option<NominatedPath>,
    pending_nomination: Option<(SocketAddr, SocketAddr)>,
    next_heartbeat_id: u64,
    checking_since: Instant,
}

#[derive(Clone, Copy)]
struct PathMaterial {
    id: PathId,
    token: PathToken,
}

#[derive(Clone, Copy, Debug, Default)]
struct CheckState {
    request_seen: bool,
    response_seen: bool,
}

#[derive(Clone, Copy)]
struct PendingCheck {
    peer: PeerIdentity,
    local_endpoint: SocketAddr,
    remote_endpoint: SocketAddr,
}

#[derive(Clone, Copy)]
struct NominatedPath {
    local_endpoint: SocketAddr,
    remote_endpoint: SocketAddr,
    last_seen: Instant,
}

impl PathManager {
    pub async fn new(
        local: PeerIdentity,
        sockets: Vec<(Arc<UdpSocket>, u32)>,
        cancellation: CancellationToken,
    ) -> io::Result<Arc<Self>> {
        let mut candidates = Vec::with_capacity(sockets.len());
        for (socket, priority) in sockets {
            candidates.push(LocalCandidate {
                wire: HostCandidate::new(socket.local_addr()?, priority, 0)
                    .map_err(io::Error::other)?,
                socket,
            });
        }
        Ok(Arc::new(Self {
            local,
            candidates,
            cancellation,
            inner: Mutex::new(PathState::default()),
        }))
    }

    pub fn start(self: &Arc<Self>) -> Vec<JoinHandle<()>> {
        let mut tasks = self
            .candidates
            .iter()
            .map(|candidate| {
                tokio::spawn(run_udp_receiver(
                    self.clone(),
                    candidate.wire.endpoint,
                    candidate.socket.clone(),
                ))
            })
            .collect::<Vec<_>>();
        tasks.push(tokio::spawn(run_path_maintenance(self.clone())));
        tasks
    }

    pub fn peer_connected(
        self: &Arc<Self>,
        descriptor: PeerDescriptor,
        control: mpsc::Sender<ControlMessage>,
    ) {
        let remote = descriptor.identity;
        let mut offer = None;
        {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            state.transactions.retain(|_, check| check.peer != remote);
            let path = PeerPath {
                control: control.clone(),
                material: None,
                remote_candidates: Vec::new(),
                checks: BTreeMap::new(),
                nominated: None,
                pending_nomination: None,
                next_heartbeat_id: 1,
                checking_since: Instant::now(),
            };
            state.peers.insert(remote, path);
            if self.local < remote {
                let material = PathMaterial {
                    id: PathId::from_bytes(nonzero_random()),
                    token: PathToken::from_bytes(nonzero_random()),
                };
                if let Some(path) = state.peers.get_mut(&remote) {
                    path.material = Some(material);
                }
                offer = Some(path_offer(self.local, material, &self.candidates));
            }
        }
        if let Some(offer) = offer {
            let _ = control.try_send(offer);
        }
    }

    pub fn peer_disconnected(&self, peer: PeerIdentity) {
        let mut state = self.inner.lock().expect("LAN path lock poisoned");
        state.peers.remove(&peer);
        state.transactions.retain(|_, check| check.peer != peer);
    }

    pub fn handle_control(self: &Arc<Self>, peer: PeerIdentity, message: ControlMessage) -> bool {
        match message {
            ControlMessage::PathOffer {
                peer: sender,
                path_id,
                path_token,
                data_candidates,
            } if sender == peer => {
                self.handle_offer(
                    peer,
                    PathMaterial {
                        id: path_id,
                        token: path_token,
                    },
                    data_candidates,
                );
                true
            }
            ControlMessage::Nominate {
                path_id,
                local_endpoint,
                remote_endpoint,
            } => {
                self.handle_nominate(peer, path_id, local_endpoint, remote_endpoint);
                true
            }
            ControlMessage::NominateAck { path_id } => {
                self.handle_nominate_ack(peer, path_id);
                true
            }
            _ => false,
        }
    }

    pub fn states(&self) -> Vec<LanPeerPathState> {
        let state = self.inner.lock().expect("LAN path lock poisoned");
        let mut paths = state
            .peers
            .iter()
            .map(|(peer, path)| {
                let (status, local_endpoint, remote_endpoint) = path.nominated.map_or_else(
                    || {
                        let status = if path.checking_since.elapsed() >= PATH_UNAVAILABLE_AFTER {
                            LanPeerPathStatus::Unavailable
                        } else {
                            LanPeerPathStatus::Checking
                        };
                        (status, None, None)
                    },
                    |nominated| {
                        (
                            LanPeerPathStatus::Usable,
                            Some(nominated.local_endpoint),
                            Some(nominated.remote_endpoint),
                        )
                    },
                );
                LanPeerPathState {
                    peer: *peer,
                    status,
                    local_endpoint,
                    remote_endpoint,
                }
            })
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| path.peer);
        paths
    }

    fn handle_offer(
        self: &Arc<Self>,
        peer: PeerIdentity,
        material: PathMaterial,
        candidates: Vec<HostCandidate>,
    ) {
        let mut reply = None;
        let should_check = {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            let Some(path) = state.peers.get_mut(&peer) else {
                return;
            };
            match path.material {
                Some(existing)
                    if existing.id != material.id || existing.token != material.token =>
                {
                    return;
                }
                None if self.local < peer => return,
                None => {
                    path.material = Some(material);
                    reply = Some((
                        path.control.clone(),
                        path_offer(self.local, material, &self.candidates),
                    ));
                }
                Some(_) => {}
            }
            path.remote_candidates = candidates;
            path.checks.clear();
            path.nominated = None;
            path.pending_nomination = None;
            path.checking_since = Instant::now();
            true
        };
        if let Some((control, message)) = reply {
            let _ = control.try_send(message);
        }
        if should_check {
            self.send_checks(peer);
        }
    }

    fn send_checks(self: &Arc<Self>, peer: PeerIdentity) {
        let sends = {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            state.transactions.retain(|_, check| check.peer != peer);
            let Some(path) = state.peers.get(&peer) else {
                return;
            };
            let Some(material) = path.material else {
                return;
            };
            let remote_candidates = path.remote_candidates.clone();
            let mut sends = Vec::new();
            for local in &self.candidates {
                for remote in &remote_candidates {
                    let transaction = TransactionId::from_bytes(nonzero_random());
                    state.transactions.insert(
                        transaction,
                        PendingCheck {
                            peer,
                            local_endpoint: local.wire.endpoint,
                            remote_endpoint: remote.endpoint,
                        },
                    );
                    let frame = CheckFrame {
                        path: PathContext {
                            path_id: material.id,
                            path_token: material.token,
                            from: self.local,
                            to_steam_id64: peer.steam_id64,
                        },
                        transaction_id: transaction,
                        phase: CheckPhase::Request,
                    };
                    if let Ok(payload) = frame.encode() {
                        sends.push((local.socket.clone(), remote.endpoint, payload));
                    }
                }
            }
            sends
        };
        for (socket, endpoint, payload) in sends {
            tokio::spawn(async move {
                let _ = socket.send_to(&payload, endpoint).await;
            });
        }
    }

    fn handle_nominate(
        &self,
        peer: PeerIdentity,
        path_id: PathId,
        controller_local: SocketAddr,
        controller_remote: SocketAddr,
    ) {
        let response = {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            let Some(path) = state.peers.get_mut(&peer) else {
                return;
            };
            if self.local < peer || path.material.is_none_or(|material| material.id != path_id) {
                return;
            }
            let local_pair = (controller_remote, controller_local);
            if path
                .checks
                .get(&local_pair)
                .is_none_or(|check| !check.request_seen || !check.response_seen)
            {
                return;
            }
            path.nominated = Some(NominatedPath {
                local_endpoint: controller_remote,
                remote_endpoint: controller_local,
                last_seen: Instant::now(),
            });
            Some((
                path.control.clone(),
                ControlMessage::NominateAck { path_id },
            ))
        };
        if let Some((control, message)) = response {
            let _ = control.try_send(message);
        }
    }

    fn handle_nominate_ack(&self, peer: PeerIdentity, path_id: PathId) {
        let mut state = self.inner.lock().expect("LAN path lock poisoned");
        let Some(path) = state.peers.get_mut(&peer) else {
            return;
        };
        if self.local >= peer || path.material.is_none_or(|material| material.id != path_id) {
            return;
        }
        let Some((local_endpoint, remote_endpoint)) = path.pending_nomination.take() else {
            return;
        };
        path.nominated = Some(NominatedPath {
            local_endpoint,
            remote_endpoint,
            last_seen: Instant::now(),
        });
    }

    fn maybe_nominate(&self, peer: PeerIdentity) {
        let nomination = {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            let Some(path) = state.peers.get_mut(&peer) else {
                return;
            };
            if self.local >= peer || path.nominated.is_some() || path.pending_nomination.is_some() {
                return;
            }
            let Some(material) = path.material else {
                return;
            };
            let priorities = path
                .remote_candidates
                .iter()
                .map(|candidate| (candidate.endpoint, candidate.priority))
                .collect::<HashMap<_, _>>();
            let local_priorities = self
                .candidates
                .iter()
                .map(|candidate| (candidate.wire.endpoint, candidate.wire.priority))
                .collect::<HashMap<_, _>>();
            let selected = select_candidate_pair(&path.checks, &local_priorities, &priorities);
            let Some((local_endpoint, remote_endpoint)) = selected else {
                return;
            };
            path.pending_nomination = Some((local_endpoint, remote_endpoint));
            Some((
                path.control.clone(),
                ControlMessage::Nominate {
                    path_id: material.id,
                    local_endpoint,
                    remote_endpoint,
                },
            ))
        };
        if let Some((control, message)) = nomination {
            let _ = control.try_send(message);
        }
    }
}

async fn run_udp_receiver(
    manager: Arc<PathManager>,
    local_endpoint: SocketAddr,
    socket: Arc<UdpSocket>,
) {
    let mut buffer = vec![0_u8; tractor_beam_direct_protocol::MAX_FRAME_LEN];
    loop {
        tokio::select! {
            () = manager.cancellation.cancelled() => return,
            received = socket.recv_from(&mut buffer) => {
                let Ok((size, source)) = received else { return; };
                manager.handle_datagram(
                    local_endpoint,
                    source,
                    Bytes::copy_from_slice(&buffer[..size]),
                );
            }
        }
    }
}

impl PathManager {
    fn handle_datagram(self: &Arc<Self>, local: SocketAddr, source: SocketAddr, bytes: Bytes) {
        let Ok(frame) = decode_frame(bytes) else {
            return;
        };
        match frame {
            DirectFrame::Check(check) => self.handle_check(local, source, check),
            DirectFrame::Heartbeat(heartbeat) => self.handle_heartbeat(local, source, heartbeat),
            DirectFrame::Data(_) => {}
        }
    }

    fn handle_check(self: &Arc<Self>, local: SocketAddr, source: SocketAddr, frame: CheckFrame) {
        let mut response = None;
        let peer = frame.path.from;
        {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            let Some(path) = state.peers.get(&peer) else {
                return;
            };
            if frame.path.to_steam_id64 != self.local.steam_id64
                || path.material.is_none_or(|material| {
                    material.id != frame.path.path_id || material.token != frame.path.path_token
                })
                || !self
                    .candidates
                    .iter()
                    .any(|candidate| candidate.wire.endpoint == local)
                || !path
                    .remote_candidates
                    .iter()
                    .any(|candidate| candidate.endpoint == source)
            {
                return;
            }
            match frame.phase {
                CheckPhase::Request => {
                    let path = state
                        .peers
                        .get_mut(&peer)
                        .expect("LAN path presence was validated");
                    path.checks.entry((local, source)).or_default().request_seen = true;
                    response = Some(CheckFrame {
                        path: PathContext {
                            path_id: frame.path.path_id,
                            path_token: frame.path.path_token,
                            from: self.local,
                            to_steam_id64: peer.steam_id64,
                        },
                        transaction_id: frame.transaction_id,
                        phase: CheckPhase::Response,
                    });
                }
                CheckPhase::Response => {
                    let Some(pending) = state.transactions.remove(&frame.transaction_id) else {
                        return;
                    };
                    if pending.peer != peer
                        || pending.local_endpoint != local
                        || pending.remote_endpoint != source
                    {
                        return;
                    }
                    let path = state
                        .peers
                        .get_mut(&peer)
                        .expect("LAN path presence was validated");
                    path.checks
                        .entry((local, source))
                        .or_default()
                        .response_seen = true;
                }
            }
        }
        if let Some(response) = response
            && let Some(socket) = self.socket_for(local)
            && let Ok(payload) = response.encode()
        {
            tokio::spawn(async move {
                let _ = socket.send_to(&payload, source).await;
            });
        }
        self.maybe_nominate(peer);
    }

    fn handle_heartbeat(&self, local: SocketAddr, source: SocketAddr, frame: HeartbeatFrame) {
        let mut response = None;
        {
            let mut state = self.inner.lock().expect("LAN path lock poisoned");
            let Some(path) = state.peers.get_mut(&frame.path.from) else {
                return;
            };
            let Some(nominated) = path.nominated.as_mut() else {
                return;
            };
            if frame.path.to_steam_id64 != self.local.steam_id64
                || nominated.local_endpoint != local
                || nominated.remote_endpoint != source
                || path.material.is_none_or(|material| {
                    material.id != frame.path.path_id || material.token != frame.path.path_token
                })
            {
                return;
            }
            nominated.last_seen = Instant::now();
            if frame.phase == HeartbeatPhase::Request {
                response = Some(HeartbeatFrame {
                    path: PathContext {
                        path_id: frame.path.path_id,
                        path_token: frame.path.path_token,
                        from: self.local,
                        to_steam_id64: frame.path.from.steam_id64,
                    },
                    heartbeat_id: frame.heartbeat_id,
                    phase: HeartbeatPhase::Response,
                });
            }
        }
        if let Some(response) = response
            && let Some(socket) = self.socket_for(local)
            && let Ok(payload) = response.encode()
        {
            tokio::spawn(async move {
                let _ = socket.send_to(&payload, source).await;
            });
        }
    }

    fn socket_for(&self, endpoint: SocketAddr) -> Option<Arc<UdpSocket>> {
        self.candidates
            .iter()
            .find(|candidate| candidate.wire.endpoint == endpoint)
            .map(|candidate| candidate.socket.clone())
    }
}

async fn run_path_maintenance(manager: Arc<PathManager>) {
    let mut interval = time::interval(HEARTBEAT_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = manager.cancellation.cancelled() => return,
            _ = interval.tick() => maintain_paths(&manager),
        }
    }
}

fn maintain_paths(manager: &Arc<PathManager>) {
    let now = Instant::now();
    let mut heartbeats = Vec::new();
    let mut nominations = Vec::new();
    let mut recheck = Vec::new();
    {
        let mut state = manager.inner.lock().expect("LAN path lock poisoned");
        for (peer, path) in &mut state.peers {
            let Some(nominated) = path.nominated else {
                if let (Some(material), Some((local_endpoint, remote_endpoint))) =
                    (path.material, path.pending_nomination)
                {
                    nominations.push((
                        path.control.clone(),
                        ControlMessage::Nominate {
                            path_id: material.id,
                            local_endpoint,
                            remote_endpoint,
                        },
                    ));
                }
                if path.material.is_some() && !path.remote_candidates.is_empty() {
                    recheck.push(*peer);
                }
                continue;
            };
            if now.duration_since(nominated.last_seen) >= PATH_LIVENESS_TIMEOUT {
                path.nominated = None;
                path.pending_nomination = None;
                path.checks.clear();
                path.checking_since = now;
                recheck.push(*peer);
                continue;
            }
            let Some(material) = path.material else {
                continue;
            };
            let heartbeat_id = path.next_heartbeat_id;
            path.next_heartbeat_id = path.next_heartbeat_id.checked_add(1).unwrap_or(1);
            heartbeats.push((
                *peer,
                nominated,
                HeartbeatFrame {
                    path: PathContext {
                        path_id: material.id,
                        path_token: material.token,
                        from: manager.local,
                        to_steam_id64: peer.steam_id64,
                    },
                    heartbeat_id,
                    phase: HeartbeatPhase::Request,
                },
            ));
        }
    }
    for (control, nomination) in nominations {
        let _ = control.try_send(nomination);
    }
    for (_, nominated, heartbeat) in heartbeats {
        if let Some(socket) = manager.socket_for(nominated.local_endpoint)
            && let Ok(payload) = heartbeat.encode()
        {
            tokio::spawn(async move {
                let _ = socket.send_to(&payload, nominated.remote_endpoint).await;
            });
        }
    }
    for peer in recheck {
        manager.send_checks(peer);
    }
}

fn path_offer(
    local: PeerIdentity,
    material: PathMaterial,
    candidates: &[LocalCandidate],
) -> ControlMessage {
    ControlMessage::PathOffer {
        peer: local,
        path_id: material.id,
        path_token: material.token,
        data_candidates: candidates.iter().map(|candidate| candidate.wire).collect(),
    }
}

fn select_candidate_pair(
    checks: &BTreeMap<(SocketAddr, SocketAddr), CheckState>,
    local_priorities: &HashMap<SocketAddr, u32>,
    remote_priorities: &HashMap<SocketAddr, u32>,
) -> Option<(SocketAddr, SocketAddr)> {
    checks
        .iter()
        .filter(|(_, check)| check.request_seen && check.response_seen)
        .max_by_key(|((local, remote), _)| {
            (
                local_priorities.get(local).copied().unwrap_or_default()
                    + remote_priorities.get(remote).copied().unwrap_or_default(),
                *local,
                *remote,
            )
        })
        .map(|(pair, _)| *pair)
}

fn nonzero_random<const N: usize>() -> [u8; N] {
    loop {
        let value = rand::rng().random::<[u8; N]>();
        if value.iter().any(|byte| *byte != 0) {
            return value;
        }
    }
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
