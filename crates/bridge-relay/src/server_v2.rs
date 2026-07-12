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
use tracing::{Instrument as _, debug, info, warn};
use tractor_beam_relay_protocol::v2::{
    BOOTSTRAP_SCHEMA, BootstrapMessage, BuildMetadata, CAP_RESUME, CAP_ROOM_PATH_PROBE,
    CAP_TCP_DATA, CAP_UDP_DATA, ClientControl, CompatibilityReject, DataFrame, Frame, ProbeFrame,
    ProtocolRange, RejectCode, ServerControl, decode_bootstrap, decode_client_control,
    decode_frame, encode_bootstrap, encode_server_control, select_capabilities, select_protocol,
};

use crate::{
    config::RelayConfig,
    domain::PeerId,
    domain_v2::{DataDestination, DataSource, JoinBegin, PresenceBroadcast, RouteData, RouteProbe},
    metrics_v2::RelayMetricsV2,
    peer_registry::PeerRegistry,
    state_v2::RelayStateV2,
    v2,
};

type SharedState = Arc<Mutex<RelayStateV2>>;
type SharedTcpEgress = Arc<Mutex<HashMap<PeerId, mpsc::Sender<Bytes>>>>;
type SharedMetrics = Arc<RelayMetricsV2>;

struct TcpTaskContext {
    state: SharedState,
    egress: SharedTcpEgress,
    udp: Option<Arc<UdpSocket>>,
    max_frame_size: usize,
    queue_capacity: usize,
    metrics: SharedMetrics,
}

pub(crate) async fn run(config: RelayConfig, metrics: SharedMetrics) -> io::Result<()> {
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
    run_with_listeners(listener, udp, config, metrics).await
}

async fn run_with_listeners(
    listener: TcpListener,
    udp: Option<Arc<UdpSocket>>,
    config: RelayConfig,
    metrics: SharedMetrics,
) -> io::Result<()> {
    let state = Arc::new(Mutex::new(RelayStateV2::new(config.clone())));
    let egress = Arc::new(Mutex::new(HashMap::new()));
    let registry = Arc::new(Mutex::new(PeerRegistry::default()));
    if let Some(socket) = udp.clone() {
        tokio::spawn(udp_task(
            socket,
            Arc::clone(&state),
            Arc::clone(&egress),
            Arc::clone(&metrics),
        ));
    }
    tokio::spawn(cleanup_task(
        Arc::clone(&state),
        Arc::clone(&egress),
        Arc::clone(&metrics),
    ));
    tokio::spawn(metrics_task(
        Arc::clone(&state),
        Arc::clone(&egress),
        config.tcp_egress_queue_capacity,
        Arc::clone(&metrics),
    ));

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
                metrics: Arc::clone(&metrics),
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
        metrics,
    } = context;
    let negotiation_span = tracing::info_span!(
        "relay.bootstrap",
        network.transport = "tcp",
        otel.status_code = tracing::field::Empty,
        error.type = tracing::field::Empty
    );
    let negotiation = time::timeout(
        Duration::from_secs(5),
        negotiate(&mut stream, address, udp.is_some()).instrument(negotiation_span.clone()),
    )
    .await;
    let enabled_capabilities = match negotiation {
        Ok(Ok(enabled)) => enabled,
        other => {
            negotiation_span.record("otel.status_code", "ERROR");
            negotiation_span.record("error.type", "bootstrap_rejected");
            metrics.control(&metrics.bootstrap_rejected, "bootstrap", "rejected");
            if let Ok(Err(error)) = other {
                warn!(%address, %error, "v2 bootstrap failed");
            }
            return;
        }
    };
    metrics.control(&metrics.bootstrap_accepted, "bootstrap", "accepted");

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
                    Ok(bytes) => match handle_tcp_frame(peer_id, enabled_capabilities, bytes.freeze(), &state, &egress, udp.as_ref(), &metrics).await {
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
        metrics.control(&metrics.control_detached, "detach", "accepted");
        info!(%peer_id, %address, grace_seconds = 120, "v2 control connection detached");
    }
}

async fn negotiate(
    stream: &mut TcpStream,
    address: SocketAddr,
    udp_enabled: bool,
) -> io::Result<u64> {
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
    let available = CAP_TCP_DATA
        | CAP_RESUME
        | CAP_ROOM_PATH_PROBE
        | if udp_enabled { CAP_UDP_DATA } else { 0 };
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
    Ok(enabled)
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
    enabled_capabilities: u64,
    raw: Bytes,
    state: &SharedState,
    egress: &SharedTcpEgress,
    udp: Option<&Arc<UdpSocket>>,
    metrics: &RelayMetricsV2,
) -> io::Result<bool> {
    match decode_frame(raw.clone()).map_err(invalid_data)? {
        Frame::ClientControl(payload) => {
            let message = decode_client_control(&payload).map_err(invalid_data)?;
            let operation = control_operation_name(&message);
            let span = tracing::info_span!(
                "relay.control",
                operation,
                otel.status_code = tracing::field::Empty,
                error.type = tracing::field::Empty
            );
            let result = handle_control(
                peer_id,
                enabled_capabilities,
                message,
                state,
                egress,
                metrics,
            )
            .instrument(span.clone())
            .await;
            if result.is_err() {
                span.record("otel.status_code", "ERROR");
                span.record("error.type", "control_rejected");
            }
            result
        }
        Frame::Data(data) => {
            forward_data(
                data,
                raw,
                DataSource::Tcp(peer_id),
                state,
                egress,
                udp,
                metrics,
            )
            .await?;
            Ok(false)
        }
        Frame::Probe(probe) => {
            if let Err(error) = forward_probe(
                probe,
                raw,
                DataSource::Tcp(peer_id),
                state,
                egress,
                udp,
                metrics,
            )
            .await
            {
                debug!(%peer_id, %error, "TCP probe rejected");
            }
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
    enabled_capabilities: u64,
    message: ClientControl,
    state: &SharedState,
    egress: &SharedTcpEgress,
    metrics: &RelayMetricsV2,
) -> io::Result<bool> {
    let now = Instant::now();
    let operation = control_operation_name(&message);
    let _duration = metrics.start_control_duration(operation);
    metrics.record_control(operation, "attempted");
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
                capabilities: enabled_capabilities,
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
                    metrics.control(&metrics.joins, "join_proof", "accepted");
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
            RelayMetricsV2::increment_local(&metrics.resumes_attempted);
            let key = v2::resume_key(&resume_credential);
            let result = match key {
                Ok(key) => state.lock().await.resume(peer_id, connection_id, key, now),
                Err(error) => Err(error),
            };
            match result {
                Ok(ready) => {
                    metrics.control(&metrics.resumes_succeeded, "resume", "accepted");
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
                    metrics.control(&metrics.resumes_rejected, "resume", "rejected");
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
    metrics: SharedMetrics,
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
                    &metrics,
                )
                .await
                {
                    debug!(%address, %error, "UDP data rejected");
                }
            }
            Ok(Frame::Probe(probe)) => {
                if let Err(error) = forward_probe(
                    probe,
                    raw,
                    DataSource::Udp(address),
                    &state,
                    &egress,
                    Some(&socket),
                    &metrics,
                )
                .await
                {
                    debug!(%address, %error, "UDP probe rejected");
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
    metrics: &RelayMetricsV2,
) -> io::Result<()> {
    let frame_bytes = raw.len();
    let transport = source.transport_name();
    let started = Instant::now();
    let (destination, room_metric_id) = {
        let mut state = state.lock().await;
        let room_metric_id = state
            .room_metric_id_for_connection(data.connection_id)
            .unwrap_or_default();
        let destination = state.route_data(RouteData {
            connection_id: data.connection_id,
            frame_id: data.frame_id,
            from_steam_id64: data.from_steam_id64,
            to_steam_id64: data.to_steam_id64,
            source,
            frame_bytes,
            now: Instant::now(),
        });
        (destination, room_metric_id)
    };
    let destination = destination.map_err(|error| {
        match error {
            crate::domain_v2::StateError::DuplicateFrame
            | crate::domain_v2::StateError::FrameTooOld => {
                metrics.data(
                    &metrics.data_duplicates,
                    transport,
                    "inbound",
                    "game",
                    "duplicate",
                    0,
                );
            }
            crate::domain_v2::StateError::RateLimited => {
                metrics.data(
                    &metrics.data_rate_limited,
                    transport,
                    "inbound",
                    "game",
                    "rate_limited",
                    0,
                );
            }
            _ => metrics.data(
                &metrics.data_rejected,
                transport,
                "inbound",
                "game",
                "rejected",
                0,
            ),
        }
        invalid_data(error)
    })?;
    metrics.data(
        &metrics.data_received,
        transport,
        "inbound",
        "game",
        "accepted",
        frame_bytes,
    );
    let sampled = metrics.should_trace_data(room_metric_id, data.frame_id);
    let span = tracing::info_span!(
        "relay.data.dispatch",
        network.transport = transport,
        frame.type = "game",
        room.metric_id = room_metric_id,
        frame.id = data.frame_id,
        otel.status_code = tracing::field::Empty,
        error.type = tracing::field::Empty
    );
    let dispatch = async {
        match destination {
            DataDestination::Tcp(peer_id) => send_frame(egress, peer_id, raw).await,
            DataDestination::Udp(address) => {
                let socket = udp.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotConnected, "UDP listener unavailable")
                })?;
                socket.send_to(&raw, address).await?;
                Ok(())
            }
        }
    };
    let result = if sampled {
        dispatch.instrument(span.clone()).await
    } else {
        dispatch.await
    };
    if result.is_err() && sampled {
        span.record("otel.status_code", "ERROR");
        span.record("error.type", "egress_failed");
    }
    if result.is_ok() {
        metrics.data(
            &metrics.data_forwarded,
            destination.transport_name(),
            "outbound",
            "game",
            "forwarded",
            frame_bytes,
        );
    }
    metrics.record_dispatch_duration(transport, "game", started.elapsed().as_secs_f64());
    result
}

async fn forward_probe(
    probe: ProbeFrame,
    raw: Bytes,
    source: DataSource,
    state: &SharedState,
    egress: &SharedTcpEgress,
    udp: Option<&Arc<UdpSocket>>,
    metrics: &RelayMetricsV2,
) -> io::Result<()> {
    let frame_bytes = raw.len();
    let transport = source.transport_name();
    let started = Instant::now();
    let (destination, room_metric_id) = {
        let mut state = state.lock().await;
        let room_metric_id = state
            .room_metric_id_for_connection(probe.connection_id)
            .unwrap_or_default();
        let destination = state.route_probe(RouteProbe {
            connection_id: probe.connection_id,
            from_steam_id64: probe.from_steam_id64,
            to_steam_id64: probe.to_steam_id64,
            source,
            frame_bytes,
            now: Instant::now(),
        });
        (destination, room_metric_id)
    };
    let destination = destination.map_err(|error| {
        let outcome = match error {
            crate::domain_v2::StateError::RateLimited
            | crate::domain_v2::StateError::ProbeRateLimited => "rate_limited",
            _ => "rejected",
        };
        metrics.data(
            &metrics.data_rejected,
            transport,
            "inbound",
            "probe",
            outcome,
            0,
        );
        invalid_data(error)
    })?;
    metrics.data(
        &metrics.data_received,
        transport,
        "inbound",
        "probe",
        "accepted",
        frame_bytes,
    );
    let sampled = metrics.should_trace_data(room_metric_id, probe.probe_id);
    let span = tracing::info_span!(
        "relay.probe.dispatch",
        network.transport = transport,
        frame.type = "probe",
        room.metric_id = room_metric_id,
        probe.id = probe.probe_id,
        probe.phase = ?probe.phase,
        otel.status_code = tracing::field::Empty,
        error.type = tracing::field::Empty
    );
    let dispatch = async {
        match destination {
            DataDestination::Tcp(peer_id) => send_frame(egress, peer_id, raw).await,
            DataDestination::Udp(address) => {
                let socket = udp.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotConnected, "UDP listener unavailable")
                })?;
                socket.send_to(&raw, address).await?;
                Ok(())
            }
        }
    };
    let result = if sampled {
        dispatch.instrument(span.clone()).await
    } else {
        dispatch.await
    };
    if result.is_err() && sampled {
        span.record("otel.status_code", "ERROR");
        span.record("error.type", "egress_failed");
    }
    if result.is_ok() {
        metrics.data(
            &metrics.data_forwarded,
            destination.transport_name(),
            "outbound",
            "probe",
            "forwarded",
            frame_bytes,
        );
    }
    metrics.record_dispatch_duration(transport, "probe", started.elapsed().as_secs_f64());
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

async fn cleanup_task(
    state: SharedState,
    egress: SharedTcpEgress,
    metrics: SharedMetrics,
) -> io::Result<()> {
    let mut interval = time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let broadcasts = state.lock().await.cleanup(Instant::now());
        for _ in &broadcasts {
            metrics.control(&metrics.sessions_expired, "session_expire", "accepted");
            info!("v2 detached session expired");
        }
        for broadcast in broadcasts {
            send_presence(&egress, broadcast).await;
        }
    }
}

async fn metrics_task(
    state: SharedState,
    egress: SharedTcpEgress,
    queue_capacity: usize,
    metrics: SharedMetrics,
) -> io::Result<()> {
    let mut interval = time::interval(Duration::from_secs(30));
    loop {
        interval.tick().await;
        let state = state.lock().await;
        let (rooms, peers) = state.active_counts();
        let peer_counts = state.active_peer_counts();
        drop(state);
        let max_queue_utilization = egress
            .lock()
            .await
            .values()
            .map(|sender| 1.0 - sender.capacity() as f64 / queue_capacity as f64)
            .fold(0.0_f64, f64::max);
        metrics.record_snapshot(rooms, peer_counts, max_queue_utilization);
        info!(
            rooms,
            peers,
            bootstrap_accepted = RelayMetricsV2::value(&metrics.bootstrap_accepted),
            bootstrap_rejected = RelayMetricsV2::value(&metrics.bootstrap_rejected),
            joins = RelayMetricsV2::value(&metrics.joins),
            control_detached = RelayMetricsV2::value(&metrics.control_detached),
            resumes_attempted = RelayMetricsV2::value(&metrics.resumes_attempted),
            resumes_succeeded = RelayMetricsV2::value(&metrics.resumes_succeeded),
            resumes_rejected = RelayMetricsV2::value(&metrics.resumes_rejected),
            sessions_expired = RelayMetricsV2::value(&metrics.sessions_expired),
            data_received = RelayMetricsV2::value(&metrics.data_received),
            data_forwarded = RelayMetricsV2::value(&metrics.data_forwarded),
            data_duplicates = RelayMetricsV2::value(&metrics.data_duplicates),
            data_rate_limited = RelayMetricsV2::value(&metrics.data_rate_limited),
            data_rejected = RelayMetricsV2::value(&metrics.data_rejected),
            "v2 relay metrics"
        );
    }
}

const fn control_operation_name(message: &ClientControl) -> &'static str {
    match message {
        ClientControl::JoinBegin { .. } => "join_begin",
        ClientControl::JoinProof { .. } => "join_proof",
        ClientControl::Resume { .. } => "resume",
        ClientControl::UdpPathRequest => "udp_path_request",
        ClientControl::ControlPing { .. } => "ping",
        ClientControl::ControlPong { .. } => "pong",
        ClientControl::Stop => "stop",
        ClientControl::UdpPathHello { .. } => "udp_path_hello",
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
        CAP_ROOM_PATH_PROBE, CAP_TCP_DATA, CAP_UDP_DATA, DataProfile, ProbeFrame, ProbePhase,
        ProtocolVersion, SecretString, decode_server_control, encode_client_control,
    };

    fn test_metrics() -> SharedMetrics {
        Arc::new(RelayMetricsV2::new(
            &opentelemetry::global::meter("tractor-beam-relay-test"),
            "test",
            1.0,
        ))
    }

    #[tokio::test]
    async fn real_tcp_socket_negotiates_and_joins_v2() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = RelayConfig {
            pow_difficulty_bits: 0,
            udp_bind: None,
            ..RelayConfig::default()
        };
        let server = tokio::spawn(run_with_listeners(listener, None, config, test_metrics()));

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
        let server = tokio::spawn(run_with_listeners(listener, None, config, test_metrics()));

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

    #[tokio::test]
    async fn real_tcp_socket_forwards_probe_between_capable_peers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = RelayConfig {
            pow_difficulty_bits: 0,
            udp_bind: None,
            ..RelayConfig::default()
        };
        let server = tokio::spawn(run_with_listeners(listener, None, config, test_metrics()));
        let (mut first, first_connection_id) = connect_joined_peer(address, 101).await;
        let (mut second, _) = connect_joined_peer(address, 202).await;

        let probe = ProbeFrame {
            connection_id: first_connection_id,
            probe_id: 7,
            from_steam_id64: 101,
            to_steam_id64: 202,
            phase: ProbePhase::Request,
        };
        first
            .send(Frame::Probe(probe).encode().unwrap())
            .await
            .unwrap();
        let received = loop {
            let raw = second.next().await.unwrap().unwrap().freeze();
            if let Frame::Probe(probe) = decode_frame(raw).unwrap() {
                break probe;
            }
        };
        assert_eq!(received, probe);
        server.abort();
    }

    #[tokio::test]
    async fn rejected_tcp_probe_does_not_close_control_session() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = RelayConfig {
            pow_difficulty_bits: 0,
            udp_bind: None,
            ..RelayConfig::default()
        };
        let server = tokio::spawn(run_with_listeners(listener, None, config, test_metrics()));
        let (mut peer, connection_id) = connect_joined_peer(address, 101).await;

        peer.send(
            Frame::Probe(ProbeFrame {
                connection_id,
                probe_id: 8,
                from_steam_id64: 101,
                to_steam_id64: 999,
                phase: ProbePhase::Request,
            })
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
        send_client_control(&mut peer, &ClientControl::ControlPing { id: 42 }).await;

        assert!(matches!(
            time::timeout(Duration::from_secs(1), receive_server_control(&mut peer))
                .await
                .unwrap(),
            ServerControl::ControlPong { id: 42 }
        ));
        server.abort();
    }

    #[tokio::test]
    async fn real_udp_socket_forwards_probe_without_tcp_fallback() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp_address = listener.local_addr().unwrap();
        let relay_udp = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let udp_address = relay_udp.local_addr().unwrap();
        let config = RelayConfig {
            pow_difficulty_bits: 0,
            udp_bind: Some(udp_address.to_string()),
            ..RelayConfig::default()
        };
        let server = tokio::spawn(run_with_listeners(
            listener,
            Some(relay_udp),
            config,
            test_metrics(),
        ));
        let (_first_control, first_udp, first_connection_id) =
            connect_joined_udp_peer(tcp_address, udp_address, 301).await;
        let (_second_control, second_udp, _) =
            connect_joined_udp_peer(tcp_address, udp_address, 302).await;

        let probe = ProbeFrame {
            connection_id: first_connection_id,
            probe_id: 9,
            from_steam_id64: 301,
            to_steam_id64: 302,
            phase: ProbePhase::Request,
        };
        first_udp.send(&probe.encode().unwrap()).await.unwrap();
        let mut buffer = [0_u8; 128];
        let size = time::timeout(Duration::from_secs(1), second_udp.recv(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            decode_frame(Bytes::copy_from_slice(&buffer[..size])).unwrap(),
            Frame::Probe(probe)
        );
        server.abort();
    }

    async fn connect_joined_peer(
        address: SocketAddr,
        steam_id64: u64,
    ) -> (Framed<TcpStream, LengthDelimitedCodec>, u64) {
        let mut stream = TcpStream::connect(address).await.unwrap();
        let hello = BootstrapMessage::ClientHello {
            bootstrap_schema: BOOTSTRAP_SCHEMA,
            supported_protocol_ranges: vec![ProtocolRange {
                major: 2,
                min_minor: 0,
                max_minor: 0,
            }],
            required_capabilities: CAP_TCP_DATA,
            optional_capabilities: CAP_RESUME | CAP_ROOM_PATH_PROBE,
            client: BuildMetadata {
                version: "probe-test".into(),
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
            BootstrapMessage::ServerHello { enabled_capabilities, .. }
                if enabled_capabilities & CAP_ROOM_PATH_PROBE != 0
        ));
        let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
        send_client_control(
            &mut framed,
            &ClientControl::JoinBegin {
                session_credential: SecretString::new("1111111111111111"),
                steam_id64,
                display_name: None,
                data_profile: DataProfile::Tcp,
            },
        )
        .await;
        let ServerControl::AdmissionChallenge { challenge_id, .. } =
            receive_server_control(&mut framed).await
        else {
            panic!("expected challenge")
        };
        send_client_control(
            &mut framed,
            &ClientControl::JoinProof {
                challenge_id,
                proof: SecretString::new(""),
            },
        )
        .await;
        let ready = receive_server_control(&mut framed).await;
        let ServerControl::JoinReady { connection_id, .. } = ready else {
            panic!("expected join ready")
        };
        (framed, connection_id)
    }

    async fn connect_joined_udp_peer(
        tcp_address: SocketAddr,
        udp_address: SocketAddr,
        steam_id64: u64,
    ) -> (Framed<TcpStream, LengthDelimitedCodec>, UdpSocket, u64) {
        let mut stream = TcpStream::connect(tcp_address).await.unwrap();
        let hello = BootstrapMessage::ClientHello {
            bootstrap_schema: BOOTSTRAP_SCHEMA,
            supported_protocol_ranges: vec![ProtocolRange {
                major: 2,
                min_minor: 0,
                max_minor: 0,
            }],
            required_capabilities: CAP_UDP_DATA,
            optional_capabilities: CAP_RESUME | CAP_ROOM_PATH_PROBE,
            client: BuildMetadata {
                version: "udp-probe-test".into(),
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
            BootstrapMessage::ServerHello { enabled_capabilities, .. }
                if enabled_capabilities & (CAP_UDP_DATA | CAP_ROOM_PATH_PROBE)
                    == CAP_UDP_DATA | CAP_ROOM_PATH_PROBE
        ));
        let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
        send_client_control(
            &mut framed,
            &ClientControl::JoinBegin {
                session_credential: SecretString::new("1111111111111111"),
                steam_id64,
                display_name: None,
                data_profile: DataProfile::Udp,
            },
        )
        .await;
        let ServerControl::AdmissionChallenge { challenge_id, .. } =
            receive_server_control(&mut framed).await
        else {
            panic!("expected challenge")
        };
        send_client_control(
            &mut framed,
            &ClientControl::JoinProof {
                challenge_id,
                proof: SecretString::new(""),
            },
        )
        .await;
        let ServerControl::JoinReady { connection_id, .. } =
            receive_server_control(&mut framed).await
        else {
            panic!("expected join ready")
        };
        send_client_control(&mut framed, &ClientControl::UdpPathRequest).await;
        let ServerControl::UdpPathToken { path_token, .. } =
            receive_server_control(&mut framed).await
        else {
            panic!("expected path token")
        };
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        udp.connect(udp_address).await.unwrap();
        let payload = encode_client_control(&ClientControl::UdpPathHello {
            connection_id,
            path_token,
        })
        .unwrap();
        udp.send(&Frame::ClientControl(payload).encode().unwrap())
            .await
            .unwrap();
        assert!(matches!(
            receive_server_control(&mut framed).await,
            ServerControl::UdpPathReady { connection_id: ready } if ready == connection_id
        ));
        (framed, udp, connection_id)
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
