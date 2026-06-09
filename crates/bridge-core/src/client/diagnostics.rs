use std::{fs, io, path::PathBuf};

use directories::ProjectDirs;

use super::{BridgeClient, PRODUCT_NAME, state::unix_seconds};

impl BridgeClient {
    pub fn export_diagnostics(&mut self) -> io::Result<PathBuf> {
        let directory = diagnostics_directory();
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("basement-bridge-{}.txt", unix_seconds()));
        fs::write(&path, self.redacted_diagnostics_text())?;
        self.log(
            super::LogLevel::Info,
            format!("Diagnostics exported to {}", path.display()),
        );
        Ok(path)
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
        output.push_str(&format!("errors: {}\n\n", self.state.counters.errors));
        output.push_str("primary files:\n");
        for file in crate::diagnostics::primary_diagnostic_files() {
            output.push_str("- ");
            output.push_str(file);
            output.push('\n');
        }
        output.push_str("\nprimary file excerpts:\n");
        let log_directory = crate::diagnostics::isaac_online_logs_directory();
        output.push_str(&format!("directory: {}\n", log_directory.display()));
        for file in crate::diagnostics::primary_diagnostic_files() {
            let path = log_directory.join(file);
            output.push_str("\n--- ");
            output.push_str(file);
            output.push_str(" ---\n");
            match fs::read_to_string(&path) {
                Ok(contents) => output.push_str(crate::diagnostics::file_excerpt(&contents)),
                Err(error) => output.push_str(&format!("unavailable: {error}\n")),
            }
            if !output.ends_with('\n') {
                output.push('\n');
            }
        }
        output.push_str("\nlogs:\n");
        for entry in &self.state.logs {
            output.push_str(&format!(
                "[{}] {} {}\n",
                entry.timestamp, entry.level, entry.message
            ));
        }
        output
    }
}

#[must_use]
pub fn diagnostics_directory() -> PathBuf {
    ProjectDirs::from("io.github", "mcthesw", PRODUCT_NAME)
        .map(|project| project.data_local_dir().join("diagnostics"))
        .unwrap_or_else(|| std::env::temp_dir().join(PRODUCT_NAME).join("diagnostics"))
}
