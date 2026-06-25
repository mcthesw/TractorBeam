use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::{
    BridgeClient, HookReceiveProbeReport, LogLevel, PRODUCT_NAME, ReadinessProbeReport,
    RelayEndpoint, SessionHealthSnapshot, SessionMode, TransportChoice, diagnostics_directory,
    runtime_name, state::unix_seconds,
};
use crate::udp_fec::{UdpFecConfig, UdpFecSessionSnapshot};

const SHARE_CODE_PREFIX: &str = "BB1.";
const REPORT_SCHEMA_VERSION: u8 = 2;

#[path = "internal_test_upload.rs"]
mod upload;
pub use upload::InternalTestUploadReceipt;
use upload::post_zip;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InternalTestReportSession {
    pub relay_name: Option<String>,
    pub relay: RelayEndpoint,
    pub transport: TransportChoice,
    pub udp_fec: UdpFecConfig,
    pub room: String,
    pub mode: SessionMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InternalTestShareCode {
    pub session: InternalTestReportSession,
    pub test_run_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InternalTestReportRequest {
    pub session: InternalTestReportSession,
    pub test_run_id: String,
    pub user_note: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InternalTestReport {
    pub report_id: String,
    pub test_run_id: String,
    pub zip_path: PathBuf,
    pub preview_text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InternalTestReportError {
    #[error("share code is not a Basement Bridge test code")]
    InvalidShareCodePrefix,
    #[error("share code is invalid: {0}")]
    InvalidShareCode(String),
    #[error("upload endpoint is not configured")]
    MissingUploadEndpoint,
    #[error("upload credential is not configured")]
    MissingUploadToken,
    #[error("upload endpoint must use http://")]
    UnsupportedUploadEndpoint,
    #[error("upload endpoint is invalid: {0}")]
    InvalidUploadEndpoint(String),
    #[error("upload failed with HTTP {status_code}: {body}")]
    UploadRejected { status_code: u16, body: String },
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Zip(#[from] zip::result::ZipError),
}

impl InternalTestShareCode {
    #[must_use]
    pub fn encode(&self) -> String {
        let payload = ShareCodePayload::from(self);
        let json = serde_json::to_vec(&payload).unwrap_or_default();
        format!("{SHARE_CODE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    pub fn decode(value: &str) -> Result<Self, InternalTestReportError> {
        let trimmed = value.trim();
        let Some(encoded) = trimmed.strip_prefix(SHARE_CODE_PREFIX) else {
            return Err(InternalTestReportError::InvalidShareCodePrefix);
        };
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|error| InternalTestReportError::InvalidShareCode(error.to_string()))?;
        let payload = serde_json::from_slice::<ShareCodePayload>(&bytes)?;
        payload.try_into()
    }
}

#[must_use]
pub fn new_internal_test_id() -> String {
    Uuid::new_v4().hyphenated().to_string()
}

impl BridgeClient {
    pub fn open_internal_test_report_directory(&mut self) -> io::Result<Option<PathBuf>> {
        let directory = diagnostics_directory().join("internal-test");
        if !directory.exists() {
            return Ok(None);
        }
        open::that_detached(&directory)?;
        self.log(
            LogLevel::Info,
            format!(
                "Opened internal test report directory {}",
                directory.display()
            ),
        );
        Ok(Some(directory))
    }

    pub fn prepare_internal_test_report(
        &mut self,
        request: InternalTestReportRequest,
    ) -> Result<InternalTestReport, InternalTestReportError> {
        let report_id = new_internal_test_id();
        let directory = diagnostics_directory().join("internal-test");
        fs::create_dir_all(&directory)?;
        let zip_path = directory.join(format!("basement-bridge-report-{report_id}.zip"));
        let metadata = self.report_metadata(&report_id, &request);
        let diagnostics_text = self.redacted_diagnostics_text();
        let preview_text = report_preview_text(&metadata, &diagnostics_text);
        write_report_zip(
            &zip_path,
            &metadata,
            &diagnostics_text,
            self.log_sink.process_log_path(),
            self.log_sink.recent_session_logs(),
        )?;
        self.log(
            LogLevel::Info,
            format!(
                "Internal test report prepared: report_id={report_id} test_run_id={} path={}",
                request.test_run_id,
                zip_path.display()
            ),
        );
        Ok(InternalTestReport {
            report_id,
            test_run_id: request.test_run_id,
            zip_path,
            preview_text,
        })
    }

    pub fn upload_internal_test_report(
        &mut self,
        report: &InternalTestReport,
    ) -> Result<InternalTestUploadReceipt, InternalTestReportError> {
        let config = &self.loaded_config.config.internal_test;
        let endpoint = config
            .upload_endpoint
            .as_deref()
            .ok_or(InternalTestReportError::MissingUploadEndpoint)?;
        let token = config
            .upload_token
            .as_deref()
            .ok_or(InternalTestReportError::MissingUploadToken)?;
        let body = fs::read(&report.zip_path)?;
        let receipt = post_zip(
            endpoint,
            token,
            &report.report_id,
            &report.test_run_id,
            &body,
        )?;
        self.log(
            LogLevel::Info,
            format!(
                "Internal test report uploaded: report_id={} test_run_id={} status={}",
                report.report_id, report.test_run_id, receipt.status_code
            ),
        );
        Ok(receipt)
    }

    fn report_metadata(
        &self,
        report_id: &str,
        request: &InternalTestReportRequest,
    ) -> ReportMetadata {
        ReportMetadata {
            schema_version: REPORT_SCHEMA_VERSION,
            report_id: report_id.to_owned(),
            test_run_id: request.test_run_id.clone(),
            created_at_unix: unix_seconds(),
            product: PRODUCT_NAME.to_owned(),
            runtime: runtime_name().to_owned(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            session: request.session.clone(),
            user_note: request.user_note.clone(),
            observability: ObservabilitySummary::from_state(
                self.state.latest_readiness_probe.as_ref(),
                self.state.latest_hook_receive_probe.as_ref(),
                self.state.latest_hook_receive_probe_error.as_deref(),
                self.state
                    .latest_session_health_summary
                    .as_ref()
                    .or(self.state.latest_session_health.as_ref()),
            ),
            readiness: self.state.latest_readiness_probe.clone(),
            hook_receive: self.state.latest_hook_receive_probe.clone(),
            hook_receive_error: self.state.latest_hook_receive_probe_error.clone(),
            session_health: self
                .state
                .latest_session_health_summary
                .clone()
                .or_else(|| self.state.latest_session_health.clone()),
            udp_fec: self.state.latest_udp_fec.clone(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ShareCodePayload {
    version: u8,
    relay_name: Option<String>,
    relay_host: String,
    relay_port: u16,
    transport: String,
    udp_fec: Option<UdpFecConfig>,
    room: String,
    mode: String,
    test_run_id: String,
}

impl From<&InternalTestShareCode> for ShareCodePayload {
    fn from(value: &InternalTestShareCode) -> Self {
        Self {
            version: 1,
            relay_name: value.session.relay_name.clone(),
            relay_host: value.session.relay.host.clone(),
            relay_port: value.session.relay.port,
            transport: serialize_transport(value.session.transport).to_owned(),
            udp_fec: Some(value.session.udp_fec),
            room: value.session.room.clone(),
            mode: serialize_mode(value.session.mode).to_owned(),
            test_run_id: value.test_run_id.clone(),
        }
    }
}

impl TryFrom<ShareCodePayload> for InternalTestShareCode {
    type Error = InternalTestReportError;

    fn try_from(value: ShareCodePayload) -> Result<Self, Self::Error> {
        if value.version != 1 {
            return Err(InternalTestReportError::InvalidShareCode(format!(
                "unsupported version {}",
                value.version
            )));
        }
        let relay = RelayEndpoint::new(value.relay_host.trim(), value.relay_port);
        relay
            .validate()
            .map_err(|error| InternalTestReportError::InvalidShareCode(error.to_string()))?;
        let transport = parse_transport(&value.transport)?;
        let mut udp_fec = value.udp_fec.unwrap_or_default();
        if transport != TransportChoice::Udp {
            udp_fec.enabled = false;
        }
        let room = value.room.trim().to_owned();
        if room.is_empty() {
            return Err(InternalTestReportError::InvalidShareCode(
                "room is required".to_owned(),
            ));
        }
        let test_run_id = value.test_run_id.trim().to_owned();
        if test_run_id.is_empty() {
            return Err(InternalTestReportError::InvalidShareCode(
                "test_run_id is required".to_owned(),
            ));
        }
        Ok(Self {
            session: InternalTestReportSession {
                relay_name: value
                    .relay_name
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !name.is_empty()),
                relay,
                transport,
                udp_fec,
                room,
                mode: parse_mode(&value.mode)?,
            },
            test_run_id,
        })
    }
}

#[derive(Debug, Serialize)]
struct ReportMetadata {
    schema_version: u8,
    report_id: String,
    test_run_id: String,
    created_at_unix: u64,
    product: String,
    runtime: String,
    app_version: String,
    session: InternalTestReportSession,
    user_note: String,
    observability: ObservabilitySummary,
    readiness: Option<ReadinessProbeReport>,
    hook_receive: Option<HookReceiveProbeReport>,
    hook_receive_error: Option<String>,
    session_health: Option<SessionHealthSnapshot>,
    udp_fec: Option<UdpFecSessionSnapshot>,
}

#[derive(Debug, Serialize)]
struct ObservabilitySummary {
    readiness: String,
    runtime_rtt: String,
    local_queue_pressure: String,
    relay_packet_continuity: String,
    hook_output: String,
    attribution_note: &'static str,
}

impl ObservabilitySummary {
    fn from_state(
        readiness: Option<&ReadinessProbeReport>,
        hook_receive: Option<&HookReceiveProbeReport>,
        hook_receive_error: Option<&str>,
        session_health: Option<&SessionHealthSnapshot>,
    ) -> Self {
        Self {
            readiness: readiness
                .map(readiness_summary)
                .unwrap_or_else(|| "not run".to_owned()),
            runtime_rtt: session_health
                .map(runtime_rtt_summary)
                .unwrap_or_else(|| "no session health".to_owned()),
            local_queue_pressure: session_health
                .map(queue_summary)
                .unwrap_or_else(|| "no session health".to_owned()),
            relay_packet_continuity: session_health
                .map(packet_continuity_summary)
                .unwrap_or_else(|| "no session health".to_owned()),
            hook_output: hook_receive
                .map(|report| report.short_summary())
                .or_else(|| hook_receive_error.map(ToOwned::to_owned))
                .or_else(|| session_health.map(hook_output_summary))
                .unwrap_or_else(|| "not checked".to_owned()),
            attribution_note: "Evidence localizes observed symptoms; compare both players and relay logs before assigning root cause.",
        }
    }
}

fn write_report_zip(
    path: &Path,
    metadata: &ReportMetadata,
    diagnostics_text: &str,
    process_log_path: Option<PathBuf>,
    session_log_paths: Vec<PathBuf>,
) -> Result<(), InternalTestReportError> {
    let file = File::create(path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    write_zip_text(
        &mut zip,
        options,
        "report.json",
        &serde_json::to_string_pretty(metadata)?,
    )?;
    write_zip_text(&mut zip, options, "diagnostics.txt", diagnostics_text)?;
    write_optional_json(
        &mut zip,
        options,
        "artifacts/readiness.json",
        &metadata.readiness,
    )?;
    write_optional_json(
        &mut zip,
        options,
        "artifacts/hook_receive.json",
        &metadata.hook_receive,
    )?;
    write_optional_json(
        &mut zip,
        options,
        "artifacts/session_health.json",
        &metadata.session_health,
    )?;
    write_optional_json(
        &mut zip,
        options,
        "artifacts/udp_fec.json",
        &metadata.udp_fec,
    )?;
    if let Some(path) = process_log_path {
        write_zip_file(&mut zip, options, "logs/process", &path)?;
    }
    for path in session_log_paths {
        write_zip_file(&mut zip, options, "logs/sessions", &path)?;
    }
    zip.finish()?;
    Ok(())
}

fn report_preview_text(metadata: &ReportMetadata, diagnostics_text: &str) -> String {
    let mut output = String::new();
    output.push_str("Report metadata\n");
    output.push_str(&format!("report_id: {}\n", metadata.report_id));
    output.push_str(&format!("test_run_id: {}\n", metadata.test_run_id));
    if let Some(relay_name) = metadata.session.relay_name.as_deref() {
        output.push_str(&format!("relay_name: {relay_name}\n"));
    }
    output.push_str(&format!(
        "relay: {}:{}\n",
        metadata.session.relay.host, metadata.session.relay.port
    ));
    output.push_str(&format!("transport: {}\n", metadata.session.transport));
    output.push_str(&format!(
        "connection_profile: {}\n",
        connection_profile_summary(&metadata.session)
    ));
    output.push_str(&format!("room: {}\n", metadata.session.room));
    output.push_str(&format!("mode: {}\n", metadata.session.mode));
    if !metadata.user_note.is_empty() {
        output.push_str("user_note:\n");
        output.push_str(&metadata.user_note);
        output.push('\n');
    }
    output.push_str("\nDiagnostics preview\n");
    output.push_str(diagnostics_text);
    if !diagnostics_text.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn connection_profile_summary(session: &InternalTestReportSession) -> String {
    match session.transport {
        TransportChoice::Tcp => "tcp".to_owned(),
        TransportChoice::Udp if session.udp_fec.enabled => {
            format!("udp+fec:{}", session.udp_fec.profile)
        }
        TransportChoice::Udp => "udp".to_owned(),
    }
}

fn write_optional_json<T: Serialize>(
    zip: &mut ZipWriter<File>,
    options: SimpleFileOptions,
    name: &str,
    value: &Option<T>,
) -> Result<(), InternalTestReportError> {
    let text = match value {
        Some(value) => serde_json::to_string_pretty(value)?,
        None => "null\n".to_owned(),
    };
    write_zip_text(zip, options, name, &text)
}

fn write_zip_text(
    zip: &mut ZipWriter<File>,
    options: SimpleFileOptions,
    name: &str,
    contents: &str,
) -> Result<(), InternalTestReportError> {
    zip.start_file(name, options)?;
    zip.write_all(contents.as_bytes())?;
    if !contents.ends_with('\n') {
        zip.write_all(b"\n")?;
    }
    Ok(())
}

fn write_zip_file(
    zip: &mut ZipWriter<File>,
    options: SimpleFileOptions,
    directory: &str,
    path: &Path,
) -> Result<(), InternalTestReportError> {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let name = format!("{directory}/{file_name}");
    zip.start_file(name, options)?;
    match fs::read(path) {
        Ok(contents) => zip.write_all(&contents)?,
        Err(error) => zip.write_all(format!("unavailable: {error}\n").as_bytes())?,
    }
    Ok(())
}

fn readiness_summary(report: &ReadinessProbeReport) -> String {
    let cases = report.cases.len();
    let lost = report
        .cases
        .iter()
        .map(|case| case.missing_packets)
        .sum::<u64>();
    let worst_p95 = report
        .cases
        .iter()
        .filter_map(|case| case.p95_latency_ms)
        .max();
    let worst_jitter = report.cases.iter().filter_map(|case| case.jitter_ms).max();
    format!(
        "cases={cases}; missing_packets={lost}; worst_p95_ms={}; worst_jitter_ms={}",
        display_optional(worst_p95),
        display_optional(worst_jitter)
    )
}

fn runtime_rtt_summary(snapshot: &SessionHealthSnapshot) -> String {
    let rtt = snapshot.runtime_rtt;
    format!(
        "sent={}; received={}; timed_out={}; pending={}; p95_ms={}",
        rtt.sent,
        rtt.received,
        rtt.timed_out,
        rtt.pending,
        display_optional(rtt.latency.p95_ms)
    )
}

fn queue_summary(snapshot: &SessionHealthSnapshot) -> String {
    format!(
        "outbound_dropped={}; inbound_dropped={}; total_dropped={}",
        snapshot.queues.outbound_dropped,
        snapshot.queues.inbound_dropped,
        snapshot.queues.total_dropped()
    )
}

fn packet_continuity_summary(snapshot: &SessionHealthSnapshot) -> String {
    format!(
        "relay_gap_p95_ms={}; sequence_gaps={}; duplicate_or_reordered={}",
        display_optional(snapshot.relay_recv.gap.p95_ms),
        snapshot.source_sequence.gaps,
        snapshot.source_sequence.duplicate_or_reordered
    )
}

fn hook_output_summary(snapshot: &SessionHealthSnapshot) -> String {
    format!(
        "hook_out_p95_ms={}; over_500ms={}; over_1000ms={}",
        display_optional(snapshot.hook_out_send_duration.p95_ms),
        snapshot.hook_out_send_duration.over_500_ms,
        snapshot.hook_out_send_duration.over_1000_ms
    )
}

fn serialize_transport(value: TransportChoice) -> &'static str {
    match value {
        TransportChoice::Udp => "udp",
        TransportChoice::Tcp => "tcp",
    }
}

fn parse_transport(value: &str) -> Result<TransportChoice, InternalTestReportError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "udp" => Ok(TransportChoice::Udp),
        "tcp" => Ok(TransportChoice::Tcp),
        other => Err(InternalTestReportError::InvalidShareCode(format!(
            "invalid transport {other}"
        ))),
    }
}

fn serialize_mode(value: SessionMode) -> &'static str {
    match value {
        SessionMode::Official => "official",
        SessionMode::Fallback => "fallback",
        SessionMode::Pure => "pure",
    }
}

fn parse_mode(value: &str) -> Result<SessionMode, InternalTestReportError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "official" => Ok(SessionMode::Official),
        "fallback" => Ok(SessionMode::Fallback),
        "pure" => Ok(SessionMode::Pure),
        other => Err(InternalTestReportError::InvalidShareCode(format!(
            "invalid mode {other}"
        ))),
    }
}

fn display_optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_code_round_trips_without_private_fields() {
        let code = InternalTestShareCode {
            session: InternalTestReportSession {
                relay_name: Some("Test relay".to_owned()),
                relay: RelayEndpoint::new("relay.example.test", 25_910),
                transport: TransportChoice::Udp,
                udp_fec: UdpFecConfig {
                    enabled: true,
                    ..UdpFecConfig::default()
                },
                room: "bb-test".to_owned(),
                mode: SessionMode::Pure,
            },
            test_run_id: "run-123".to_owned(),
        }
        .encode();

        assert!(code.starts_with(SHARE_CODE_PREFIX));
        assert!(!code.contains("7656119"));
        assert!(!code.contains("upload"));

        let decoded = InternalTestShareCode::decode(&code).unwrap();

        assert_eq!(decoded.test_run_id, "run-123");
        assert_eq!(decoded.session.relay.host, "relay.example.test");
        assert_eq!(decoded.session.transport, TransportChoice::Udp);
        assert!(decoded.session.udp_fec.enabled);
    }

    #[test]
    fn rejects_non_bridge_share_code() {
        let error = InternalTestShareCode::decode("not-a-code").unwrap_err();

        assert!(matches!(
            error,
            InternalTestReportError::InvalidShareCodePrefix
        ));
    }
}
