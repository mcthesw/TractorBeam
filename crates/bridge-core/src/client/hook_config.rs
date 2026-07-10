use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::{SessionConfig, SessionMode, hook_ipc::HookIpcSession};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HookConfigWrite {
    pub(super) path: PathBuf,
}

pub(super) fn write_hook_config(
    config: &SessionConfig,
    paths: &tractor_beam_isaac_injector::NativeHookPaths,
    ipc: &HookIpcSession,
) -> io::Result<HookConfigWrite> {
    let path = hook_config_path(&paths.hook)?;
    let directory = path.parent().ok_or_else(|| {
        io::Error::other("Native Hook launch parameter path has no parent directory")
    })?;
    fs::create_dir_all(directory)?;
    let fallback_to_steam = u8::from(config.mode == SessionMode::Fallback);
    let contents = hook_config_contents(fallback_to_steam, ipc);
    fs::write(&path, contents)?;
    Ok(HookConfigWrite { path })
}

fn hook_config_contents(fallback_to_steam: u8, ipc: &HookIpcSession) -> String {
    format!(
        "mode=replace\nfallback_to_steam={fallback_to_steam}\nipc_endpoint={}\nipc_session={}\n",
        ipc.endpoint,
        ipc.session_id.to_hex()
    )
}

fn hook_config_path(hook: &Path) -> io::Result<PathBuf> {
    let Some(directory) = hook.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Native Hook path has no parent directory: {}",
                hook.display()
            ),
        ));
    };
    Ok(directory.join(crate::diagnostics::BRIDGE_CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_config_path_uses_native_hook_directory() {
        let hook = Path::new("bundle")
            .join("native-hook")
            .join("tractor_beam_native_hook.dll");

        let path = hook_config_path(&hook).unwrap();

        assert_eq!(
            path,
            Path::new("bundle")
                .join("native-hook")
                .join(crate::diagnostics::BRIDGE_CONFIG_FILE)
        );
    }

    #[test]
    fn hook_config_contents_includes_local_ipc_identity() {
        let ipc = HookIpcSession::test();
        let contents = hook_config_contents(1, &ipc);

        assert!(contents.contains(&format!("ipc_endpoint={}\n", ipc.endpoint)));
        assert!(contents.contains(&format!("ipc_session={}\n", ipc.session_id.to_hex())));
        assert!(!contents.contains("sidecar="));
        assert!(!contents.contains("control="));
    }
}
