use std::{io, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::{
    SinkExt as _, StreamExt as _,
    stream::{SplitSink, SplitStream},
};
use sha2::{Digest as _, Sha256};
use tokio::{
    net::{TcpStream, UdpSocket},
    time,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::protocol::{
    CAP_ROOM_PATH_PROBE, ClientControl, DataFrame, DataProfile, Frame, PeerPresenceInfo,
    ProbeFrame, ProbePhase, SecretString, ServerControl, decode_frame, decode_server_control,
    encode_client_control,
};

use super::{ExternalRelayConfig, RelayEndpoint, TransportChoice, packet_flow::OutboundGamePacket};

mod bootstrap;
mod pow;
use bootstrap::negotiate;
use pow::solve_pow;

pub(super) const MAX_RELAY_DATAGRAM_SIZE: usize = 65_535;
pub(super) const RELAY_JOIN_TIMEOUT: Duration = Duration::from_secs(8);

type TcpFramed = Framed<TcpStream, LengthDelimitedCodec>;

pub(super) struct RelayTransport {
    pub(super) sender: RelayTransportSender,
    pub(super) receiver: RelayTransportReceiver,
    session: Option<RelaySessionConfig>,
    enabled_capabilities: u64,
}

#[derive(Clone)]
struct RelaySessionConfig {
    route: ExternalRelayConfig,
    display_name: String,
}

pub(super) struct RelayTransportSender {
    tcp: SplitSink<TcpFramed, Bytes>,
    udp: Option<Arc<UdpSocket>>,
    connection_id: Option<u64>,
    from_steam_id64: u64,
    next_frame_id: u64,
    next_probe_id: u64,
    resume_credential: Option<SecretString>,
}

pub(super) struct RelayTransportReceiver {
    tcp: SplitStream<TcpFramed>,
    udp: Option<Arc<UdpSocket>>,
    udp_buffer: Box<[u8; MAX_RELAY_DATAGRAM_SIZE]>,
}

impl RelayTransport {
    pub(super) async fn connect(
        endpoint: &RelayEndpoint,
        choice: TransportChoice,
        client_version: &str,
        git_hash: Option<&str>,
        steam_id64: u64,
    ) -> io::Result<Self> {
        let mut stream = TcpStream::connect(endpoint.to_string()).await?;
        stream.set_nodelay(true)?;
        let enabled_capabilities = negotiate(&mut stream, choice, client_version, git_hash).await?;
        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(MAX_RELAY_DATAGRAM_SIZE)
            .new_codec();
        let (tcp_sender, tcp_receiver) = Framed::new(stream, codec).split();
        let udp = if choice == TransportChoice::Udp {
            let socket = UdpSocket::bind("0.0.0.0:0").await?;
            socket.connect(endpoint.to_string()).await?;
            Some(Arc::new(socket))
        } else {
            None
        };
        Ok(Self {
            sender: RelayTransportSender {
                tcp: tcp_sender,
                udp: udp.clone(),
                connection_id: None,
                from_steam_id64: steam_id64,
                next_frame_id: 1,
                next_probe_id: 1,
                resume_credential: None,
            },
            receiver: RelayTransportReceiver {
                tcp: tcp_receiver,
                udp,
                udp_buffer: Box::new([0; MAX_RELAY_DATAGRAM_SIZE]),
            },
            session: None,
            enabled_capabilities,
        })
    }

    pub(super) async fn connect_session(
        route: &ExternalRelayConfig,
        steam_id64: &str,
        display_name: &str,
    ) -> io::Result<(Self, Vec<PeerPresenceInfo>)> {
        let build = crate::build_info::current();
        let steam_id64 = steam_id64
            .parse::<u64>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "SteamID64 is invalid"))?;
        let mut relay = Self::connect(
            &route.relay,
            route.transport,
            build.version,
            build.git_hash,
            steam_id64,
        )
        .await?;
        let peers =
            complete_relay_join(&mut relay.sender, &mut relay.receiver, route, display_name)
                .await?;
        relay.session = Some(RelaySessionConfig {
            route: route.clone(),
            display_name: display_name.to_owned(),
        });
        Ok((relay, peers))
    }

    pub(super) async fn reconnect(&mut self) -> io::Result<RecoveryKind> {
        let session = self.session.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Relay session is not reconnectable",
            )
        })?;
        let connection_id = self.sender.connection_id.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "Relay connection id is unavailable",
            )
        })?;
        let resume_credential = self.sender.resume_credential.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "Relay resume credential is unavailable",
            )
        })?;
        let next_frame_id = self.sender.next_frame_id;
        let next_probe_id = self.sender.next_probe_id;
        let build = crate::build_info::current();
        let mut replacement = Self::connect(
            &session.route.relay,
            session.route.transport,
            build.version,
            build.git_hash,
            self.sender.from_steam_id64,
        )
        .await?;
        send_control(
            &mut replacement.sender,
            &ClientControl::Resume {
                connection_id,
                resume_credential: resume_credential.clone(),
            },
        )
        .await?;
        let recovery = loop {
            match recv_server_control(&mut replacement.receiver).await? {
                ServerControl::ResumeReady {
                    connection_id: returned,
                    peers,
                    ..
                } if returned == connection_id => {
                    replacement.sender.connection_id = Some(connection_id);
                    replacement.sender.resume_credential = Some(resume_credential);
                    replacement.sender.next_frame_id = next_frame_id;
                    replacement.sender.next_probe_id = next_probe_id;
                    if replacement.sender.udp.is_some() {
                        validate_udp_path(
                            &mut replacement.sender,
                            &mut replacement.receiver,
                            connection_id,
                        )
                        .await?;
                    }
                    break RecoveryKind::Resumed { peers };
                }
                ServerControl::ResumeRejected {
                    allow_full_join: true,
                    ..
                } => {
                    let peers = complete_relay_join(
                        &mut replacement.sender,
                        &mut replacement.receiver,
                        &session.route,
                        &session.display_name,
                    )
                    .await?;
                    break RecoveryKind::FullJoin { peers };
                }
                ServerControl::ResumeRejected { code, .. } => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("Relay resume rejected: {code:?}"),
                    ));
                }
                ServerControl::Error {
                    code,
                    message,
                    retryable,
                } => {
                    return Err(io::Error::other(format!(
                        "Relay recovery error {code:?} retryable={retryable}: {message}"
                    )));
                }
                _ => {}
            }
        };
        replacement.session = Some(session);
        *self = replacement;
        Ok(recovery)
    }
}

impl RelayTransport {
    pub(super) fn supports_room_path_probe(&self) -> bool {
        self.enabled_capabilities & CAP_ROOM_PATH_PROBE != 0
    }

    pub(super) fn local_steam_id64(&self) -> u64 {
        self.sender.from_steam_id64
    }
}

#[derive(Clone, Debug)]
pub(super) enum RecoveryKind {
    Resumed { peers: Vec<PeerPresenceInfo> },
    FullJoin { peers: Vec<PeerPresenceInfo> },
}

impl RelayTransportSender {
    pub(super) async fn send_data_datagram(
        &mut self,
        packet: OutboundGamePacket,
    ) -> io::Result<()> {
        let connection_id = self.connection_id.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "Relay join is not complete")
        })?;
        let frame = DataFrame {
            connection_id,
            frame_id: self.next_frame_id,
            from_steam_id64: self.from_steam_id64,
            to_steam_id64: packet.to_steam_id64,
            source_sequence: packet.source_sequence,
            channel: packet.channel,
            send_type: packet.send_type,
            payload: packet.payload,
        }
        .encode()
        .map_err(io::Error::other)?;
        self.next_frame_id = self.next_frame_id.saturating_add(1);
        if let Some(udp) = &self.udp {
            udp.send(&frame).await?;
        } else {
            self.tcp.send(frame).await.map_err(io::Error::other)?;
        }
        Ok(())
    }

    pub(super) async fn send_probe(
        &mut self,
        to_steam_id64: u64,
        probe_id: u64,
        phase: ProbePhase,
    ) -> io::Result<()> {
        let connection_id = self.connection_id.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "Relay join is not complete")
        })?;
        let frame = ProbeFrame {
            connection_id,
            probe_id,
            from_steam_id64: self.from_steam_id64,
            to_steam_id64,
            phase,
        }
        .encode()
        .map_err(io::Error::other)?;
        if let Some(udp) = &self.udp {
            udp.send(&frame).await?;
        } else {
            self.tcp.send(frame).await.map_err(io::Error::other)?;
        }
        Ok(())
    }

    pub(super) fn next_probe_id(&mut self) -> u64 {
        let id = self.next_probe_id;
        self.next_probe_id = self.next_probe_id.checked_add(1).unwrap_or(1);
        id
    }
}

impl RelayTransportReceiver {
    pub(super) async fn recv_datagram(&mut self) -> io::Result<Bytes> {
        if let Some(udp) = &self.udp {
            tokio::select! {
                frame = self.tcp.next() => tcp_frame(frame),
                result = udp.recv(self.udp_buffer.as_mut_slice()) => {
                    let size = result?;
                    Ok(Bytes::copy_from_slice(&self.udp_buffer[..size]))
                }
            }
        } else {
            tcp_frame(self.tcp.next().await)
        }
    }
}

pub(super) async fn complete_relay_join(
    sender: &mut RelayTransportSender,
    receiver: &mut RelayTransportReceiver,
    route: &ExternalRelayConfig,
    display_name: &str,
) -> io::Result<Vec<PeerPresenceInfo>> {
    time::timeout(
        RELAY_JOIN_TIMEOUT,
        complete_relay_join_inner(sender, receiver, route, display_name),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Relay join timed out"))?
}

pub(super) async fn send_control(
    sender: &mut RelayTransportSender,
    message: &ClientControl,
) -> io::Result<()> {
    let payload = encode_client_control(message).map_err(io::Error::other)?;
    let frame = Frame::ClientControl(payload)
        .encode()
        .map_err(io::Error::other)?;
    sender.tcp.send(frame).await.map_err(io::Error::other)
}

async fn complete_relay_join_inner(
    sender: &mut RelayTransportSender,
    receiver: &mut RelayTransportReceiver,
    route: &ExternalRelayConfig,
    display_name: &str,
) -> io::Result<Vec<PeerPresenceInfo>> {
    let profile = match route.transport {
        TransportChoice::Tcp => DataProfile::Tcp,
        TransportChoice::Udp => DataProfile::Udp,
    };
    send_control(
        sender,
        &ClientControl::JoinBegin {
            session_credential: SecretString::new(route.session_credential.wire_secret()),
            steam_id64: sender.from_steam_id64,
            display_name: Some(display_name.to_owned()),
            data_profile: profile,
        },
    )
    .await?;

    let (challenge_id, proof) = loop {
        match recv_server_control(receiver).await? {
            ServerControl::AdmissionChallenge {
                challenge_id,
                algorithm,
                nonce,
                difficulty_bits,
            } if algorithm == "sha256" => {
                let proof = solve_pow(
                    &challenge_id,
                    route.session_credential.as_bytes(),
                    sender.from_steam_id64,
                    &nonce,
                    difficulty_bits,
                )?;
                break (challenge_id, proof);
            }
            ServerControl::Error { code, message, .. } => {
                return Err(io::Error::other(format!("{code:?}: {message}")));
            }
            _ => {}
        }
    };
    send_control(
        sender,
        &ClientControl::JoinProof {
            challenge_id,
            proof: SecretString::new(proof),
        },
    )
    .await?;

    let (connection_id, peers) = loop {
        match recv_server_control(receiver).await? {
            ServerControl::JoinReady {
                connection_id,
                resume_credential,
                peers,
            } => {
                sender.resume_credential = Some(resume_credential);
                break (connection_id, peers);
            }
            ServerControl::Error { code, message, .. } => {
                return Err(io::Error::other(format!("{code:?}: {message}")));
            }
            _ => {}
        }
    };
    sender.connection_id = Some(connection_id);

    if sender.udp.is_some() {
        validate_udp_path(sender, receiver, connection_id).await?;
    }
    Ok(peers)
}

async fn validate_udp_path(
    sender: &mut RelayTransportSender,
    receiver: &mut RelayTransportReceiver,
    connection_id: u64,
) -> io::Result<()> {
    let udp = sender.udp.clone().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "UDP data profile is not active",
        )
    })?;
    send_control(sender, &ClientControl::UdpPathRequest).await?;
    let path_token = loop {
        match recv_server_control(receiver).await? {
            ServerControl::UdpPathToken {
                connection_id: returned,
                path_token,
            } if returned == connection_id => break path_token,
            ServerControl::Error { code, message, .. } => {
                return Err(io::Error::other(format!("{code:?}: {message}")));
            }
            _ => {}
        }
    };
    let payload = encode_client_control(&ClientControl::UdpPathHello {
        connection_id,
        path_token,
    })
    .map_err(io::Error::other)?;
    let frame = Frame::ClientControl(payload)
        .encode()
        .map_err(io::Error::other)?;
    udp.send(&frame).await?;
    loop {
        match recv_server_control(receiver).await? {
            ServerControl::UdpPathReady {
                connection_id: returned,
            } if returned == connection_id => break,
            ServerControl::Error { code, message, .. } => {
                return Err(io::Error::other(format!("{code:?}: {message}")));
            }
            _ => {}
        }
    }
    Ok(())
}

async fn recv_server_control(receiver: &mut RelayTransportReceiver) -> io::Result<ServerControl> {
    loop {
        let raw = receiver.recv_datagram().await?;
        if let Frame::ServerControl(payload) = decode_frame(raw).map_err(io::Error::other)? {
            return decode_server_control(&payload).map_err(io::Error::other);
        }
    }
}

fn tcp_frame(frame: Option<Result<bytes::BytesMut, io::Error>>) -> io::Result<Bytes> {
    let Some(frame) = frame else {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Relay TCP control connection closed",
        ));
    };
    frame.map(|bytes| bytes.freeze())
}
