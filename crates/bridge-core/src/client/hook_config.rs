use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::{SessionConfig, SessionMode};

pub(super) const HOOK_IN: &str = "127.0.0.1:25900";
pub(super) const HOOK_OUT: &str = "127.0.0.1:25901";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HookConfigWrite {
    pub(super) path: PathBuf,
}

pub(super) fn write_hook_config(
    config: &SessionConfig,
    paths: &basement_isaac_injector::NativeHookPaths,
) -> io::Result<HookConfigWrite> {
    let path = hook_config_path(&paths.hook)?;
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("Native Hook config path has no parent directory"))?;
    fs::create_dir_all(directory)?;
    let fallback_to_steam = u8::from(config.mode == SessionMode::Fallback);
    let contents = format!(
        "mode=replace\nfallback_to_steam={fallback_to_steam}\nsidecar={HOOK_IN}\nbind={HOOK_OUT}\n"
    );
    fs::write(&path, contents)?;
    Ok(HookConfigWrite { path })
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
            .join("basement_native_hook.dll");

        let path = hook_config_path(&hook).unwrap();

        assert_eq!(
            path,
            Path::new("bundle")
                .join("native-hook")
                .join(crate::diagnostics::BRIDGE_CONFIG_FILE)
        );
    }
}
