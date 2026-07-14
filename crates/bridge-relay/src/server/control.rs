use std::{
    io,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpStream, UdpSocket},
    sync::mpsc,
    time,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{Instrument as _, Span, debug, info, warn};
use tractor_beam_relay_protocol::{
    BOOTSTRAP_SCHEMA, BootstrapMessage, BuildMetadata, CAP_RESUME, CAP_ROOM_PATH_PROBE,
    CAP_TCP_DATA, CAP_UDP_DATA, ClientControl, CompatibilityReject, Frame, ProtocolRange,
    RejectCode, decode_bootstrap, decode_client_control, decode_frame, encode_bootstrap,
    select_capabilities, select_protocol,
};

use super::{
    SharedState, SharedTcpEgress, TcpTaskContext,
    data::{forward_data, forward_probe, send_presence},
    establishment::{EstablishmentAttempt, EstablishmentRegistry, Milestone, mark_span},
    invalid_data,
};
use crate::{
    domain::{DataSource, PeerId},
    metrics::RelayMetrics,
};

mod commands;

pub(super) async fn tcp_task(
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
        establishments,
    } = context;
    let _connection = metrics.start_connection();
    let mut establishment = Some(EstablishmentAttempt::start(peer_id, Arc::clone(&metrics)));
    let negotiation_span = establishment.as_ref().map_or_else(Span::none, |attempt| {
        attempt.milestone_span(Milestone::Bootstrap)
    });
    let negotiation = time::timeout(
        Duration::from_secs(5),
        negotiate(&mut stream, address, udp.is_some()).instrument(negotiation_span.clone()),
    )
    .await;
    let enabled_capabilities = match negotiation {
        Ok(Ok(enabled)) => enabled,
        other => {
            mark_span(&negotiation_span, "rejected", Some("bootstrap_rejected"));
            metrics.record_control("bootstrap", "rejected");
            if let Ok(Err(error)) = other {
                warn!(%address, %error, "bootstrap failed");
            }
            if let Some(attempt) = establishment.take() {
                attempt.finish("rejected", Some("bootstrap_rejected"));
            }
            return;
        }
    };
    mark_span(&negotiation_span, "accepted", None);
    drop(negotiation_span);
    metrics.record_control("bootstrap", "accepted");

    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(max_frame_size.max(16 * 1024 + 16))
        .new_codec();
    let framed = Framed::new(stream, codec);
    let (mut sink, mut inbound) = framed.split();
    let (outbound_tx, mut outbound_rx) = mpsc::channel(queue_capacity);
    egress.lock().await.insert(peer_id, outbound_tx);
    let mut explicit_stop = false;
    let establishment_deadline = time::sleep(EstablishmentAttempt::deadline());
    tokio::pin!(establishment_deadline);

    loop {
        tokio::select! {
            () = &mut establishment_deadline, if establishment.is_some() => {
                if let Some(attempt) = establishment.take() {
                    attempt.finish("timeout", Some("establishment_timeout"));
                }
            }
            frame = inbound.next() => {
                let Some(frame) = frame else { break; };
                match frame {
                    Ok(bytes) => match handle_tcp_frame(
                        bytes.freeze(),
                        TcpFrameContext {
                            peer_id,
                            enabled_capabilities,
                            state: &state,
                            egress: &egress,
                            udp: udp.as_ref(),
                            metrics: &metrics,
                            establishments: &establishments,
                        },
                        &mut establishment,
                    ).await {
                        Ok(stop) => {
                            if stop { explicit_stop = true; break; }
                        }
                        Err(error) => {
                            if let Some(attempt) = establishment.take() {
                                attempt.finish("failed", Some("tcp_frame_rejected"));
                            }
                            warn!(%peer_id, %address, %error, "TCP frame rejected");
                            break;
                        }
                    },
                    Err(error) => {
                        if let Some(attempt) = establishment.take() {
                            attempt.finish("failed", Some("tcp_framing_failed"));
                        }
                        warn!(%peer_id, %address, %error, "TCP framing failed");
                        break;
                    }
                }
            }
            Some(frame) = outbound_rx.recv() => {
                if let Err(error) = sink.send(frame).await {
                    if let Some(attempt) = establishment.take() {
                        attempt.finish("failed", Some("tcp_egress_failed"));
                    }
                    debug!(%peer_id, %address, %error, "TCP send ended");
                    break;
                }
            }
        }
    }

    egress.lock().await.remove(&peer_id);
    if let Some(attempt) = establishment.take() {
        attempt.finish("disconnected", Some("control_disconnected"));
    }
    if let Some(connection_id) = state.lock().await.connection_for_control(peer_id) {
        establishments.disconnect(connection_id).await;
    }
    let broadcast = if explicit_stop {
        state.lock().await.stop(peer_id)
    } else {
        state.lock().await.detach(peer_id, Instant::now())
    };
    if let Some(broadcast) = broadcast {
        send_presence(&egress, &metrics, broadcast).await;
    }
    if explicit_stop {
        info!(%peer_id, %address, "session stopped");
    } else {
        metrics.record_control("detach", "accepted");
        info!(%peer_id, %address, grace_seconds = 120, "control connection detached");
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
        "bootstrap accepted"
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

pub(super) async fn read_bootstrap(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let length = stream.read_u32().await?;
    let length = usize::try_from(length).map_err(invalid_data)?;
    if length > tractor_beam_relay_protocol::MAX_BOOTSTRAP_PAYLOAD {
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
    raw: Bytes,
    context: TcpFrameContext<'_>,
    establishment: &mut Option<EstablishmentAttempt>,
) -> io::Result<bool> {
    let TcpFrameContext {
        peer_id,
        enabled_capabilities,
        state,
        egress,
        udp,
        metrics,
        establishments,
    } = context;
    match decode_frame(raw.clone()).map_err(invalid_data)? {
        Frame::ClientControl(payload) => {
            let message = decode_client_control(&payload).map_err(invalid_data)?;
            let span = establishment.as_ref().map_or_else(Span::none, |attempt| {
                control_milestone(&message)
                    .map_or_else(Span::none, |milestone| attempt.milestone_span(milestone))
            });
            let result = commands::handle_control(
                peer_id,
                enabled_capabilities,
                message,
                state,
                egress,
                metrics,
            )
            .instrument(span.clone())
            .await;
            match result {
                Ok(result) => {
                    apply_establishment_event(result.event, &span, establishment, establishments)
                        .await;
                    Ok(result.stop)
                }
                Err(error) => {
                    mark_span(&span, "failed", Some("control_io_failed"));
                    if let Some(attempt) = establishment.take() {
                        attempt.finish("failed", Some("control_io_failed"));
                    }
                    Err(error)
                }
            }
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

struct TcpFrameContext<'a> {
    peer_id: PeerId,
    enabled_capabilities: u64,
    state: &'a SharedState,
    egress: &'a SharedTcpEgress,
    udp: Option<&'a Arc<UdpSocket>>,
    metrics: &'a RelayMetrics,
    establishments: &'a EstablishmentRegistry,
}

struct ControlResult {
    stop: bool,
    event: EstablishmentEvent,
}

impl ControlResult {
    const CONTINUE: Self = Self {
        stop: false,
        event: EstablishmentEvent::None,
    };
    const STOP: Self = Self {
        stop: true,
        event: EstablishmentEvent::None,
    };

    const fn continue_with(event: EstablishmentEvent) -> Self {
        Self { stop: false, event }
    }
}

enum EstablishmentEvent {
    None,
    Progress,
    Rejected(&'static str),
    Established {
        operation: &'static str,
        profile: &'static str,
        connection_id: u64,
        wait_for_udp: bool,
    },
}

async fn apply_establishment_event(
    event: EstablishmentEvent,
    span: &Span,
    attempt: &mut Option<EstablishmentAttempt>,
    registry: &EstablishmentRegistry,
) {
    match event {
        EstablishmentEvent::None => {}
        EstablishmentEvent::Progress => mark_span(span, "accepted", None),
        EstablishmentEvent::Rejected(error) => {
            mark_span(span, "rejected", Some(error));
            if let Some(attempt) = attempt.take() {
                attempt.finish("rejected", Some(error));
            }
        }
        EstablishmentEvent::Established {
            operation,
            profile,
            connection_id,
            wait_for_udp,
        } => {
            mark_span(span, "accepted", None);
            if let Some(mut attempt) = attempt.take() {
                attempt.set_route(operation, profile);
                if wait_for_udp {
                    registry.wait_for_udp(connection_id, attempt).await;
                } else {
                    attempt.finish("accepted", None);
                }
            }
        }
    }
}

const fn control_milestone(message: &ClientControl) -> Option<Milestone> {
    match message {
        ClientControl::JoinBegin { .. } => Some(Milestone::JoinBegin),
        ClientControl::JoinProof { .. } => Some(Milestone::JoinProof),
        ClientControl::Resume { .. } => Some(Milestone::Resume),
        _ => None,
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
