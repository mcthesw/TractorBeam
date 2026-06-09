use std::{fs, io, path::PathBuf};

use directories::UserDirs;

use super::{SessionConfig, SessionMode};

pub(super) const HOOK_IN: &str = "127.0.0.1:25900";
pub(super) const HOOK_OUT: &str = "127.0.0.1:25901";

pub(super) fn write_hook_config(config: &SessionConfig) -> io::Result<()> {
    let directory = hook_config_directory();
    fs::create_dir_all(&directory)?;
    let fallback_to_steam = u8::from(config.mode == SessionMode::Fallback);
    let contents = format!(
        "mode=replace\nfallback_to_steam={fallback_to_steam}\nsidecar={HOOK_IN}\nbind={HOOK_OUT}\n"
    );
    fs::write(
        directory.join(crate::diagnostics::BRIDGE_CONFIG_FILE),
        contents,
    )
}

fn hook_config_directory() -> PathBuf {
    UserDirs::new()
        .and_then(|dirs| dirs.document_dir().map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join("My Games")
        .join("Binding of Isaac Repentance+")
        .join("online_logs")
}
