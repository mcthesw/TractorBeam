use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Mutex, mpsc},
    time,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, info, warn};
use tractor_beam_relay_protocol::v2::{
    BOOTSTRAP_SCHEMA, BootstrapMessage, BuildMetadata, CAP_RESUME, CAP_TCP_DATA, CAP_UDP_DATA,
    ClientControl, CompatibilityReject, DataFrame, Frame, ProtocolRange, RejectCode, ServerControl,
    decode_bootstrap, decode_client_control, decode_frame, encode_bootstrap, encode_server_control,
    select_capabilities, select_protocol,
};

use crate::{
    config::RelayConfig,
    domain::PeerId,
    domain_v2::{DataDestination, DataSource, JoinBegin, PresenceBroadcast, RouteData},
    metrics_v2::{METRICS, RelayMetricsV2},
    peer_registry::PeerRegistry,
    state_v2::RelayStateV2,
    v2,
};

type SharedState = Arc<Mutex<RelayStateV2>>;
type SharedTcpEgress = Arc<Mutex<HashMap<PeerId, mpsc::Sender<Bytes>>>>;

struct TcpTaskContext {
    state: SharedState,
    egress: SharedTcpEgress,
    udp: Option<Arc<UdpSocket>>,
    max_frame_size: usize,
    queue_capacity: usize,
}

pub(crate) async fn run(config: RelayConfig) -> io::Result<()> {
    let tcp_bind = config.tcp_bind.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "TCP control listener is required",
        )
    })?;
    let listener = TcpListener::bind(tcp_bind).await?;
    let udp = match config.udp_bind.as_deref() {
        Some(bind) => Some(Arc::new(UdpSocket::bind(bind).await?)),
        None => None,
    };
    info!(
        tcp_bind,
        udp_bind = config.udp_bind.as_deref().unwrap_or("disabled"),
        recovery_grace_seconds = 120,
        "Relay Protocol v2 listening"
    );
    run_with_listeners(listener, udp, config).await
}

async fn run_with_listeners(
    listener: TcpListener,
    udp: Option<Arc<UdpSocket>>,
    config: RelayConfig,
) -> io::Result<()> {
    let state = Arc::new(Mutex::new(RelayStateV2::new(config.clone())));
    let egress = Arc::new(Mutex::new(HashMap::new()));
    let registry = Arc::new(Mutex::new(PeerRegistry::default()));
    if let Some(socket) = udp.clone() {
        tokio::spawn(udp_task(socket, Arc::clone(&state), Arc::clone(&egress)));
    }
    tokio::spawn(cleanup_task(Arc::clone(&state), Arc::clone(&egress)));
    tokio::spawn(metrics_task(Arc::clone(&state)));

    loop {
        let (stream, address) = listener.accept().await?;
        if config
            .blocked_cidrs
            .iter()
            .any(|network| network.contains(&address.ip()))
        {
            debug!(%address, "blocked TCP control connection");
            continue;
        }
        stream.set_nodelay(true)?;
        let peer_id = registry.lock().await.allocate();
        tokio::spawn(tcp_task(
            stream,
            address,
            peer_id,
            TcpTaskContext {
                state: Arc::clone(&state),
                egress: Arc::clone(&egress),
                udp: udp.clone(),
                max_frame_size: config.max_packet_size,
                queue_capacity: config.tcp_egress_queue_capacity,
            },
        ));
    }
}

async fn tcp_task(
    mut stream: TcpStream,
    address: SocketAddr,
    peer_id: PeerId,
    context: TcpTaskContext,
) {
    let TcpTaskContext {
        state,
        egress,
        udp,
        max_frame_size,
        queue_capacity,
    } = context;
    let negotiation = time::timeout(
        Duration::from_secs(5),
        negotiate(&mut stream, address, udp.is_some()),
    )
    .await;
    if !matches!(negotiation, Ok(Ok(()))) {
        RelayMetricsV2::increment(&METRICS.bootstrap_rejected);
        if let Ok(Err(error)) = negotiation {
            warn!(%address, %error, "v2 bootstrap failed");
        }
        return;
    }
    RelayMetricsV2::increment(&METRICS.bootstrap_accepted);

    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(max_frame_size.max(16 * 1024 + 16))
        .new_codec();
    let framed = Framed::new(stream, codec);
    let (mut sink, mut inbound) = framed.split();
    let (outbound_tx, mut outbound_rx) = mpsc::channel(queue_capacity);
    egress.lock().await.insert(peer_id, outbound_tx);
    let mut explicit_stop = false;

    loop {
        tokio::select! {
            frame = inbound.next() => {
                let Some(frame) = frame else { break; };
                match frame {
                    Ok(bytes) => match handle_tcp_frame(peer_id, bytes.freeze(), &state, &egress, udp.as_ref()).await {
                        Ok(stop) => {
                            if stop { explicit_stop = true; break; }
                        }
                        Err(error) => {
                            warn!(%peer_id, %address, %error, "v2 TCP frame rejected");
                            break;
                        }
                    },
                    Err(error) => {
                        warn!(%peer_id, %address, %error, "v2 TCP framing failed");
                        break;
                    }
                }
            }
            Some(frame) = outbound_rx.recv() => {
                if let Err(error) = sink.send(frame).await {
                    debug!(%peer_id, %address, %error, "v2 TCP send ended");
                    break;
                }
            }
        }
    }

    egress.lock().await.remove(&peer_id);
    let broadcast = if explicit_stop {
        state.lock().await.stop(peer_id)
    } else {
        state.lock().await.detach(peer_id, Instant::now())
    };
    if let Some(broadcast) = broadcast {
        send_presence(&egress, broadcast).await;
    }
    if explicit_stop {
        info!(%peer_id, %address, "v2 session stopped");
    } else {
        RelayMetricsV2::increment(&METRICS.control_detached);
        info!(%peer_id, %address, grace_seconds = 120, "v2 control connection detached");
    }
}

async fn negotiate(
    stream: &mut TcpStream,
    address: SocketAddr,
    udp_enabled: bool,
) -> io::Result<()> {
    let frame = read_bootstrap(stream).await?;
    let hello = decode_bootstrap(&frame).map_err(invalid_data)?;
    let BootstrapMessage::ClientHello {
        bootstrap_schema,
        supported_protocol_ranges,
        required_capabilities,
        optional_capabilities,
        client,
    } = hello
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected ClientHello",
        ));
    };
    let relay_ranges = vec![ProtocolRange {
        major: 2,
        min_minor: 0,
        max_minor: 0,
    }];
    let available = CAP_TCP_DATA | CAP_RESUME | if udp_enabled { CAP_UDP_DATA } else { 0 };
    if bootstrap_schema != BOOTSTRAP_SCHEMA {
        send_reject(stream, RejectCode::UnsupportedBootstrapSchema, relay_ranges).await?;
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported bootstrap schema",
        ));
    }
    let selected = match select_protocol(&supported_protocol_ranges, &relay_ranges) {
        Ok(version) => version,
        Err(_) => {
            send_reject(stream, RejectCode::UnsupportedProtocol, relay_ranges).await?;
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "no compatible v2 protocol",
            ));
        }
    };
    let enabled = match select_capabilities(required_capabilities, optional_capabilities, available)
    {
        Ok(enabled) => enabled,
        Err(_) => {
            send_reject(
                stream,
                RejectCode::MissingRequiredCapabilities,
                relay_ranges,
            )
            .await?;
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "required capabilities unavailable",
            ));
        }
    };
    let response = BootstrapMessage::ServerHello {
        bootstrap_schema: BOOTSTRAP_SCHEMA,
        selected_protocol: selected,
        enabled_capabilities: enabled,
        relay: relay_build(),
    };
    stream
        .write_all(&encode_bootstrap(&response).map_err(invalid_data)?)
        .await?;
    info!(
        %address,
        client_version = %client.version,
        client_git_hash = client.git_hash.as_deref().unwrap_or(""),
        protocol_major = selected.major,
        protocol_minor = selected.minor,
        enabled_capabilities = enabled,
        "v2 bootstrap accepted"
    );
    Ok(())
}

async fn send_reject(
    stream: &mut TcpStream,
    code: RejectCode,
    ranges: Vec<ProtocolRange>,
) -> io::Result<()> {
    let reject = BootstrapMessage::CompatibilityReject(CompatibilityReject {
        code,
        relay_supported_ranges: ranges,
        minimum_client_version: None,
        relay: relay_build(),
    });
    stream
        .write_all(&encode_bootstrap(&reject).map_err(invalid_data)?)
        .await
}

async fn read_bootstrap(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let length = stream.read_u32().await?;
    let length = usize::try_from(length).map_err(invalid_data)?;
    if length > tractor_beam_relay_protocol::v2::MAX_BOOTSTRAP_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bootstrap too large",
        ));
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    let mut frame = Vec::with_capacity(4 + length);
    frame.extend_from_slice(&u32::try_from(length).map_err(invalid_data)?.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

async fn handle_tcp_frame(
    peer_id: PeerId,
    raw: Bytes,
    state: &SharedState,
    egress: &SharedTcpEgress,
    udp: Option<&Arc<UdpSocket>>,
) -> io::Result<bool> {
    match decode_frame(raw.clone()).map_err(invalid_data)? {
        Frame::ClientControl(payload) => {
            let message = decode_client_control(&payload).map_err(invalid_data)?;
            handle_control(peer_id, message, state, egress).await
        }
        Frame::Data(data) => {
            forward_data(data, raw, DataSource::Tcp(peer_id), state, egress, udp).await?;
            Ok(false)
        }
        Frame::ServerControl(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "client sent server control frame",
        )),
    }
}

async fn handle_control(
    peer_id: PeerId,
    message: ClientControl,
    state: &SharedState,
    egress: &SharedTcpEgress,
) -> io::Result<bool> {
    let now = Instant::now();
    match message {
        ClientControl::JoinBegin {
            session_credential,
            steam_id64,
            display_name,
            data_profile,
        } => {
            let begin = JoinBegin {
                control_peer: peer_id,
                session: v2::session_key(&session_credential).map_err(invalid_data)?,
                steam_id64,
                display_name,
                profile: v2::profile(data_profile),
                now,
            };
            let response = match state.lock().await.begin_join(begin) {
                Ok(challenge) => ServerControl::AdmissionChallenge {
                    challenge_id: v2::hex(challenge.challenge_id),
                    algorithm: "sha256".to_owned(),
                    nonce: v2::hex(challenge.pow_nonce),
                    difficulty_bits: challenge.difficulty_bits,
                },
                Err(error) => v2::state_error(error),
            };
            send_control(egress, peer_id, &response).await?;
        }
        ClientControl::JoinProof {
            challenge_id,
            proof,
        } => {
            let response_and_broadcast = state.lock().await.complete_join(
                peer_id,
                v2::decode_hex_16(&challenge_id).map_err(invalid_data)?,
                proof.expose_secret(),
                now,
            );
            match response_and_broadcast {
                Ok((ready, broadcast)) => {
                    RelayMetricsV2::increment(&METRICS.joins);
                    send_control(
                        egress,
                        peer_id,
                        &ServerControl::JoinReady {
                            connection_id: ready.connection_id,
                            resume_credential: v2::secret(ready.resume_key.0),
                            peers: v2::peer_views(ready.peers),
                        },
                    )
                    .await?;
                    if let Some(broadcast) = broadcast {
                        send_presence(egress, broadcast).await;
                    }
                }
                Err(error) => send_control(egress, peer_id, &v2::state_error(error)).await?,
            }
        }
        ClientControl::Resume {
            connection_id,
            resume_credential,
        } => {
            RelayMetricsV2::increment(&METRICS.resumes_attempted);
            let key = v2::resume_key(&resume_credential);
            let result = match key {
                Ok(key) => state.lock().await.resume(peer_id, connection_id, key, now),
                Err(error) => Err(error),
            };
            match result {
                Ok(ready) => {
                    RelayMetricsV2::increment(&METRICS.resumes_succeeded);
                    info!(udp_path_valid = ready.udp_path_valid, "v2 session resumed");
                    send_control(
                        egress,
                        peer_id,
                        &ServerControl::ResumeReady {
                            connection_id: ready.connection_id,
                            peers: v2::peer_views(ready.peers),
                            udp_path_valid: ready.udp_path_valid,
                        },
                    )
                    .await?;
                    if let Some(broadcast) = ready.broadcast {
                        send_presence(egress, broadcast).await;
                    }
                }
                Err(error) => {
                    RelayMetricsV2::increment(&METRICS.resumes_rejected);
                    info!(reason = ?error, "v2 session resume rejected");
                    send_control(egress, peer_id, &v2::resume_rejection(error)).await?
                }
            }
        }
        ClientControl::UdpPathRequest => {
            let state = state.lock().await;
            let Some(connection_id) = state.connection_for_control(peer_id) else {
                send_control(
                    egress,
                    peer_id,
                    &v2::state_error(crate::domain_v2::StateError::UnknownConnection),
                )
                .await?;
                return Ok(false);
            };
            let Some(key) = state.path_key(connection_id) else {
                send_control(
                    egress,
                    peer_id,
                    &v2::state_error(crate::domain_v2::StateError::ProfileMismatch),
                )
                .await?;
                return Ok(false);
            };
            drop(state);
            send_control(
                egress,
                peer_id,
                &ServerControl::UdpPathToken {
                    connection_id,
                    path_token: v2::secret(key.0),
                },
            )
            .await?;
        }
        ClientControl::ControlPing { id } => {
            send_control(egress, peer_id, &ServerControl::ControlPong { id }).await?
        }
        ClientControl::ControlPong { .. } => {}
        ClientControl::Stop => return Ok(true),
        ClientControl::UdpPathHello { .. } => {
            send_control(
                egress,
                peer_id,
                &v2::state_error(crate::domain_v2::StateError::ProfileMismatch),
            )
            .await?;
        }
    }
    Ok(false)
}

async fn udp_task(
    socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedTcpEgress,
) -> io::Result<()> {
    let mut buffer = vec![0_u8; 65_535];
    loop {
        let (size, address) = socket.recv_from(&mut buffer).await?;
        let raw = Bytes::copy_from_slice(&buffer[..size]);
        match decode_frame(raw.clone()) {
            Ok(Frame::ClientControl(payload)) => {
                if let Ok(ClientControl::UdpPathHello {
                    connection_id,
                    path_token,
                }) = decode_client_control(&payload)
                {
                    let result = match v2::path_key(&path_token) {
                        Ok(key) => state.lock().await.bind_udp_path(
                            connection_id,
                            key,
                            address,
                            Instant::now(),
                        ),
                        Err(error) => Err(error),
                    };
                    match result {
                        Ok(peer_id) => {
                            let _ = send_control(
                                &egress,
                                peer_id,
                                &ServerControl::UdpPathReady { connection_id },
                            )
                            .await;
                        }
                        Err(error) => debug!(%address, ?error, "UDP path validation rejected"),
                    }
                }
            }
            Ok(Frame::Data(data)) => {
                if let Err(error) = forward_data(
                    data,
                    raw,
                    DataSource::Udp(address),
                    &state,
                    &egress,
                    Some(&socket),
                )
                .await
                {
                    debug!(%address, %error, "UDP data rejected");
                }
            }
            _ => debug!(%address, "invalid v2 UDP frame"),
        }
    }
}

async fn forward_data(
    data: DataFrame,
    raw: Bytes,
    source: DataSource,
    state: &SharedState,
    egress: &SharedTcpEgress,
    udp: Option<&Arc<UdpSocket>>,
) -> io::Result<()> {
    RelayMetricsV2::increment(&METRICS.data_received);
    let destination = state
        .lock()
        .await
        .route_data(RouteData {
            connection_id: data.connection_id,
            frame_id: data.frame_id,
            from_steam_id64: data.from_steam_id64,
            to_steam_id64: data.to_steam_id64,
            source,
            frame_bytes: raw.len(),
            now: Instant::now(),
        })
        .map_err(|error| {
            match error {
                crate::domain_v2::StateError::DuplicateFrame
                | crate::domain_v2::StateError::FrameTooOld => {
                    RelayMetricsV2::increment(&METRICS.data_duplicates);
                }
                crate::domain_v2::StateError::RateLimited => {
                    RelayMetricsV2::increment(&METRICS.data_rate_limited);
                }
                _ => RelayMetricsV2::increment(&METRICS.data_rejected),
            }
            invalid_data(error)
        })?;
    let result = match destination {
        DataDestination::Tcp(peer_id) => send_frame(egress, peer_id, raw).await,
        DataDestination::Udp(address) => {
            let socket = udp.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "UDP listener unavailable")
            })?;
            socket.send_to(&raw, address).await?;
            Ok(())
        }
    };
    if result.is_ok() {
        RelayMetricsV2::increment(&METRICS.data_forwarded);
    }
    result
}

async fn send_control(
    egress: &SharedTcpEgress,
    peer_id: PeerId,
    message: &ServerControl,
) -> io::Result<()> {
    let payload = encode_server_control(message).map_err(invalid_data)?;
    let frame = Frame::ServerControl(payload)
        .encode()
        .map_err(invalid_data)?;
    send_frame(egress, peer_id, frame).await
}

async fn send_presence(egress: &SharedTcpEgress, broadcast: PresenceBroadcast) {
    let message = ServerControl::PeerPresenceUpdate {
        peers: v2::peer_views(broadcast.peers),
    };
    for recipient in broadcast.recipients {
        let _ = send_control(egress, recipient, &message).await;
    }
}

async fn send_frame(egress: &SharedTcpEgress, peer_id: PeerId, frame: Bytes) -> io::Result<()> {
    let sender = egress.lock().await.get(&peer_id).cloned().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotConnected,
            format!("missing TCP egress for {peer_id}"),
        )
    })?;
    sender
        .try_send(frame)
        .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "TCP egress queue is full"))
}

async fn cleanup_task(state: SharedState, egress: SharedTcpEgress) -> io::Result<()> {
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let broadcasts = state.lock().await.cleanup(Instant::now());
        for _ in &broadcasts {
            RelayMetricsV2::increment(&METRICS.sessions_expired);
            info!("v2 detached session expired");
        }
        for broadcast in broadcasts {
            send_presence(&egress, broadcast).await;
        }
    }
}

async fn metrics_task(state: SharedState) -> io::Result<()> {
    let mut interval = time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        let (rooms, peers) = state.lock().await.active_counts();
        info!(
            rooms,
            peers,
            bootstrap_accepted = RelayMetricsV2::value(&METRICS.bootstrap_accepted),
            bootstrap_rejected = RelayMetricsV2::value(&METRICS.bootstrap_rejected),
            joins = RelayMetricsV2::value(&METRICS.joins),
            control_detached = RelayMetricsV2::value(&METRICS.control_detached),
            resumes_attempted = RelayMetricsV2::value(&METRICS.resumes_attempted),
            resumes_succeeded = RelayMetricsV2::value(&METRICS.resumes_succeeded),
            resumes_rejected = RelayMetricsV2::value(&METRICS.resumes_rejected),
            sessions_expired = RelayMetricsV2::value(&METRICS.sessions_expired),
            data_received = RelayMetricsV2::value(&METRICS.data_received),
            data_forwarded = RelayMetricsV2::value(&METRICS.data_forwarded),
            data_duplicates = RelayMetricsV2::value(&METRICS.data_duplicates),
            data_rate_limited = RelayMetricsV2::value(&METRICS.data_rate_limited),
            data_rejected = RelayMetricsV2::value(&METRICS.data_rejected),
            "v2 relay metrics"
        );
    }
}

fn relay_build() -> BuildMetadata {
    BuildMetadata {
        version: crate::build_info::version_label(),
        git_hash: None,
    }
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tractor_beam_relay_protocol::v2::{
        CAP_TCP_DATA, DataProfile, ProtocolVersion, SecretString, decode_server_control,
        encode_client_control,
    };

    #[tokio::test]
    async fn real_tcp_socket_negotiates_and_joins_v2() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = RelayConfig {
            pow_difficulty_bits: 0,
            udp_bind: None,
            ..RelayConfig::default()
        };
        let server = tokio::spawn(run_with_listeners(listener, None, config));

        let mut stream = TcpStream::connect(address).await.unwrap();
        let hello = BootstrapMessage::ClientHello {
            bootstrap_schema: BOOTSTRAP_SCHEMA,
            supported_protocol_ranges: vec![ProtocolRange {
                major: 2,
                min_minor: 0,
                max_minor: 0,
            }],
            required_capabilities: CAP_TCP_DATA,
            optional_capabilities: CAP_RESUME,
            client: BuildMetadata {
                version: "test".into(),
                git_hash: None,
            },
        };
        stream
            .write_all(&encode_bootstrap(&hello).unwrap())
            .await
            .unwrap();
        let response = decode_bootstrap(&read_bootstrap(&mut stream).await.unwrap()).unwrap();
        assert!(matches!(
            response,
            BootstrapMessage::ServerHello {
                selected_protocol: ProtocolVersion { major: 2, minor: 0 },
                ..
            }
        ));

        let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
        send_client_control(
            &mut framed,
            &ClientControl::JoinBegin {
                session_credential: SecretString::new("1111111111111111"),
                steam_id64: 101,
                display_name: Some("Test".into()),
                data_profile: DataProfile::Tcp,
            },
        )
        .await;
        let challenge = receive_server_control(&mut framed).await;
        let ServerControl::AdmissionChallenge { challenge_id, .. } = challenge else {
            panic!("expected admission challenge");
        };
        send_client_control(
            &mut framed,
            &ClientControl::JoinProof {
                challenge_id,
                proof: SecretString::new(""),
            },
        )
        .await;
        assert!(matches!(
            receive_server_control(&mut framed).await,
            ServerControl::JoinReady { peers, .. } if peers.len() == 1 && peers[0].steam_id64 == 101
        ));
        server.abort();
    }

    #[tokio::test]
    async fn real_tcp_socket_returns_structured_bootstrap_rejection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = RelayConfig {
            udp_bind: None,
            ..RelayConfig::default()
        };
        let server = tokio::spawn(run_with_listeners(listener, None, config));

        let mut stream = TcpStream::connect(address).await.unwrap();
        let hello = BootstrapMessage::ClientHello {
            bootstrap_schema: BOOTSTRAP_SCHEMA + 1,
            supported_protocol_ranges: vec![ProtocolRange {
                major: 2,
                min_minor: 0,
                max_minor: 0,
            }],
            required_capabilities: CAP_TCP_DATA,
            optional_capabilities: CAP_RESUME,
            client: BuildMetadata {
                version: "incompatible-test".into(),
                git_hash: None,
            },
        };
        stream
            .write_all(&encode_bootstrap(&hello).unwrap())
            .await
            .unwrap();

        let response = decode_bootstrap(&read_bootstrap(&mut stream).await.unwrap()).unwrap();
        assert!(matches!(
            response,
            BootstrapMessage::CompatibilityReject(CompatibilityReject {
                code: RejectCode::UnsupportedBootstrapSchema,
                ..
            })
        ));
        server.abort();
    }

    async fn send_client_control(
        framed: &mut Framed<TcpStream, LengthDelimitedCodec>,
        message: &ClientControl,
    ) {
        let payload = encode_client_control(message).unwrap();
        framed
            .send(Frame::ClientControl(payload).encode().unwrap())
            .await
            .unwrap();
    }

    async fn receive_server_control(
        framed: &mut Framed<TcpStream, LengthDelimitedCodec>,
    ) -> ServerControl {
        let raw = framed.next().await.unwrap().unwrap().freeze();
        let Frame::ServerControl(payload) = decode_frame(raw).unwrap() else {
            panic!("expected server control frame");
        };
        decode_server_control(&payload).unwrap()
    }
}
