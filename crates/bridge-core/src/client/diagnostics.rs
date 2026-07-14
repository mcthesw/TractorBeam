use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
};

use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::{BridgeClient, PRODUCT_NAME, state::unix_seconds};

const MAX_PACKAGE_FILES: usize = 16;
const MAX_PACKAGE_ENTRY_BYTES: usize = 256 * 1024;
const MAX_PACKAGE_TOTAL_BYTES: usize = 2 * 1024 * 1024;

impl BridgeClient {
    pub fn open_log_directory(&mut self) -> io::Result<PathBuf> {
        let Some(directory) = self.log_sink.root() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "log directory is unavailable",
            ));
        };
        fs::create_dir_all(&directory)?;
        open::that_detached(&directory)?;
        self.log(
            super::LogLevel::Info,
            format!("Opened log directory {}", directory.display()),
        );
        Ok(directory)
    }

    pub fn export_troubleshooting_package(&mut self, path: &Path) -> io::Result<PathBuf> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "package path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        push_text_entry(
            &mut entries,
            "summary.txt",
            self.redacted_diagnostics_text(),
            &mut warnings,
        );
        collect_optional_file(
            &mut entries,
            "logs/bridge-client.log",
            self.log_sink.process_log_path(),
            &mut warnings,
        );
        for (index, session_path) in self
            .log_sink
            .recent_session_logs()
            .into_iter()
            .take(8)
            .enumerate()
        {
            collect_optional_file(
                &mut entries,
                &format!("logs/sessions/session-{:02}.log", index + 1),
                Some(session_path),
                &mut warnings,
            );
        }
        collect_optional_file(
            &mut entries,
            "logs/tractor-beam-hook.log",
            self.state.hook_log_path_written(),
            &mut warnings,
        );
        collect_optional_file(
            &mut entries,
            "logs/isaac-online-excerpt.log",
            Some(
                crate::diagnostics::isaac_online_logs_directory()
                    .join(crate::diagnostics::ONLINE_LOG),
            ),
            &mut warnings,
        );
        let manifest = package_manifest(&entries, &warnings);
        push_text_entry(&mut entries, "manifest.txt", manifest, &mut warnings);
        enforce_total_bound(&mut entries, &mut warnings);

        let temporary_path = temporary_package_path(path);
        if temporary_path.exists() {
            fs::remove_file(&temporary_path)?;
        }
        let result = write_package(&temporary_path, &entries).and_then(|()| {
            if path.exists() {
                fs::remove_file(path)?;
            }
            fs::rename(&temporary_path, path)
        });
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result?;
        self.log(
            super::LogLevel::Info,
            format!("Troubleshooting Package saved to {}", path.display()),
        );
        Ok(path.to_path_buf())
    }

    #[must_use]
    pub fn redacted_diagnostics_text(&self) -> String {
        crate::diagnostics::redact_text(&self.diagnostics_text())
    }

    #[must_use]
    pub fn diagnostics_text(&self) -> String {
        let mut output = String::new();
        output.push_str(PRODUCT_NAME);
        output.push_str(" diagnostics\n\n");
        output.push_str(&format!(
            "version: {}\n",
            crate::build_info::version_label()
        ));
        output.push_str(&format!("status: {:?}\n", self.state.status));
        output.push_str(&format!(
            "hook_to_relay: {}\n",
            self.state.counters.hook_to_relay
        ));
        output.push_str(&format!(
            "relay_to_hook: {}\n",
            self.state.counters.relay_to_hook
        ));
        output.push_str(&format!("sent_bytes: {}\n", self.state.counters.sent_bytes));
        output.push_str(&format!(
            "received_bytes: {}\n",
            self.state.counters.received_bytes
        ));
        output.push_str(&format!("errors: {}\n", self.state.counters.errors));
        output.push_str(&format!(
            "reconnect_dropped_packets: {}\n",
            self.state.counters.reconnect_dropped_packets
        ));
        output.push_str(&format!("relay_link: {:?}\n\n", self.state.relay_link));
        output.push_str("client config:\n");
        if let Some(path) = &self.loaded_config.source {
            output.push_str(&format!("source: {}\n", path.display()));
        } else {
            output.push_str("source: built-in defaults\n");
        }
        output.push_str(&format!(
            "default_transport: {}\ndefault_mode: {}\nrelay_presets: {}\n",
            self.loaded_config.config.default_transport,
            self.loaded_config.config.default_mode,
            self.loaded_config.config.relays.len()
        ));
        if let Some(root) = self.log_sink.root() {
            output.push_str(&format!("log_directory: {}\n\n", root.display()));
        } else {
            output.push_str("log_directory: unavailable\n\n");
        }
        output.push_str("session lifecycle:\n");
        if let Some(reason) = &self.state.last_stop_reason {
            output.push_str(&format!("last_stop_reason: {reason}\n\n"));
        } else {
            output.push_str("last_stop_reason: none\n\n");
        }
        output.push_str("latest probes:\n");
        if let Some(report) = &self.state.latest_readiness_probe {
            output.push_str(&format!("readiness: {}\n", report.detailed_log()));
        } else {
            output.push_str("readiness: none\n");
        }
        if let Some(report) = &self.state.latest_hook_receive_probe {
            output.push_str(&format!("hook_receive: {report}\n"));
        } else {
            output.push_str("hook_receive: none\n");
        }
        if let Some(error) = &self.state.latest_hook_receive_probe_error {
            output.push_str(&format!("hook_receive_error: {error}\n"));
        }
        if let Some(status) = &self.state.latest_input_delay_status {
            match &status.result {
                Ok(value) => output.push_str(&format!(
                    "input_delay: operation={} result=ok value={} updated_at={}\n",
                    status.operation, value, status.updated_at
                )),
                Err(error) => output.push_str(&format!(
                    "input_delay: operation={} result=error error={} updated_at={}\n",
                    status.operation, error, status.updated_at
                )),
            }
        } else {
            output.push_str("input_delay: none\n");
        }
        match serde_json::to_string(&self.input_delay_evidence()) {
            Ok(json) => output.push_str(&format!("input_delay_evidence: {json}\n")),
            Err(error) => {
                output.push_str(&format!("input_delay_evidence: unavailable ({error})\n"))
            }
        }
        output.push('\n');
        output.push_str("native hook local IPC:\n");
        let ipc = &self.state.hook_ipc;
        output.push_str(&format!("connection: {}\n", ipc.connection));
        match (ipc.negotiated_major, ipc.negotiated_minor) {
            (Some(major), Some(minor)) => {
                output.push_str(&format!("negotiated_version: {major}.{minor}\n"));
            }
            _ => output.push_str("negotiated_version: none\n"),
        }
        output.push_str(&format!("reconnects: {}\n", ipc.reconnects));
        output.push_str(&format!(
            "hook_data_dropped: {}\nclient_data_dropped: {}\nmalformed_frames: {}\nupdated_at: {}\n",
            ipc.hook_data_dropped,
            ipc.client_data_dropped,
            ipc.malformed_frames,
            ipc.updated_at,
        ));
        if let Some(error) = &ipc.last_error {
            output.push_str(&format!("last_error: {error}\n"));
        }
        output.push('\n');
        output.push_str("native hook startup:\n");
        let startup = &self.state.hook_startup;
        if startup.is_started() {
            output.push_str(&format!("phase: {}\n", startup.phase));
            output.push_str(&format!("injected: {}\n", startup.injected));
            output.push_str(&format!("endpoint_ready: {}\n", startup.endpoint_ready));
            output.push_str(&format!("access_denied: {}\n", startup.access_denied));
            output.push_str(&format!("updated_at: {}\n", startup.updated_at));
            if let Some(process_name) = &startup.process_name {
                output.push_str(&format!("process_name: {process_name}\n"));
            }
            if let Some(pid) = startup.pid {
                output.push_str(&format!("pid: {pid}\n"));
            }
            if let Some(path) = &startup.injector_path {
                output.push_str(&format!("injector_path: {}\n", path.display()));
            }
            if let Some(path) = &startup.hook_path {
                output.push_str(&format!("hook_path: {}\n", path.display()));
            }
            if let Some(path) = &startup.launch_parameters_path {
                output.push_str(&format!("launch_parameters_path: {}\n", path.display()));
            }
            if let Some(endpoint) = &startup.endpoint {
                output.push_str(&format!("endpoint: {endpoint}\n"));
            }
            if let Some(message) = &startup.message {
                output.push_str(&format!("message: {message}\n"));
            }
        } else {
            output.push_str("phase: not_started\n");
        }
        output.push('\n');
        output.push_str("session health:\n");
        if let Some(snapshot) = self
            .state
            .latest_session_health_summary
            .as_ref()
            .or(self.state.latest_session_health.as_ref())
        {
            output.push_str(&snapshot.compact_log_line("summary"));
            output.push('\n');
            match serde_json::to_string_pretty(snapshot) {
                Ok(json) => {
                    output.push_str(&json);
                    output.push('\n');
                }
                Err(error) => output.push_str(&format!("json_unavailable: {error}\n")),
            }
        } else {
            output.push_str("none\n");
        }
        output.push('\n');
        output.push_str("current smoothness:\n");
        match serde_json::to_string_pretty(&self.state.smoothness) {
            Ok(json) => {
                output.push_str(&json);
                output.push('\n');
            }
            Err(error) => output.push_str(&format!("json_unavailable: {error}\n")),
        }
        output.push('\n');
        output.push_str("room path quality:\n");
        if self.state.room_path_quality.is_empty() {
            output.push_str("none\n");
        } else {
            for quality in &self.state.room_path_quality {
                output.push_str(&format!(
                    "state={:?} completed={} responses={} median_ms={} p95_ms={} jitter_ms={} loss_basis_points={} freshness_ms={}\n",
                    quality.state,
                    quality.completed,
                    quality.responses,
                    display_duration_ms(quality.median_rtt),
                    display_duration_ms(quality.p95_rtt),
                    display_duration_ms(quality.jitter),
                    quality
                        .loss_basis_points
                        .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                    display_duration_ms(quality.freshness),
                ));
            }
        }
        output.push('\n');
        output.push_str("client incidents:\n");
        if self.state.client_incidents.is_empty() {
            output.push_str("none\n\n");
        } else {
            for incident in &self.state.client_incidents {
                output.push_str(&format!(
                    "- [{}] {}: {}\n",
                    incident.timestamp, incident.kind, incident.summary
                ));
                output.push_str(&format!(
                    "  {}\n",
                    incident.health.compact_log_line("health")
                ));
            }
            output.push('\n');
        }
        output.push_str("hook runtime files:\n");
        if let Some(path) = &self.state.hook_launch_parameters_path_written {
            output.push_str(&format!(
                "launch_parameters_path_written: {}\n",
                path.display()
            ));
            if let Some(cleanup) = &self.state.hook_launch_parameters_cleanup {
                output.push_str(&format!("launch_parameters_cleanup: {cleanup}\n"));
            } else {
                output.push_str("launch_parameters_cleanup: none\n");
            }
            if let Some(hook_log_path) = self.state.hook_log_path_written() {
                output.push_str(&format!(
                    "hook_log_path_expected: {}\n",
                    hook_log_path.display()
                ));
            }
            if let Some(directory) = path.parent() {
                for file in [
                    crate::diagnostics::BRIDGE_CONFIG_FILE,
                    crate::diagnostics::BRIDGE_HOOK_LOG,
                ] {
                    let path = directory.join(file);
                    output.push_str("\n--- ");
                    output.push_str(file);
                    output.push_str(" ---\n");
                    match read_text_excerpt(&path) {
                        Ok(contents) => output.push_str(&contents),
                        Err(error) => output.push_str(&format!("unavailable: {error}\n")),
                    }
                    if !output.ends_with('\n') {
                        output.push('\n');
                    }
                }
            }
        } else {
            output.push_str("launch_parameters_path_written: none\n");
        }
        output.push_str("\nIsaac online log excerpts:\n");
        let log_directory = crate::diagnostics::isaac_online_logs_directory();
        output.push_str(&format!("directory: {}\n", log_directory.display()));
        let file = crate::diagnostics::ONLINE_LOG;
        let path = log_directory.join(file);
        output.push_str("\n--- ");
        output.push_str(file);
        output.push_str(" ---\n");
        match read_text_excerpt(&path) {
            Ok(contents) => output.push_str(&contents),
            Err(error) => output.push_str(&format!("unavailable: {error}\n")),
        }
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("\nlogs:\n");
        for entry in &self.state.logs {
            output.push_str(&format!(
                "[{}] {} {}\n",
                format_evidence_timestamp(entry.timestamp_ms),
                entry.level,
                entry.message
            ));
        }
        output.push_str("\nprocess log:\n");
        if let Some(path) = self.log_sink.process_log_path() {
            output.push_str(&format!("--- {} ---\n", path.display()));
            match read_text_excerpt(&path) {
                Ok(contents) => output.push_str(&contents),
                Err(error) => output.push_str(&format!("unavailable: {error}\n")),
            }
        } else {
            output.push_str("unavailable\n");
        }
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("\nsession logs:\n");
        for path in self.log_sink.recent_session_logs() {
            output.push_str("\n--- ");
            output.push_str(&path.display().to_string());
            output.push_str(" ---\n");
            match read_text_excerpt(&path) {
                Ok(contents) => output.push_str(&contents),
                Err(error) => output.push_str(&format!("unavailable: {error}\n")),
            }
            if !output.ends_with('\n') {
                output.push('\n');
            }
        }
        output
    }
}

fn display_duration_ms(value: Option<std::time::Duration>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.as_millis().to_string())
}

mod package;

use package::*;

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;
    use crate::client::{BridgeClient, LoadedClientConfig, LogLevel, state::log_entry};

    #[test]
    fn troubleshooting_package_is_a_redacted_bounded_zip() {
        let mut client = BridgeClient::with_config(LoadedClientConfig::default());
        client.state.logs.push(log_entry(
            LogLevel::Info,
            "session_credential=secret-session resume_credential=secret-resume path_token=secret-path connection_id=42 C:\\Users\\alice\\save",
        ));
        let directory = std::env::temp_dir().join(format!(
            "tractor-beam-package-test-{}-{}",
            std::process::id(),
            unix_seconds()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("support.zip");

        assert_eq!(client.export_troubleshooting_package(&path).unwrap(), path);
        assert!(path.exists());
        assert!(!temporary_package_path(&path).exists());

        let file = File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert!(archive.len() <= MAX_PACKAGE_FILES);
        let mut combined = String::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            assert!(entry.size() <= MAX_PACKAGE_ENTRY_BYTES as u64);
            entry.read_to_string(&mut combined).unwrap();
        }
        assert!(combined.contains("Tractor Beam Troubleshooting Package"));
        for secret in [
            "secret-session",
            "secret-resume",
            "secret-path",
            "connection_id=42",
            "alice",
        ] {
            assert!(!combined.contains(secret), "package leaked {secret}");
        }

        fs::remove_dir_all(directory).ok();
    }
}
