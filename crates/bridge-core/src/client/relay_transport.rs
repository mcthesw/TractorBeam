use std::{io, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt, stream::SplitSink, stream::SplitStream};
use tokio::{
    net::{TcpStream, UdpSocket},
    time,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::protocol::{ControlMessage, Envelope, MessageType};

use super::{RelayEndpoint, SessionConfig, TransportChoice};

pub(super) const MAX_RELAY_DATAGRAM_SIZE: usize = 65_535;
pub(super) const RELAY_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

type TcpFramed = Framed<TcpStream, LengthDelimitedCodec>;

pub(super) struct RelayTransport {
    pub(super) sender: RelayTransportSender,
    pub(super) receiver: RelayTransportReceiver,
}

pub(super) enum RelayTransportSender {
    Udp { socket: Arc<UdpSocket> },
    Tcp(SplitSink<TcpFramed, Bytes>),
}

pub(super) enum RelayTransportReceiver {
    Udp {
        socket: Arc<UdpSocket>,
        buffer: Box<[u8; MAX_RELAY_DATAGRAM_SIZE]>,
    },
    Tcp(SplitStream<TcpFramed>),
}

impl RelayTransport {
    pub(super) async fn connect(
        endpoint: &RelayEndpoint,
        choice: TransportChoice,
    ) -> io::Result<Self> {
        match choice {
            TransportChoice::Udp => connect_udp(endpoint).await,
            TransportChoice::Tcp => connect_tcp(endpoint).await,
        }
    }
}

impl RelayTransportSender {
    pub(super) async fn send_datagram(&mut self, bytes: Bytes) -> io::Result<()> {
        match self {
            Self::Udp { socket, .. } => {
                socket.send(&bytes).await?;
                Ok(())
            }
            Self::Tcp(sink) => sink.send(bytes).await.map_err(io::Error::other),
        }
    }

    pub(super) async fn send_data_datagram(&mut self, bytes: Bytes) -> io::Result<()> {
        match self {
            Self::Udp { socket } => {
                socket.send(&bytes).await?;
                Ok(())
            }
            Self::Tcp(sink) => sink.send(bytes).await.map_err(io::Error::other),
        }
    }
}

impl RelayTransportReceiver {
    pub(super) async fn recv_datagram(&mut self) -> io::Result<Bytes> {
        match self {
            Self::Udp { socket, buffer } => {
                let size = socket.recv(buffer.as_mut_slice()).await?;
                Ok(Bytes::copy_from_slice(&buffer[..size]))
            }
            Self::Tcp(stream) => {
                let Some(frame) = stream.next().await else {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "relay TCP connection closed",
                    ));
                };
                frame.map(|bytes| bytes.freeze()).map_err(io::Error::other)
            }
        }
    }
}

pub(super) async fn complete_relay_join(
    sender: &mut RelayTransportSender,
    receiver: &mut RelayTransportReceiver,
    config: &SessionConfig,
) -> io::Result<usize> {
    time::timeout(
        RELAY_JOIN_TIMEOUT,
        complete_relay_join_inner(sender, receiver, config),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "relay join timed out"))?
}

pub(super) async fn send_control(
    sender: &mut RelayTransportSender,
    message_type: MessageType,
    message: &ControlMessage,
) -> io::Result<()> {
    let payload = message.encode().map_err(io::Error::other)?;
    let raw = Envelope::new(message_type, payload)
        .encode()
        .map_err(io::Error::other)?;
    sender.send_datagram(raw).await
}

async fn complete_relay_join_inner(
    sender: &mut RelayTransportSender,
    receiver: &mut RelayTransportReceiver,
    config: &SessionConfig,
) -> io::Result<usize> {
    send_join(sender, config, None).await?;
    loop {
        let raw = receiver.recv_datagram().await?;
        let envelope = Envelope::decode(raw).map_err(io::Error::other)?;
        let control = ControlMessage::decode(&envelope.payload).map_err(io::Error::other)?;
        match control {
            ControlMessage::Challenge { token } => send_join(sender, config, Some(token)).await?,
            ControlMessage::Ready { peer_count } => {
                return Ok(peer_count);
            }
            ControlMessage::Error { code, message } => {
                return Err(io::Error::other(format!("{code}: {message}")));
            }
            _ => {}
        }
    }
}

async fn send_join(
    sender: &mut RelayTransportSender,
    config: &SessionConfig,
    challenge: Option<String>,
) -> io::Result<()> {
    let message = ControlMessage::Join {
        room: config.room.clone(),
        steam_id64: config.steam_id64.clone(),
        display_name: Some(config.display_name.clone()),
        challenge,
    };
    send_control(sender, MessageType::Join, &message).await
}

async fn connect_udp(endpoint: &RelayEndpoint) -> io::Result<RelayTransport> {
    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
    socket.connect(endpoint.to_string()).await?;
    Ok(RelayTransport {
        sender: RelayTransportSender::Udp {
            socket: Arc::clone(&socket),
        },
        receiver: RelayTransportReceiver::Udp {
            socket,
            buffer: Box::new([0; MAX_RELAY_DATAGRAM_SIZE]),
        },
    })
}

async fn connect_tcp(endpoint: &RelayEndpoint) -> io::Result<RelayTransport> {
    let stream = TcpStream::connect(endpoint.to_string()).await?;
    stream.set_nodelay(true)?;
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_RELAY_DATAGRAM_SIZE)
        .new_codec();
    let framed = Framed::new(stream, codec);
    let (sender, receiver) = framed.split();
    Ok(RelayTransport {
        sender: RelayTransportSender::Tcp(sender),
        receiver: RelayTransportReceiver::Tcp(receiver),
    })
}
