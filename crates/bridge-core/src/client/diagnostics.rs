use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::{BridgeClient, PRODUCT_NAME, state::unix_seconds};

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

    pub fn export_diagnostics_bundle(&mut self, path: &Path) -> io::Result<PathBuf> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Diagnostics Bundle path has no parent",
            )
        })?;
        fs::create_dir_all(parent)?;
        let mut sources = self
            .log_sink
            .log_files()
            .into_iter()
            .filter_map(|path| package_source("logs/client", path, None))
            .collect::<Vec<_>>();
        if sources.is_empty() {
            sources.push(missing_package_source("logs/client/*.log"));
        }
        if let Some(root) = self.log_sink.root() {
            let hook_sources = crate::diagnostics::daily_log_files(&root.join("hook"))
                .into_iter()
                .filter_map(|path| package_source("logs/hook", path, None))
                .collect::<Vec<_>>();
            if hook_sources.is_empty() {
                sources.push(missing_package_source("logs/hook/*.log"));
            } else {
                sources.extend(hook_sources);
            }
        } else {
            sources.push(missing_package_source("logs/hook/*.log"));
        }
        sources.push(PackageSource {
            archive_name: "logs/isaac/online.log".to_owned(),
            path: Some(
                crate::diagnostics::isaac_online_logs_directory()
                    .join(crate::diagnostics::ONLINE_LOG),
            ),
            tail_bytes: Some(crate::diagnostics::MAX_ISAAC_EXCERPT_BYTES),
        });

        write_diagnostics_bundle(path, &self.diagnostics_text(), &sources)?;
        self.log(
            super::LogLevel::Info,
            format!("Diagnostics Bundle saved to {}", path.display()),
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
        output.push_str("direct LAN:\n");
        output.push_str(&format!("peers: {}\n", self.state.lan_peers.len()));
        for peer in &self.state.lan_peers {
            output.push_str(&format!(
                "peer: steam_id64={} instance={:?} connection={:?}\n",
                peer.peer.identity.steam_id64, peer.peer.identity.instance_id, peer.connection
            ));
        }
        for path in &self.state.lan_paths {
            output.push_str(&format!(
                "path: peer={} status={:?} local={:?} remote={:?}\n",
                path.peer.steam_id64, path.status, path.local_endpoint, path.remote_endpoint
            ));
        }
        output.push('\n');
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
            output.push_str(&format!("runtime_file_present: {}\n", path.is_file()));
        } else {
            output.push_str("launch_parameters_path_written: none\n");
        }
        output.push_str("\nIsaac online log:\n");
        let log_directory = crate::diagnostics::isaac_online_logs_directory();
        output.push_str(&format!("directory: {}\n", log_directory.display()));
        output.push_str(&format!(
            "path: {}\n",
            log_directory.join(crate::diagnostics::ONLINE_LOG).display()
        ));
        output
    }
}

fn package_source(prefix: &str, path: PathBuf, tail_bytes: Option<u64>) -> Option<PackageSource> {
    let name = path.file_name()?.to_str()?;
    Some(PackageSource {
        archive_name: format!("{prefix}/{name}"),
        path: Some(path),
        tail_bytes,
    })
}

fn missing_package_source(archive_name: &str) -> PackageSource {
    PackageSource {
        archive_name: archive_name.to_owned(),
        path: None,
        tail_bytes: None,
    }
}

fn display_duration_ms(value: Option<std::time::Duration>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.as_millis().to_string())
}

mod package;

use package::*;

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, io::Read as _};

    use super::*;
    use crate::client::{BridgeClient, ClientLogSink, LoadedClientConfig};

    #[derive(Debug)]
    struct TestLogSink {
        root: PathBuf,
        client_logs: Vec<PathBuf>,
    }

    impl ClientLogSink for TestLogSink {
        fn root(&self) -> Option<PathBuf> {
            Some(self.root.clone())
        }

        fn log_files(&self) -> Vec<PathBuf> {
            self.client_logs.clone()
        }
    }

    #[test]
    fn direct_lan_diagnostics_include_path_evidence_without_path_secrets() {
        use crate::client::{LanPeerPathState, LanPeerPathStatus};
        use tractor_beam_direct_protocol::{InstanceId, PeerIdentity};

        let mut client = BridgeClient::with_config(LoadedClientConfig::default());
        client.state.lan_paths.push(LanPeerPathState {
            peer: PeerIdentity::new(7, InstanceId::from_bytes([7; 16])),
            status: LanPeerPathStatus::Usable,
            local_endpoint: Some("192.168.1.2:30000".parse().unwrap()),
            remote_endpoint: Some("192.168.1.3:30001".parse().unwrap()),
        });

        let raw = client.diagnostics_text();
        assert!(raw.contains("status=Usable"));
        assert!(raw.contains("192.168.1.2:30000"));
        assert!(!raw.contains("path_token"));
        assert!(!raw.contains("path_id"));
        assert!(!raw.contains("session_credential"));

        let redacted = client.redacted_diagnostics_text();
        assert!(!redacted.contains("192.168.1.2"));
        assert!(!redacted.contains("192.168.1.3"));
    }

    #[test]
    fn diagnostics_bundle_streams_redacted_daily_logs_without_summary_duplication() {
        let directory = std::env::temp_dir().join(format!(
            "tractor-beam-package-test-{}-{}",
            std::process::id(),
            unix_seconds()
        ));
        let log_root = directory.join("logs");
        let client_dir = log_root.join("client");
        let hook_dir = log_root.join("hook");
        fs::create_dir_all(&client_dir).unwrap();
        fs::create_dir_all(&hook_dir).unwrap();
        let client_log = client_dir.join("2026-07-16.log");
        let hook_log = hook_dir.join("2026-07-16.log");
        fs::write(
            &client_log,
            "client-marker token=secret-token C:\\Users\\alice\\save\n",
        )
        .unwrap();
        fs::write(&hook_log, "hook-marker ipc_session=0011223344556677\n").unwrap();
        let mut client = BridgeClient::with_config_and_log_sink(
            LoadedClientConfig::default(),
            Box::new(TestLogSink {
                root: log_root,
                client_logs: vec![client_log],
            }),
        );
        let path = directory.join("support.zip");
        fs::write(&path, "old bundle").unwrap();

        assert_eq!(client.export_diagnostics_bundle(&path).unwrap(), path);
        assert!(path.exists());

        let file = File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entries = BTreeMap::new();
        let mut entry_order = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let mut contents = String::new();
            entry.read_to_string(&mut contents).unwrap();
            entry_order.push(entry.name().to_owned());
            entries.insert(entry.name().to_owned(), contents);
        }
        assert_eq!(entry_order.last().map(String::as_str), Some("manifest.txt"));
        assert_eq!(
            entries.keys().cloned().collect::<Vec<_>>(),
            [
                "logs/client/2026-07-16.log",
                "logs/hook/2026-07-16.log",
                "manifest.txt",
                "summary.txt",
            ]
        );
        assert!(!entries["summary.txt"].contains("client-marker"));
        assert!(entries["logs/client/2026-07-16.log"].contains("client-marker"));
        assert!(entries["logs/hook/2026-07-16.log"].contains("hook-marker"));
        assert!(entries["manifest.txt"].contains("Tractor Beam Diagnostics Bundle"));
        let combined = entries.values().cloned().collect::<String>();
        for secret in ["secret-token", "alice", "0011223344556677"] {
            assert!(!combined.contains(secret), "package leaked {secret}");
        }

        fs::remove_dir_all(directory).ok();
    }
}
