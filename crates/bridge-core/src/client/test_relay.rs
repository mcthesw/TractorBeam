use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, thread};

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    runtime::Builder,
    sync::{Mutex, mpsc, oneshot},
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::protocol::v2::{
    BOOTSTRAP_SCHEMA, BootstrapMessage, BuildMetadata, ClientControl, DataFrame, Frame,
    PeerPresence, PeerPresenceInfo, ProtocolVersion, SecretString, ServerControl, decode_bootstrap,
    decode_client_control, decode_frame, encode_bootstrap, encode_server_control,
};

type Peers = Arc<Mutex<HashMap<u64, mpsc::Sender<Bytes>>>>;

pub(super) struct TestRelay {
    pub(super) address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl TestRelay {
    pub(super) fn spawn() -> Self {
        Self::spawn_inner(false)
    }

    pub(super) fn spawn_silent() -> Self {
        Self::spawn_inner(true)
    }

    fn spawn_inner(silent: bool) -> Self {
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = oneshot::channel();
        let worker = thread::spawn(move || {
            Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    ready_tx.send(listener.local_addr().unwrap()).unwrap();
                    let peers = Peers::default();
                    let mut silent_connections = Vec::new();
                    tokio::pin!(stop_rx);
                    loop {
                        tokio::select! {
                            _ = &mut stop_rx => break,
                            accepted = listener.accept() => {
                                let (stream, _) = accepted.unwrap();
                                if !silent {
                                    tokio::spawn(serve_connection(stream, Arc::clone(&peers)));
                                } else {
                                    silent_connections.push(stream);
                                }
                            }
                        }
                    }
                });
        });
        Self {
            address: ready_rx.recv().unwrap(),
            stop: Some(stop_tx),
            worker: Some(worker),
        }
    }

    pub(super) fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for TestRelay {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn serve_connection(mut stream: TcpStream, peers: Peers) -> io::Result<()> {
    let length = stream.read_u32().await? as usize;
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await?;
    let mut hello = Vec::with_capacity(4 + length);
    hello.extend_from_slice(&(length as u32).to_be_bytes());
    hello.extend_from_slice(&payload);
    let BootstrapMessage::ClientHello { .. } =
        decode_bootstrap(&hello).map_err(io::Error::other)?
    else {
        return Err(io::Error::other("expected client hello"));
    };
    let response = BootstrapMessage::ServerHello {
        bootstrap_schema: BOOTSTRAP_SCHEMA,
        selected_protocol: ProtocolVersion { major: 2, minor: 0 },
        enabled_capabilities: crate::protocol::v2::CAP_TCP_DATA
            | crate::protocol::v2::CAP_RESUME
            | crate::protocol::v2::CAP_ROOM_PATH_PROBE,
        relay: BuildMetadata {
            version: "test".into(),
            git_hash: None,
        },
    };
    stream
        .write_all(&encode_bootstrap(&response).map_err(io::Error::other)?)
        .await?;

    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Bytes>(16);
    let mut steam_id = None;
    loop {
        tokio::select! {
            Some(outbound) = outbound_rx.recv() => framed.send(outbound).await.map_err(io::Error::other)?,
            inbound = framed.next() => {
                let Some(inbound) = inbound else { break; };
                let raw = inbound.map_err(io::Error::other)?.freeze();
                match decode_frame(raw.clone()).map_err(io::Error::other)? {
                    Frame::ClientControl(payload) => match decode_client_control(&payload).map_err(io::Error::other)? {
                        ClientControl::JoinBegin { steam_id64, .. } => {
                            steam_id = Some(steam_id64);
                            send_control(&outbound_tx, ServerControl::AdmissionChallenge {
                                challenge_id: "00000000000000000000000000000000".into(),
                                algorithm: "sha256".into(),
                                nonce: "00000000000000000000000000000000".into(),
                                difficulty_bits: 0,
                            }).await?;
                        }
                        ClientControl::JoinProof { .. } => {
                            let steam = steam_id.ok_or_else(|| io::Error::other("proof before join"))?;
                            peers.lock().await.insert(steam, outbound_tx.clone());
                            send_control(&outbound_tx, ServerControl::JoinReady {
                                connection_id: steam,
                                resume_credential: SecretString::new("test-resume"),
                                peers: vec![PeerPresenceInfo { steam_id64: steam, display_name: None, presence: PeerPresence::Connected, capabilities: crate::protocol::v2::CAP_ROOM_PATH_PROBE }],
                            }).await?;
                        }
                        ClientControl::ControlPing { id } => send_control(&outbound_tx, ServerControl::ControlPong { id }).await?,
                        ClientControl::Stop => break,
                        _ => {}
                    },
                    Frame::Data(data) => forward(data, raw, &peers).await?,
                    Frame::Probe(probe) => {
                        let destination = peers.lock().await.get(&probe.to_steam_id64).cloned();
                        if let Some(destination) = destination {
                            destination.send(raw).await.map_err(io::Error::other)?;
                        }
                    }
                    Frame::ServerControl(_) => return Err(io::Error::other("unexpected server frame")),
                }
            }
        }
    }
    if let Some(steam) = steam_id {
        peers.lock().await.remove(&steam);
    }
    Ok(())
}

async fn send_control(sender: &mpsc::Sender<Bytes>, message: ServerControl) -> io::Result<()> {
    let payload = encode_server_control(&message).map_err(io::Error::other)?;
    let frame = Frame::ServerControl(payload)
        .encode()
        .map_err(io::Error::other)?;
    sender.send(frame).await.map_err(io::Error::other)
}

async fn forward(data: DataFrame, raw: Bytes, peers: &Peers) -> io::Result<()> {
    let destination = peers.lock().await.get(&data.to_steam_id64).cloned();
    if let Some(destination) = destination {
        destination.send(raw).await.map_err(io::Error::other)?;
    }
    Ok(())
}
