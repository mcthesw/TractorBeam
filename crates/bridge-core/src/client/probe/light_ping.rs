use std::{
    io,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::Serialize;
use tokio::{runtime::Builder, task::JoinSet, time};

use crate::client::{
    RelayEndpoint, TransportChoice,
    relay_transport::RelayTransport,
    state::{RuntimeEvent, log_event},
};
use crate::protocol::{ControlMessage, Envelope, MessageType};

const LIGHT_PING_COUNT: u8 = 5;
const LIGHT_PING_TIMEOUT: Duration = Duration::from_secs(2);
const LIGHT_PING_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LightPingTarget {
    pub relay_id: Option<String>,
    pub relay_name: Option<String>,
    pub endpoint: RelayEndpoint,
    pub transport: TransportChoice,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LightPingReport {
    pub target: LightPingTarget,
    pub sent: u8,
    pub received: u8,
    pub median_rtt_ms: Option<u128>,
    pub failure_reason: Option<String>,
}

impl LightPingReport {
    #[must_use]
    pub fn latency_label(&self) -> &'static str {
        if self.failure_reason.is_some() {
            "unreachable"
        } else {
            "ok"
        }
    }
}

#[derive(Debug)]
pub struct LightPingHandle {
    pub events: mpsc::Receiver<RuntimeEvent>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for LightPingHandle {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn spawn_light_ping_probes(
    targets: Vec<LightPingTarget>,
) -> io::Result<LightPingHandle> {
    let (event_tx, event_rx) = mpsc::channel::<RuntimeEvent>();
    let worker = thread::spawn(move || {
        let runtime = match Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = event_tx.send(log_event(
                    crate::client::LogLevel::Error,
                    format!("Light ping runtime failed: {error}"),
                ));
                return;
            }
        };
        runtime.block_on(async move {
            let mut tasks: JoinSet<LightPingReport> = JoinSet::new();
            for target in targets {
                let event_tx = event_tx.clone();
                tasks.spawn(async move {
                    let report = light_ping_relay(target).await;
                    let _ = event_tx.send(RuntimeEvent::LightPingFinished(Box::new(report.clone())));
                    report
                });
            }
            while tasks.join_next().await.is_some() {}
        });
    });
    Ok(LightPingHandle {
        events: event_rx,
        worker: Some(worker),
    })
}

async fn light_ping_relay(target: LightPingTarget) -> LightPingReport {
    let mut relay = match RelayTransport::connect(&target.endpoint, target.transport).await {
        Ok(relay) => relay,
        Err(error) => {
            return LightPingReport {
                target,
                sent: 0,
                received: 0,
                median_rtt_ms: None,
                failure_reason: Some(format!("connect failed: {error}")),
            };
        }
    };

    let mut rtts: Vec<u128> = Vec::with_capacity(LIGHT_PING_COUNT as usize);
    let mut sent: u8 = 0;
    let mut received: u8 = 0;
    let mut failure_reason: Option<String> = None;

    for id in 1..=LIGHT_PING_COUNT {
        let message = ControlMessage::HealthPing { id: u64::from(id) };
        let payload = match message.encode() {
            Ok(payload) => payload,
            Err(error) => {
                failure_reason = Some(format!("encode failed: {error}"));
                break;
            }
        };
        let raw = match Envelope::new(MessageType::Heartbeat, payload).encode() {
            Ok(raw) => raw,
            Err(error) => {
                failure_reason = Some(format!("envelope failed: {error}"));
                break;
            }
        };
        let started = Instant::now();
        if let Err(error) = relay.sender.send_datagram(raw).await {
            failure_reason = Some(format!("send failed: {error}"));
            break;
        }
        sent += 1;

        match time::timeout(LIGHT_PING_TIMEOUT, relay.receiver.recv_datagram()).await {
            Ok(Ok(data)) => {
                if let Ok(envelope) = Envelope::decode(data)
                    && let Ok(ControlMessage::HealthPong { id: pong_id }) =
                        ControlMessage::decode(&envelope.payload)
                    && pong_id == u64::from(id)
                {
                    rtts.push(started.elapsed().as_millis());
                    received += 1;
                }
            }
            Ok(Err(error)) => {
                if failure_reason.is_none() {
                    failure_reason = Some(format!("recv failed: {error}"));
                }
            }
            Err(_) => {
                if failure_reason.is_none() {
                    failure_reason = Some("ping timed out".to_owned());
                }
            }
        }
        time::sleep(LIGHT_PING_INTERVAL).await;
    }

    rtts.sort_unstable();
    let median = if rtts.is_empty() {
        None
    } else {
        Some(rtts[rtts.len() / 2])
    };

    LightPingReport {
        target,
        sent,
        received,
        median_rtt_ms: median,
        failure_reason: if received == 0 {
            failure_reason.or(Some("no pongs received".to_owned()))
        } else {
            None
        },
    }
}