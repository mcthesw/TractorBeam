use std::{
    env,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{self, Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{InjectionStep, InjectorError};

#[cfg(windows)]
mod windows_elevation;

/// Rust Native Hook DLL name expected in the Client Bundle.
pub const NATIVE_HOOK_DLL: &str = "tractor_beam_native_hook.dll";

/// Rust Injector helper executable name expected in the Client Bundle.
#[cfg(windows)]
pub const NATIVE_INJECTOR_EXE: &str = "tractor-beam-isaac-injector.exe";
#[cfg(not(windows))]
pub const NATIVE_INJECTOR_EXE: &str = "tractor-beam-isaac-injector";
const INJECTOR_POLL_INTERVAL: Duration = Duration::from_millis(10);
const ACTIVE_GUARD: &[u8] = b"tractor-beam-injection-active-v1";
static NEXT_GUARD_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeHookPaths {
    pub injector: PathBuf,
    pub hook: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InjectorLaunchEvent {
    ElevatedRetryStarting,
    ElevatedRetrySucceeded,
}

#[derive(Clone, Debug)]
pub struct InjectionGuard {
    inner: Arc<InjectionGuardInner>,
}

#[derive(Debug)]
struct InjectionGuardInner {
    path: PathBuf,
    cancelled: AtomicBool,
}

impl InjectionGuard {
    pub fn create() -> Result<Self, InjectorError> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = NEXT_GUARD_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "tractor-beam-injection-{}-{nonce}-{id}.guard",
            process::id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| InjectorError::step_io(InjectionStep::HelperProcess, error))?;
        std::io::Write::write_all(&mut file, ACTIVE_GUARD)
            .map_err(|error| InjectorError::step_io(InjectionStep::HelperProcess, error))?;
        Ok(Self {
            inner: Arc::new(InjectionGuardInner {
                path,
                cancelled: AtomicBool::new(false),
            }),
        })
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        let _ = fs::write(&self.inner.path, b"cancelled");
        let _ = fs::remove_file(&self.inner.path);
    }

    #[must_use]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        &self.inner.path
    }
}

impl Drop for InjectionGuardInner {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn injection_guard_active(path: &Path) -> bool {
    fs::read(path).is_ok_and(|contents| contents == ACTIVE_GUARD)
}

pub fn resolve_native_hook_paths() -> Result<NativeHookPaths, InjectorError> {
    let directories = bundle_search_dirs();
    resolve_native_hook_paths_in(&directories)
}

fn resolve_native_hook_paths_in(directories: &[PathBuf]) -> Result<NativeHookPaths, InjectorError> {
    let injector =
        find_file(directories, NATIVE_INJECTOR_EXE).ok_or(InjectorError::MissingInjector)?;
    let hook = find_file(directories, NATIVE_HOOK_DLL).ok_or(InjectorError::MissingNativeHook)?;
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

fn guarded_injector_args(pid: u32, dll_path: &Path, guard_path: &Path) -> [OsString; 6] {
    [
        "--pid".into(),
        pid.to_string().into(),
        "--dll".into(),
        dll_path.as_os_str().to_owned(),
        "--guard-file".into(),
        guard_path.as_os_str().to_owned(),
    ]
}

#[cfg(windows)]
fn elevated_injector_args(
    pid: u32,
    dll_path: &Path,
    guard_path: &Path,
    result_path: &Path,
) -> [OsString; 8] {
    [
        "--pid".into(),
        pid.to_string().into(),
        "--dll".into(),
        dll_path.as_os_str().to_owned(),
        "--guard-file".into(),
        guard_path.as_os_str().to_owned(),
        "--result-file".into(),
        result_path.as_os_str().to_owned(),
    ]
}

pub fn run_injector(paths: &NativeHookPaths, pid: u32) -> Result<(), InjectorError> {
    let output = Command::new(&paths.injector)
        .args(injector_args(pid, &paths.hook))
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(InjectorError::injection(
            InjectionStep::HelperProcess,
            injector_failure_message(output.status, &output.stderr),
        ))
    }
}

pub fn run_injector_with_elevated_retry(
    paths: &NativeHookPaths,
    pid: u32,
    guard: &InjectionGuard,
    observer: impl FnMut(InjectorLaunchEvent),
) -> Result<(), InjectorError> {
    run_injector_with_elevated_retry_impl(
        || run_guarded_injector(paths, pid, guard),
        || run_elevated_injector(paths, pid, guard),
        observer,
    )
}

fn run_guarded_injector(
    paths: &NativeHookPaths,
    pid: u32,
    guard: &InjectionGuard,
) -> Result<(), InjectorError> {
    if guard.is_cancelled() {
        return Err(InjectorError::InjectionCancelled);
    }
    let child = Command::new(&paths.injector)
        .args(guarded_injector_args(pid, &paths.hook, guard.path()))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    wait_for_guarded_child(child, guard)
}

fn wait_for_guarded_child(mut child: Child, guard: &InjectionGuard) -> Result<(), InjectorError> {
    let status = loop {
        if guard.is_cancelled() {
            stop_child(&mut child);
            return Err(InjectorError::InjectionCancelled);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(INJECTOR_POLL_INTERVAL);
    };
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr)?;
    }
    if status.success() {
        Ok(())
    } else {
        Err(InjectorError::injection(
            InjectionStep::HelperProcess,
            injector_failure_message(status, &stderr),
        ))
    }
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn run_injector_with_elevated_retry_impl(
    normal: impl FnOnce() -> Result<(), InjectorError>,
    elevated: impl FnOnce() -> Result<(), InjectorError>,
    mut observer: impl FnMut(InjectorLaunchEvent),
) -> Result<(), InjectorError> {
    match normal() {
        Ok(()) => Ok(()),
        Err(error) if error.is_access_denied() => {
            observer(InjectorLaunchEvent::ElevatedRetryStarting);
            elevated()?;
            observer(InjectorLaunchEvent::ElevatedRetrySucceeded);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn run_elevated_injector(
    paths: &NativeHookPaths,
    pid: u32,
    guard: &InjectionGuard,
) -> Result<(), InjectorError> {
    windows_elevation::run_elevated_injector(paths, pid, guard)
}

#[cfg(not(windows))]
fn run_elevated_injector(
    _paths: &NativeHookPaths,
    _pid: u32,
    _guard: &InjectionGuard,
) -> Result<(), InjectorError> {
    Err(InjectorError::UnsupportedPlatform)
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
    if let Some(path) = env::var_os("TRACTOR_BEAM_BUNDLE_DIR") {
        directories.push(PathBuf::from(path));
    }
    if let Ok(exe) = env::current_exe()
        && let Some(directory) = exe.parent()
    {
        directories.push(directory.to_path_buf());
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
                .join("target")
                .join("i686-pc-windows-gnullvm")
                .join("debug"),
        );
        directories.push(
            directory
                .join("target")
                .join("i686-pc-windows-gnullvm")
                .join("release"),
        );
    }
    directories.sort();
    directories.dedup();
    directories
}

fn find_file(directories: &[PathBuf], name: &str) -> Option<PathBuf> {
    directories
        .iter()
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use std::fs;

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
    fn cancelling_guard_revokes_the_cross_process_permit() {
        let guard = InjectionGuard::create().expect("create injection guard");
        assert!(injection_guard_active(guard.path()));

        guard.cancel();

        assert!(guard.is_cancelled());
        assert!(!injection_guard_active(guard.path()));
    }

    #[test]
    fn bundle_search_dirs_are_unique() {
        let directories = bundle_search_dirs();
        let mut sorted = directories.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(directories, sorted);
    }

    #[test]
    fn bundle_search_dirs_do_not_include_prototype_build_outputs() {
        let directories = bundle_search_dirs();
        assert!(
            directories.iter().all(|directory| !directory
                .components()
                .any(|component| component.as_os_str() == "prototype")),
            "prototype directories must not be searched: {directories:?}"
        );
    }

    #[test]
    fn resolve_native_hook_paths_ignores_legacy_file_names() {
        let directory = tempfile::tempdir().expect("create test directory");
        let directory_path = directory.path().to_path_buf();
        fs::write(directory_path.join("eos_probe_injector.exe"), [])
            .expect("write legacy injector fixture");
        fs::write(directory_path.join("isaac_eos_probe.dll"), [])
            .expect("write legacy hook fixture");

        assert!(matches!(
            resolve_native_hook_paths_in(std::slice::from_ref(&directory_path)),
            Err(InjectorError::MissingInjector)
        ));

        let injector = directory_path.join(NATIVE_INJECTOR_EXE);
        fs::write(&injector, []).expect("write injector fixture");
        assert!(matches!(
            resolve_native_hook_paths_in(std::slice::from_ref(&directory_path)),
            Err(InjectorError::MissingNativeHook)
        ));

        let hook = directory_path.join(NATIVE_HOOK_DLL);
        fs::write(&hook, []).expect("write native hook fixture");
        assert_eq!(
            resolve_native_hook_paths_in(std::slice::from_ref(&directory_path))
                .expect("new native hook paths should resolve"),
            NativeHookPaths { injector, hook }
        );
    }

    #[test]
    fn elevated_retry_runs_after_access_denied() {
        let mut events = Vec::new();

        let result = run_injector_with_elevated_retry_impl(
            || {
                Err(InjectorError::injection(
                    InjectionStep::HelperProcess,
                    "open Isaac process: 拒绝访问。 (os error 5)",
                ))
            },
            || Ok(()),
            |event| events.push(event),
        );

        assert!(result.is_ok());
        assert_eq!(
            events,
            [
                InjectorLaunchEvent::ElevatedRetryStarting,
                InjectorLaunchEvent::ElevatedRetrySucceeded
            ]
        );
    }

    #[test]
    fn elevated_retry_does_not_run_after_non_access_denied_failure() {
        let result = run_injector_with_elevated_retry_impl(
            || {
                Err(InjectorError::injection(
                    InjectionStep::HelperProcess,
                    "LoadLibraryW returned null",
                ))
            },
            || panic!("elevated retry should not run"),
            |_| panic!("retry event should not be emitted"),
        );

        assert!(matches!(
            result,
            Err(InjectorError::Injection {
                step: InjectionStep::HelperProcess,
                ..
            })
        ));
    }

    #[test]
    fn elevated_retry_returns_cancellation_error() {
        let result = run_injector_with_elevated_retry_impl(
            || {
                Err(InjectorError::injection(
                    InjectionStep::HelperProcess,
                    "access denied",
                ))
            },
            || Err(InjectorError::AdminPermissionCancelled),
            |_| {},
        );

        assert!(matches!(
            result,
            Err(InjectorError::AdminPermissionCancelled)
        ));
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
