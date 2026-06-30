use std::{
    fmt::{self, Display},
    io,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use super::{
    PROBE_A_STEAM, PROBE_B_STEAM, ProbeHandle, ProbePeer, block_on_probe, probe_payload,
    validate_probe_payload,
};
use crate::client::{
    ConnectionProfile, LogLevel, RelayEndpoint, TransportChoice,
    state::{RuntimeEvent, log_event, unix_seconds},
};
use bytes::Bytes;
use serde::Serialize;

pub const READINESS_PROBE_SAMPLES_PER_CASE: u64 = 50;
pub const READINESS_PROBE_PAYLOAD_BYTES: [usize; 3] = [512, 1024, 2048];
pub const READINESS_PROBE_CONNECTION_PROFILES: [ConnectionProfile; 2] = ConnectionProfile::ALL;
const READINESS_SAMPLE_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReadinessProbeCaseReport {
    pub connection_profile: ConnectionProfile,
    pub transport: TransportChoice,
    pub payload_bytes: usize,
    pub duration_ms: u128,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub missing_packets: u64,
    pub min_latency_ms: Option<u128>,
    pub median_latency_ms: Option<u128>,
    pub p95_latency_ms: Option<u128>,
    pub max_latency_ms: Option<u128>,
    pub jitter_ms: Option<u128>,
    pub failure_reason: Option<String>,
}

impl ReadinessProbeCaseReport {
    #[must_use]
    pub fn has_issue(&self) -> bool {
        self.failure_reason.is_some() || self.missing_packets > 0
    }

    #[must_use]
    pub fn detailed_log(&self) -> String {
        let mut message = format!(
            "connection_profile={}; transport={}; payload_bytes={}; duration_ms={}; sent={}; received={}; missing={}; min_ms={}; median_ms={}; p95_ms={}; max_ms={}; jitter_ms={}",
            self.connection_profile,
            self.transport,
            self.payload_bytes,
            self.duration_ms,
            self.packets_sent,
            self.packets_received,
            self.missing_packets,
            display_latency(self.min_latency_ms),
            display_latency(self.median_latency_ms),
            display_latency(self.p95_latency_ms),
            display_latency(self.max_latency_ms),
            display_latency(self.jitter_ms)
        );
        if let Some(reason) = &self.failure_reason {
            message.push_str("; error=");
            message.push_str(reason);
        }
        message
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReadinessProbeReport {
    pub relay: String,
    pub room: String,
    pub duration_ms: u128,
    pub samples_per_case: u64,
    pub cases: Vec<ReadinessProbeCaseReport>,
}

impl ReadinessProbeReport {
    #[must_use]
    pub fn has_issue(&self) -> bool {
        self.cases.iter().any(ReadinessProbeCaseReport::has_issue)
    }

    #[must_use]
    pub fn detailed_log(&self) -> String {
        let cases = self
            .cases
            .iter()
            .map(ReadinessProbeCaseReport::detailed_log)
            .collect::<Vec<_>>()
            .join(" | ");
        format!(
            "Readiness matrix via {}; room={}; duration_ms={}; samples_per_case={}; cases=[{}]",
            self.relay, self.room, self.duration_ms, self.samples_per_case, cases
        )
    }
}

impl Display for ReadinessProbeReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detailed_log())
    }
}

pub(super) fn spawn_readiness_probe(relay: RelayEndpoint) -> io::Result<ProbeHandle> {
    relay.validate().map_err(io::Error::other)?;
    for payload_bytes in READINESS_PROBE_PAYLOAD_BYTES {
        validate_probe_payload(payload_bytes)?;
    }
    let (event_tx, events) = mpsc::channel();
    let worker = thread::spawn(move || {
        let report = run_readiness_probe(relay);
        let level = if report.has_issue() {
            LogLevel::Warn
        } else {
            LogLevel::Info
        };
        let _ = event_tx.send(log_event(level, report.detailed_log()));
        let _ = event_tx.send(RuntimeEvent::ReadinessProbeFinished(Ok(Box::new(report))));
    });
    Ok(ProbeHandle {
        events,
        worker: Some(worker),
    })
}

fn run_readiness_probe(relay: RelayEndpoint) -> ReadinessProbeReport {
    let relay_display = relay.to_string();
    let room = format!("bb-probe-{}-{}", std::process::id(), unix_seconds());
    let started = Instant::now();
    let cases = block_on_probe(async { Ok(run_readiness_probe_matrix_async(relay, &room).await) })
        .unwrap_or_else(|error| {
            vec![failed_readiness_case_report(
                ConnectionProfile::Tcp,
                READINESS_PROBE_PAYLOAD_BYTES[0],
                started.elapsed(),
                error.to_string(),
            )]
        });
    ReadinessProbeReport {
        relay: relay_display,
        room,
        duration_ms: started.elapsed().as_millis(),
        samples_per_case: READINESS_PROBE_SAMPLES_PER_CASE,
        cases,
    }
}

async fn run_readiness_probe_matrix_async(
    relay: RelayEndpoint,
    room: &str,
) -> Vec<ReadinessProbeCaseReport> {
    let mut cases = Vec::new();
    for connection_profile in READINESS_PROBE_CONNECTION_PROFILES {
        for payload_bytes in READINESS_PROBE_PAYLOAD_BYTES {
            let started = Instant::now();
            let case = match run_readiness_probe_case_async(
                &relay,
                connection_profile,
                room,
                payload_bytes,
            )
            .await
            {
                Ok(mut report) => {
                    report.duration_ms = started.elapsed().as_millis();
                    report
                }
                Err(error) => failed_readiness_case_report(
                    connection_profile,
                    payload_bytes,
                    started.elapsed(),
                    error.to_string(),
                ),
            };
            cases.push(case);
        }
    }
    cases
}

async fn run_readiness_probe_case_async(
    relay: &RelayEndpoint,
    connection_profile: ConnectionProfile,
    room: &str,
    payload_bytes: usize,
) -> io::Result<ReadinessProbeCaseReport> {
    let transport = connection_profile.transport();
    let mut peer_a = ProbePeer::join(relay, transport, room, PROBE_A_STEAM, "Probe A").await?;
    let mut peer_b = ProbePeer::join(relay, transport, room, PROBE_B_STEAM, "Probe B").await?;
    let payload = probe_payload(payload_bytes);
    let started = Instant::now();
    let mut sequence = 1_u32;
    let mut sent = 0_u64;
    let mut received = 0_u64;
    let mut latencies = Vec::new();

    for index in 0..READINESS_PROBE_SAMPLES_PER_CASE {
        if index % 2 == 0 {
            sample_direction(
                &mut peer_a,
                &mut peer_b,
                PROBE_B_STEAM,
                PROBE_A_STEAM,
                PROBE_B_STEAM,
                payload.clone(),
                &mut sequence,
                &mut sent,
                &mut received,
                &mut latencies,
            )
            .await;
        } else {
            sample_direction(
                &mut peer_b,
                &mut peer_a,
                PROBE_A_STEAM,
                PROBE_B_STEAM,
                PROBE_A_STEAM,
                payload.clone(),
                &mut sequence,
                &mut sent,
                &mut received,
                &mut latencies,
            )
            .await;
        }
    }

    Ok(readiness_case_report(ReadinessStats {
        connection_profile,
        transport,
        payload_bytes,
        duration: started.elapsed(),
        packets_sent: sent,
        packets_received: received,
        latencies,
        failure_reason: None,
    }))
}

#[expect(
    clippy::too_many_arguments,
    reason = "keeps the two probe directions symmetric"
)]
async fn sample_direction(
    sender: &mut ProbePeer,
    receiver: &mut ProbePeer,
    target_steam_id64: &str,
    expected_from: &str,
    expected_to: &str,
    payload: Bytes,
    sequence: &mut u32,
    sent: &mut u64,
    received: &mut u64,
    latencies: &mut Vec<u128>,
) {
    let started = Instant::now();
    *sent = sent.saturating_add(1);
    let current_sequence = *sequence;
    *sequence = sequence.saturating_add(1);
    if sender
        .send_game_with_sequence(target_steam_id64, payload.clone(), current_sequence)
        .await
        .is_err()
    {
        return;
    }
    if receiver
        .expect_game_with_timeout(
            expected_from,
            expected_to,
            current_sequence,
            &payload,
            READINESS_SAMPLE_TIMEOUT,
        )
        .await
        .is_ok()
    {
        *received = received.saturating_add(1);
        latencies.push(started.elapsed().as_millis());
    }
}

#[derive(Debug)]
struct ReadinessStats {
    connection_profile: ConnectionProfile,
    transport: TransportChoice,
    payload_bytes: usize,
    duration: Duration,
    packets_sent: u64,
    packets_received: u64,
    latencies: Vec<u128>,
    failure_reason: Option<String>,
}

fn readiness_case_report(stats: ReadinessStats) -> ReadinessProbeCaseReport {
    let mut latencies = stats.latencies;
    latencies.sort_unstable();
    let missing_packets = stats.packets_sent.saturating_sub(stats.packets_received);
    let min_latency_ms = latencies.first().copied();
    let median_latency_ms = percentile(&latencies, 50);
    let p95_latency_ms = percentile(&latencies, 95);
    let max_latency_ms = latencies.last().copied();
    let jitter_ms = median_latency_ms
        .zip(p95_latency_ms)
        .map(|(median, p95)| p95.saturating_sub(median));
    ReadinessProbeCaseReport {
        connection_profile: stats.connection_profile,
        transport: stats.transport,
        payload_bytes: stats.payload_bytes,
        duration_ms: stats.duration.as_millis(),
        packets_sent: stats.packets_sent,
        packets_received: stats.packets_received,
        missing_packets,
        min_latency_ms,
        median_latency_ms,
        p95_latency_ms,
        max_latency_ms,
        jitter_ms,
        failure_reason: stats.failure_reason,
    }
}

fn failed_readiness_case_report(
    connection_profile: ConnectionProfile,
    payload_bytes: usize,
    duration: Duration,
    failure_reason: String,
) -> ReadinessProbeCaseReport {
    readiness_case_report(ReadinessStats {
        connection_profile,
        transport: connection_profile.transport(),
        payload_bytes,
        duration,
        packets_sent: 0,
        packets_received: 0,
        latencies: Vec::new(),
        failure_reason: Some(failure_reason),
    })
}

fn percentile(values: &[u128], percentile: usize) -> Option<u128> {
    if values.is_empty() {
        return None;
    }
    let index = ((values.len() - 1) * percentile).div_ceil(100);
    values.get(index).copied()
}

fn display_latency(value: Option<u128>) -> String {
    value.map_or_else(
        || "-".to_owned(),
        |value| {
            if value == 0 {
                "<1".to_owned()
            } else {
                value.to_string()
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_matrix_declares_supported_connection_profiles() {
        assert_eq!(
            READINESS_PROBE_CONNECTION_PROFILES,
            [ConnectionProfile::Tcp, ConnectionProfile::Udp]
        );

        let report = failed_readiness_case_report(
            ConnectionProfile::Udp,
            READINESS_PROBE_PAYLOAD_BYTES[0],
            Duration::from_millis(1),
            "probe error".to_owned(),
        );

        assert_eq!(report.connection_profile, ConnectionProfile::Udp);
        assert_eq!(report.transport, TransportChoice::Udp);
        assert!(report.detailed_log().contains("connection_profile=UDP"));
    }
}
