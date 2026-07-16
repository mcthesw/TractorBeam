//! Diagnostics redaction and known Isaac log paths.

use std::{
    fs,
    path::{Path, PathBuf},
};

use directories::UserDirs;
use regex::Regex;

pub const ONLINE_LOG: &str = "online.log";
pub const HOOK_RUNTIME_FILE: &str = "hook-runtime.txt";
pub const DAILY_LOG_RETAIN_COUNT: usize = 10;
pub const MAX_ISAAC_EXCERPT_BYTES: u64 = 64 * 1024;

#[must_use]
pub fn daily_log_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = all_daily_log_files(directory);
    files.truncate(DAILY_LOG_RETAIN_COUNT);
    files
}

pub fn prune_daily_logs(directory: &Path) -> std::io::Result<()> {
    for path in all_daily_log_files(directory)
        .into_iter()
        .skip(DAILY_LOG_RETAIN_COUNT)
    {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn all_daily_log_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(directory)
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_daily_log_name)
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    files
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
        .unwrap_or_else(|| std::env::temp_dir().join("tractor-beam-online-logs"))
}

#[must_use]
pub fn redact_text(input: &str) -> String {
    let mut output = input.to_owned();
    for pattern in sensitive_patterns() {
        output = pattern.replace_all(&output, "[redacted]").into_owned();
    }
    output
}

fn is_daily_log_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 14
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && &bytes[10..] == b".log"
}

fn sensitive_patterns() -> Vec<Regex> {
    [
        r"(?i)\b\d{17}\b",
        r"(?i)(room|token|password|secret|session_credential|resume_credential|path_token|connection_id)\s*[:=]\s*[^\s]+",
        r"(?i)(ipc_session|ipc_endpoint)\s*[:=]\s*[^\s]+",
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
    fn redacts_known_sensitive_fields() {
        let text = "room=abc token=secret ipc_session=00112233445566778899aabbccddeeff ipc_endpoint=tractor-beam-00112233445566778899aabbccddeeff 76561198000000001 203.0.113.10:25910";
        let redacted = redact_text(text);

        assert!(!redacted.contains("abc"));
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("76561198000000001"));
        assert!(!redacted.contains("203.0.113.10"));
        assert!(!redacted.contains("00112233445566778899aabbccddeeff"));
    }

    #[test]
    fn redacts_user_paths_with_either_separator() {
        let text = r"C:\Users\alice\Documents and /home/bob/.steam";
        let redacted = redact_text(text);

        assert!(!redacted.contains("alice"));
        assert!(!redacted.contains("bob"));
    }

    #[test]
    fn daily_logs_ignore_unrelated_files_and_keep_newest_ten() {
        let directory =
            std::env::temp_dir().join(format!("tractor-beam-daily-logs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        for day in 1..=12 {
            fs::write(directory.join(format!("2026-07-{day:02}.log")), []).unwrap();
        }
        fs::write(directory.join("hook-runtime.txt"), []).unwrap();

        prune_daily_logs(&directory).unwrap();
        let logs = all_daily_log_files(&directory);

        assert_eq!(logs.len(), 10);
        assert_eq!(logs[0].file_name().unwrap(), "2026-07-12.log");
        assert_eq!(logs[9].file_name().unwrap(), "2026-07-03.log");
        assert!(directory.join("hook-runtime.txt").exists());
        let _ = fs::remove_dir_all(directory);
    }
}
