//! Diagnostics redaction and known Isaac log paths.

use std::path::PathBuf;

use directories::UserDirs;
use regex::Regex;

pub const ONLINE_LOG: &str = "online.log";
pub const BRIDGE_CONFIG_FILE: &str = "isaac_bridge_config.txt";
pub const BRIDGE_HOOK_LOG: &str = "basement_bridge_hook.log";
const MAX_FILE_EXCERPT_BYTES: usize = 64 * 1024;

#[must_use]
pub fn primary_diagnostic_files() -> &'static [&'static str] {
    &[ONLINE_LOG, BRIDGE_CONFIG_FILE, BRIDGE_HOOK_LOG]
}

#[must_use]
pub fn isaac_online_logs_directory() -> PathBuf {
    UserDirs::new()
        .map(|dirs| {
            dirs.document_dir()
                .unwrap_or_else(|| dirs.home_dir())
                .join("My Games")
                .join("Binding of Isaac Repentance+")
                .join("online_logs")
        })
        .unwrap_or_else(|| std::env::temp_dir().join("basement-bridge-online-logs"))
}

#[must_use]
pub fn file_excerpt(input: &str) -> &str {
    if input.len() <= MAX_FILE_EXCERPT_BYTES {
        return input;
    }

    let mut start = input.len() - MAX_FILE_EXCERPT_BYTES;
    while start < input.len() && !input.is_char_boundary(start) {
        start += 1;
    }
    &input[start..]
}

#[must_use]
pub fn redact_text(input: &str) -> String {
    let mut output = input.to_owned();
    for pattern in sensitive_patterns() {
        output = pattern.replace_all(&output, "[redacted]").into_owned();
    }
    output
}

fn sensitive_patterns() -> Vec<Regex> {
    [
        r"(?i)\b\d{17}\b",
        r"(?i)(room|token|password|secret)\s*[:=]\s*[^\s]+",
        r"(?i)\broom\s+[^\s]+",
        r"(?i)\b(?:\d{1,3}\.){3}\d{1,3}:\d{2,5}\b",
        r"(?i)[A-Z]:\\Users\\[^\\\s]+",
        r"(?i)/home/[^/\s]+",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("redaction regex compiles"))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_primary_diagnostics() {
        assert_eq!(
            primary_diagnostic_files(),
            &[ONLINE_LOG, BRIDGE_CONFIG_FILE, BRIDGE_HOOK_LOG]
        );
    }

    #[test]
    fn redacts_known_sensitive_fields() {
        let text = "room=abc token=secret 76561198000000001 203.0.113.10:25910";
        let redacted = redact_text(text);

        assert!(!redacted.contains("abc"));
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("76561198000000001"));
        assert!(!redacted.contains("203.0.113.10"));
    }

    #[test]
    fn redacts_user_paths_with_either_separator() {
        let text = r"C:\Users\alice\Documents and /home/bob/.steam";
        let redacted = redact_text(text);

        assert!(!redacted.contains("alice"));
        assert!(!redacted.contains("bob"));
    }
}
