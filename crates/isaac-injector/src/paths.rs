use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use crate::InjectorError;

/// Rust Native Hook DLL name expected in the Client Bundle.
pub const NATIVE_HOOK_DLL: &str = "basement_native_hook.dll";

/// Rust Injector helper executable name expected in the Client Bundle.
pub const NATIVE_INJECTOR_EXE: &str = "basement-isaac-injector.exe";

/// Legacy prototype DLL name accepted during the Rust migration.
pub const LEGACY_NATIVE_HOOK_DLL: &str = "isaac_eos_probe.dll";

/// Legacy prototype Injector executable accepted during the Rust migration.
pub const LEGACY_NATIVE_INJECTOR_EXE: &str = "eos_probe_injector.exe";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeHookPaths {
    pub injector: PathBuf,
    pub hook: PathBuf,
}

pub fn resolve_native_hook_paths() -> Result<NativeHookPaths, InjectorError> {
    let directories = bundle_search_dirs();
    let injector = find_file(
        &directories,
        &[NATIVE_INJECTOR_EXE, LEGACY_NATIVE_INJECTOR_EXE],
    )
    .ok_or(InjectorError::MissingInjector)?;
    let hook = find_file(&directories, &[NATIVE_HOOK_DLL, LEGACY_NATIVE_HOOK_DLL])
        .ok_or(InjectorError::MissingNativeHook)?;
    Ok(NativeHookPaths { injector, hook })
}

#[must_use]
pub fn injector_args(pid: u32, dll_path: &Path) -> [OsString; 4] {
    [
        "--pid".into(),
        pid.to_string().into(),
        "--dll".into(),
        dll_path.as_os_str().to_owned(),
    ]
}

pub fn run_injector(paths: &NativeHookPaths, pid: u32) -> Result<(), InjectorError> {
    let output = Command::new(&paths.injector)
        .args(injector_args(pid, &paths.hook))
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(InjectorError::Injection(injector_failure_message(
            output.status,
            &output.stderr,
        )))
    }
}

fn injector_failure_message(status: ExitStatus, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("injector helper exited with {status}")
    } else {
        format!("injector helper exited with {status}: {stderr}")
    }
}

fn bundle_search_dirs() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(path) = env::var_os("BASEMENT_BRIDGE_BUNDLE_DIR") {
        directories.push(PathBuf::from(path));
    }
    if let Ok(exe) = env::current_exe()
        && let Some(directory) = exe.parent()
    {
        directories.push(directory.to_path_buf());
        directories.push(directory.join("native-hook"));
    }
    if let Ok(directory) = env::current_dir() {
        directories.push(directory.join("target").join("debug"));
        directories.push(directory.join("target").join("release"));
        directories.push(
            directory
                .join("target")
                .join("i686-pc-windows-msvc")
                .join("debug"),
        );
        directories.push(
            directory
                .join("target")
                .join("i686-pc-windows-msvc")
                .join("release"),
        );
        directories.push(
            directory
                .join("prototype")
                .join("native-hook")
                .join("build")
                .join("x86-clang-rel"),
        );
    }
    directories.sort();
    directories.dedup();
    directories
}

fn find_file(directories: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    directories
        .iter()
        .flat_map(|directory| names.iter().map(|name| directory.join(name)))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_injector_args() {
        assert_eq!(
            injector_args(42, Path::new("hook.dll")),
            [
                OsString::from("--pid"),
                OsString::from("42"),
                OsString::from("--dll"),
                OsString::from("hook.dll")
            ]
        );
    }

    #[test]
    fn bundle_search_dirs_are_unique() {
        let directories = bundle_search_dirs();
        let mut sorted = directories.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(directories, sorted);
    }

    #[cfg(windows)]
    #[test]
    fn includes_stderr_in_injector_failure() {
        let status = Command::new("cmd")
            .args(["/C", "exit 1"])
            .status()
            .expect("cmd should be available on Windows");

        assert!(
            injector_failure_message(status, b"LoadLibraryW returned null")
                .contains("LoadLibraryW returned null")
        );
    }
}
