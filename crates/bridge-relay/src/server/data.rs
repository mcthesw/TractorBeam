use std::{fmt, io, sync::Arc, time::Instant};

use bytes::Bytes;
use tokio::net::UdpSocket;
use tracing::{Instrument as _, debug};
use tractor_beam_relay_protocol::{
    ClientControl, DataFrame, Frame, ProbeFrame, ServerControl, decode_client_control,
    decode_frame, encode_server_control,
};

use super::{SharedEstablishments, SharedMetrics, SharedState, SharedTcpEgress, invalid_data};
use crate::{
    domain::{
        DataDestination, DataSource, PeerId, PresenceBroadcast, RouteData, RouteProbe, StateError,
    },
    metrics::RelayMetrics,
    protocol,
};

#[derive(Debug)]
pub(super) enum ForwardDataError {
    Route(StateError),
    Dispatch(io::Error),
}

impl ForwardDataError {
    pub(super) fn is_fatal_for_tcp_sender(&self) -> bool {
        matches!(
            self,
            Self::Route(
                StateError::UnknownConnection
                    | StateError::SenderMismatch
                    | StateError::ProfileMismatch
                    | StateError::PathNotValidated
            )
        )
    }
}

impl fmt::Display for ForwardDataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Route(error) => error.fmt(formatter),
            Self::Dispatch(error) => error.fmt(formatter),
        }
    }
}

pub(super) async fn udp_task(
    socket: Arc<UdpSocket>,
    state: SharedState,
    egress: SharedTcpEgress,
    metrics: SharedMetrics,
    establishments: SharedEstablishments,
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
                    let span = establishments.udp_span(connection_id).await;
                    let validation = async {
                        match protocol::path_key(&path_token) {
                            Ok(key) => state.lock().await.bind_udp_path(
                                connection_id,
                                key,
                                address,
                                Instant::now(),
                            ),
                            Err(error) => Err(error),
                        }
                    };
                    let result = validation.instrument(span.clone()).await;
                    match result {
                        Ok(peer_id) => {
                            super::establishment::mark_span(&span, "accepted", None);
                            establishments.complete_udp(connection_id).await;
                            let _ = send_control(
                                &egress,
                                &metrics,
                                peer_id,
                                &ServerControl::UdpPathReady { connection_id },
                            )
                            .await;
                        }
                        Err(error) => {
                            super::establishment::mark_span(
                                &span,
                                "rejected",
                                Some("udp_validation_rejected"),
                            );
                            debug!(%address, ?error, "UDP path validation rejected");
                        }
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
            _ => debug!(%address, "invalid UDP frame"),
        }
    }
}

pub(super) async fn forward_data(
    data: DataFrame,
    raw: Bytes,
    source: DataSource,
    state: &SharedState,
    egress: &SharedTcpEgress,
    udp: Option<&Arc<UdpSocket>>,
    metrics: &RelayMetrics,
) -> Result<(), ForwardDataError> {
    let frame_bytes = raw.len();
    let transport = source.transport_name();
    let started = Instant::now();
    let destination = {
        let mut state = state.lock().await;
        state.route_data(RouteData {
            connection_id: data.connection_id,
            frame_id: data.frame_id,
            from_steam_id64: data.from_steam_id64,
            to_steam_id64: data.to_steam_id64,
            source,
            frame_bytes,
            now: Instant::now(),
        })
    };
    let destination = destination.map_err(|error| {
        match &error {
            StateError::DuplicateFrame | StateError::FrameTooOld => {
                metrics.record_data(transport, "inbound", "game", "duplicate", 0);
            }
            StateError::RateLimited => {
                metrics.record_data(transport, "inbound", "game", "rate_limited", 0);
            }
            _ => metrics.record_data(transport, "inbound", "game", "rejected", 0),
        }
        ForwardDataError::Route(error)
    })?;
    metrics.record_data(transport, "inbound", "game", "accepted", frame_bytes);
    let result = match destination {
        DataDestination::Tcp(peer_id) => send_frame(egress, metrics, peer_id, "game", raw).await,
        DataDestination::Udp(address) => {
            if let Some(socket) = udp {
                socket.send_to(&raw, address).await.map(|_| ())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "UDP listener unavailable",
                ))
            }
        }
    };
    if result.is_ok() {
        metrics.record_data(
            destination.transport_name(),
            "outbound",
            "game",
            "forwarded",
            frame_bytes,
        );
    } else {
        metrics.record_data(
            destination.transport_name(),
            "outbound",
            "game",
            "rejected",
            0,
        );
    }
    metrics.record_dispatch_duration(transport, "game", started.elapsed().as_secs_f64());
    result.map_err(ForwardDataError::Dispatch)
}

pub(super) async fn forward_probe(
    probe: ProbeFrame,
    raw: Bytes,
    source: DataSource,
    state: &SharedState,
    egress: &SharedTcpEgress,
    udp: Option<&Arc<UdpSocket>>,
    metrics: &RelayMetrics,
) -> io::Result<()> {
    let frame_bytes = raw.len();
    let transport = source.transport_name();
    let started = Instant::now();
    let destination = {
        let mut state = state.lock().await;
        state.route_probe(RouteProbe {
            connection_id: probe.connection_id,
            from_steam_id64: probe.from_steam_id64,
            to_steam_id64: probe.to_steam_id64,
            source,
            frame_bytes,
            now: Instant::now(),
        })
    };
    let destination = destination.map_err(|error| {
        let outcome = match error {
            crate::domain::StateError::RateLimited
            | crate::domain::StateError::ProbeRateLimited => "rate_limited",
            _ => "rejected",
        };
        metrics.record_data(transport, "inbound", "probe", outcome, 0);
        invalid_data(error)
    })?;
    metrics.record_data(transport, "inbound", "probe", "accepted", frame_bytes);
    let result = match destination {
        DataDestination::Tcp(peer_id) => send_frame(egress, metrics, peer_id, "probe", raw).await,
        DataDestination::Udp(address) => {
            let socket = udp.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "UDP listener unavailable")
            })?;
            socket.send_to(&raw, address).await?;
            Ok(())
        }
    };
    if result.is_ok() {
        metrics.record_data(
            destination.transport_name(),
            "outbound",
            "probe",
            "forwarded",
            frame_bytes,
        );
    } else {
        metrics.record_data(
            destination.transport_name(),
            "outbound",
            "probe",
            "rejected",
            0,
        );
    }
    metrics.record_dispatch_duration(transport, "probe", started.elapsed().as_secs_f64());
    result
}

pub(super) async fn send_control(
    egress: &SharedTcpEgress,
    metrics: &RelayMetrics,
    peer_id: PeerId,
    message: &ServerControl,
) -> io::Result<()> {
    let payload = encode_server_control(message).map_err(invalid_data)?;
    let frame = Frame::ServerControl(payload)
        .encode()
        .map_err(invalid_data)?;
    send_frame(egress, metrics, peer_id, "control", frame).await
}

pub(super) async fn send_presence(
    egress: &SharedTcpEgress,
    metrics: &RelayMetrics,
    broadcast: PresenceBroadcast,
) {
    let message = ServerControl::PeerPresenceUpdate {
        peers: protocol::peer_views(broadcast.peers),
    };
    for recipient in broadcast.recipients {
        let _ = send_control(egress, metrics, recipient, &message).await;
    }
}

pub(super) async fn send_frame(
    egress: &SharedTcpEgress,
    metrics: &RelayMetrics,
    peer_id: PeerId,
    frame_type: &'static str,
    frame: Bytes,
) -> io::Result<()> {
    let sender = egress.lock().await.get(&peer_id).cloned().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotConnected,
            format!("missing TCP egress for {peer_id}"),
        )
    })?;
    sender.try_send(frame).map_err(|_| {
        metrics.record_tcp_egress_queue_full(frame_type);
        io::Error::new(io::ErrorKind::WouldBlock, "TCP egress queue is full")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_forwarding_classifies_sender_violations_as_fatal() {
        for error in [
            StateError::UnknownConnection,
            StateError::SenderMismatch,
            StateError::ProfileMismatch,
            StateError::PathNotValidated,
        ] {
            assert!(ForwardDataError::Route(error).is_fatal_for_tcp_sender());
        }
    }

    #[test]
    fn data_forwarding_keeps_delivery_failures_non_fatal() {
        for error in [
            StateError::DuplicateFrame,
            StateError::FrameTooOld,
            StateError::RateLimited,
            StateError::TargetNotJoined,
            StateError::TargetUnavailable,
        ] {
            assert!(!ForwardDataError::Route(error).is_fatal_for_tcp_sender());
        }
        assert!(
            !ForwardDataError::Dispatch(io::Error::new(
                io::ErrorKind::WouldBlock,
                "TCP egress queue is full",
            ))
            .is_fatal_for_tcp_sender()
        );
    }
}
