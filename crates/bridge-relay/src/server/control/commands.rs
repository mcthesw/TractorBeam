use std::{io, time::Instant};

use tracing::info;
use tractor_beam_relay_protocol::{ClientControl, ServerControl};

use super::{ControlResult, EstablishmentEvent, control_operation_name};
use crate::{
    domain::{DataProfile, JoinBegin, PeerId},
    metrics::RelayMetrics,
    protocol,
    server::{
        SharedState, SharedTcpEgress,
        data::{send_control, send_presence},
        establishment::profile_name,
        invalid_data,
    },
};

pub(super) async fn handle_control(
    peer_id: PeerId,
    enabled_capabilities: u64,
    message: ClientControl,
    state: &SharedState,
    egress: &SharedTcpEgress,
    metrics: &RelayMetrics,
) -> io::Result<ControlResult> {
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
                session: protocol::session_key(&session_credential).map_err(invalid_data)?,
                steam_id64,
                display_name,
                profile: protocol::profile(data_profile),
                capabilities: enabled_capabilities,
                now,
            };
            let (response, event) = match state.lock().await.begin_join(begin) {
                Ok(challenge) => {
                    metrics.record_control("join_begin", "accepted");
                    (
                        ServerControl::AdmissionChallenge {
                            challenge_id: protocol::hex(challenge.challenge_id),
                            algorithm: "sha256".to_owned(),
                            nonce: protocol::hex(challenge.pow_nonce),
                            difficulty_bits: challenge.difficulty_bits,
                        },
                        EstablishmentEvent::Progress,
                    )
                }
                Err(error) => {
                    metrics.record_control("join_begin", "rejected");
                    (
                        protocol::state_error(error),
                        EstablishmentEvent::Rejected("join_begin_rejected"),
                    )
                }
            };
            send_control(egress, metrics, peer_id, &response).await?;
            return Ok(ControlResult::continue_with(event));
        }
        ClientControl::JoinProof {
            challenge_id,
            proof,
        } => {
            let response = state.lock().await.complete_join(
                peer_id,
                protocol::decode_hex_16(&challenge_id).map_err(invalid_data)?,
                proof.expose_secret(),
                now,
            );
            match response {
                Ok((ready, broadcast)) => {
                    metrics.record_control("join_proof", "accepted");
                    send_control(
                        egress,
                        metrics,
                        peer_id,
                        &ServerControl::JoinReady {
                            connection_id: ready.connection_id,
                            resume_credential: protocol::secret(ready.resume_key.0),
                            peers: protocol::peer_views(ready.peers),
                        },
                    )
                    .await?;
                    if let Some(broadcast) = broadcast {
                        send_presence(egress, metrics, broadcast).await;
                    }
                    return Ok(ControlResult::continue_with(
                        EstablishmentEvent::Established {
                            operation: "join",
                            profile: profile_name(ready.profile),
                            connection_id: ready.connection_id,
                            wait_for_udp: matches!(ready.profile, DataProfile::Udp),
                        },
                    ));
                }
                Err(error) => {
                    metrics.record_control("join_proof", "rejected");
                    send_control(egress, metrics, peer_id, &protocol::state_error(error)).await?;
                    return Ok(ControlResult::continue_with(EstablishmentEvent::Rejected(
                        "join_proof_rejected",
                    )));
                }
            }
        }
        ClientControl::Resume {
            connection_id,
            resume_credential,
        } => {
            let result = match protocol::resume_key(&resume_credential) {
                Ok(key) => state.lock().await.resume(peer_id, connection_id, key, now),
                Err(error) => Err(error),
            };
            match result {
                Ok(ready) => {
                    metrics.record_control("resume", "accepted");
                    info!(udp_path_valid = ready.udp_path_valid, "session resumed");
                    send_control(
                        egress,
                        metrics,
                        peer_id,
                        &ServerControl::ResumeReady {
                            connection_id: ready.connection_id,
                            peers: protocol::peer_views(ready.peers),
                            udp_path_valid: ready.udp_path_valid,
                        },
                    )
                    .await?;
                    if let Some(broadcast) = ready.broadcast {
                        send_presence(egress, metrics, broadcast).await;
                    }
                    return Ok(ControlResult::continue_with(
                        EstablishmentEvent::Established {
                            operation: "resume",
                            profile: profile_name(ready.profile),
                            connection_id: ready.connection_id,
                            wait_for_udp: matches!(ready.profile, DataProfile::Udp)
                                && !ready.udp_path_valid,
                        },
                    ));
                }
                Err(error) => {
                    metrics.record_control("resume", "rejected");
                    info!(reason = ?error, "session resume rejected");
                    send_control(egress, metrics, peer_id, &protocol::resume_rejection(error))
                        .await?;
                    return Ok(ControlResult::continue_with(EstablishmentEvent::Rejected(
                        "resume_rejected",
                    )));
                }
            }
        }
        ClientControl::UdpPathRequest => {
            let state = state.lock().await;
            let Some(connection_id) = state.connection_for_control(peer_id) else {
                metrics.record_control("udp_path_request", "rejected");
                send_control(
                    egress,
                    metrics,
                    peer_id,
                    &protocol::state_error(crate::domain::StateError::UnknownConnection),
                )
                .await?;
                return Ok(ControlResult::CONTINUE);
            };
            let Some(key) = state.path_key(connection_id) else {
                metrics.record_control("udp_path_request", "rejected");
                send_control(
                    egress,
                    metrics,
                    peer_id,
                    &protocol::state_error(crate::domain::StateError::ProfileMismatch),
                )
                .await?;
                return Ok(ControlResult::CONTINUE);
            };
            drop(state);
            send_control(
                egress,
                metrics,
                peer_id,
                &ServerControl::UdpPathToken {
                    connection_id,
                    path_token: protocol::secret(key.0),
                },
            )
            .await?;
            metrics.record_control("udp_path_request", "accepted");
        }
        ClientControl::ControlPing { id } => {
            send_control(egress, metrics, peer_id, &ServerControl::ControlPong { id }).await?;
            metrics.record_control("ping", "accepted");
        }
        ClientControl::ControlPong { .. } => metrics.record_control("pong", "accepted"),
        ClientControl::Stop => {
            metrics.record_control("stop", "accepted");
            return Ok(ControlResult::STOP);
        }
        ClientControl::UdpPathHello { .. } => {
            metrics.record_control("udp_path_hello", "rejected");
            send_control(
                egress,
                metrics,
                peer_id,
                &protocol::state_error(crate::domain::StateError::ProfileMismatch),
            )
            .await?;
        }
    }
    Ok(ControlResult::CONTINUE)
}
