use std::{collections::HashMap, io, sync::Arc};

use bytes::Bytes;
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::{Mutex, mpsc},
};
use tracing::{debug, info};

use crate::{
    config::RelayConfig, domain::PeerId, metrics::RelayMetrics, peer_registry::PeerRegistry,
    state::RelayState,
};

mod control;
mod data;
mod establishment;
mod maintenance;

use control::tcp_task;
use data::udp_task;
use maintenance::{cleanup_task, metrics_task};

type SharedState = Arc<Mutex<RelayState>>;
type SharedTcpEgress = Arc<Mutex<HashMap<PeerId, mpsc::Sender<Bytes>>>>;
type SharedMetrics = Arc<RelayMetrics>;
type SharedEstablishments = establishment::EstablishmentRegistry;

struct TcpTaskContext {
    state: SharedState,
    egress: SharedTcpEgress,
    udp: Option<Arc<UdpSocket>>,
    max_frame_size: usize,
    queue_capacity: usize,
    metrics: SharedMetrics,
    establishments: SharedEstablishments,
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
        "relay listening"
    );
    run_with_listeners(listener, udp, config, metrics).await
}

async fn run_with_listeners(
    listener: TcpListener,
    udp: Option<Arc<UdpSocket>>,
    config: RelayConfig,
    metrics: SharedMetrics,
) -> io::Result<()> {
    let state = Arc::new(Mutex::new(RelayState::new(config.clone())));
    let egress = Arc::new(Mutex::new(HashMap::new()));
    let registry = Arc::new(Mutex::new(PeerRegistry::default()));
    let establishments = establishment::EstablishmentRegistry::new();
    if let Some(socket) = udp.clone() {
        tokio::spawn(udp_task(
            socket,
            Arc::clone(&state),
            Arc::clone(&egress),
            Arc::clone(&metrics),
            establishments.clone(),
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
            metrics.record_blocked_connection();
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
                establishments: establishments.clone(),
            },
        ));
    }
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
