use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use crate::{InjectionStep, InjectorError};

/// Rust Native Hook DLL name expected in the Client Bundle.
pub const NATIVE_HOOK_DLL: &str = "basement_native_hook.dll";

/// Rust Injector helper executable name expected in the Client Bundle.
pub const NATIVE_INJECTOR_EXE: &str = "basement-isaac-injector.exe";

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
    observer: impl FnMut(InjectorLaunchEvent),
) -> Result<(), InjectorError> {
    run_injector_with_elevated_retry_impl(
        || run_injector(paths, pid),
        || run_elevated_injector(paths, pid),
        observer,
    )
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
fn run_elevated_injector(paths: &NativeHookPaths, pid: u32) -> Result<(), InjectorError> {
    windows_elevation::run_elevated_injector(paths, pid)
}

#[cfg(not(windows))]
fn run_elevated_injector(_paths: &NativeHookPaths, _pid: u32) -> Result<(), InjectorError> {
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

#[cfg(windows)]
mod windows_elevation {
    use std::{
        ffi::{OsStr, OsString},
        io, iter,
        os::windows::ffi::{OsStrExt, OsStringExt},
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
        System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject},
        UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
    };

    use super::{NativeHookPaths, injector_args};
    use crate::InjectorError;

    const SW_SHOWNORMAL: i32 = 1;

    pub(super) fn run_elevated_injector(
        paths: &NativeHookPaths,
        pid: u32,
    ) -> Result<(), InjectorError> {
        let verb = wide_null(OsStr::new("runas"));
        let file = wide_null(paths.injector.as_os_str());
        let args = injector_args(pid, &paths.hook);
        let parameters = shell_parameters(&args);
        let parameters = wide_null(&parameters);

        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: verb.as_ptr(),
            lpFile: file.as_ptr(),
            lpParameters: parameters.as_ptr(),
            nShow: SW_SHOWNORMAL,
            ..unsafe { std::mem::zeroed() }
        };

        // ShellExecuteExW reads the UTF-16 pointers during the call and returns
        // a process handle when SEE_MASK_NOCLOSEPROCESS succeeds.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
                return Err(InjectorError::AdminPermissionCancelled);
            }
            return Err(InjectorError::elevated_retry_failed(format!(
                "could not launch elevated injector helper: {error}"
            )));
        }

        let process = process_handle(&info);
        let Some(process) = OwnedHandle::new(process) else {
            return Err(InjectorError::elevated_retry_failed(
                "elevated injector helper did not return a process handle",
            ));
        };

        wait_for_process(&process)
    }

    fn wait_for_process(process: &OwnedHandle) -> Result<(), InjectorError> {
        match unsafe { WaitForSingleObject(process.raw(), INFINITE) } {
            WAIT_OBJECT_0 => {}
            WAIT_FAILED => {
                return Err(InjectorError::elevated_retry_failed(format!(
                    "could not wait for elevated injector helper: {}",
                    io::Error::last_os_error()
                )));
            }
            result => {
                return Err(InjectorError::elevated_retry_failed(format!(
                    "unexpected wait result {result} from elevated injector helper"
                )));
            }
        }

        let mut exit_code = 0;
        if unsafe { GetExitCodeProcess(process.raw(), &mut exit_code) } == 0 {
            return Err(InjectorError::elevated_retry_failed(format!(
                "could not read elevated injector helper exit code: {}",
                io::Error::last_os_error()
            )));
        }
        if exit_code == 0 {
            Ok(())
        } else {
            Err(InjectorError::elevated_retry_failed(format!(
                "elevated injector helper exited with exit code {exit_code}"
            )))
        }
    }

    fn process_handle(info: &SHELLEXECUTEINFOW) -> HANDLE {
        #[cfg(target_arch = "x86")]
        {
            unsafe { std::ptr::addr_of!(info.hProcess).read_unaligned() }
        }
        #[cfg(not(target_arch = "x86"))]
        {
            info.hProcess
        }
    }

    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        fn new(handle: HANDLE) -> Option<Self> {
            (!handle.is_null()).then_some(Self(handle))
        }

        fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn shell_parameters(args: &[OsString]) -> OsString {
        let mut output = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            if index > 0 {
                output.push(b' ' as u16);
            }
            append_quoted_argument(arg.as_os_str(), &mut output);
        }
        OsString::from_wide(&output)
    }

    fn append_quoted_argument(argument: &OsStr, output: &mut Vec<u16>) {
        let argument: Vec<u16> = argument.encode_wide().collect();
        let needs_quotes = argument.is_empty()
            || argument.iter().any(|character| {
                *character == b' ' as u16 || *character == b'\t' as u16 || *character == b'"' as u16
            });
        if !needs_quotes {
            output.extend(argument);
            return;
        }

        output.push(b'"' as u16);
        let mut backslashes = 0;
        for character in argument {
            if character == b'\\' as u16 {
                backslashes += 1;
            } else if character == b'"' as u16 {
                output.extend(iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
                output.push(character);
                backslashes = 0;
            } else {
                output.extend(iter::repeat_n(b'\\' as u16, backslashes));
                output.push(character);
                backslashes = 0;
            }
        }
        output.extend(iter::repeat_n(b'\\' as u16, backslashes * 2));
        output.push(b'"' as u16);
    }

    fn wide_null(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(iter::once(0)).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn shell_parameters_quote_paths_with_spaces() {
            let args = [
                OsString::from("--pid"),
                OsString::from("42"),
                OsString::from("--dll"),
                OsString::from(r"C:\Program Files\Bridge\hook.dll"),
            ];

            assert_eq!(
                shell_parameters(&args).to_string_lossy(),
                r#"--pid 42 --dll "C:\Program Files\Bridge\hook.dll""#
            );
        }

        #[test]
        fn shell_parameters_escape_quotes_and_trailing_slashes() {
            let args = [OsString::from(r#"C:\Bridge "Test"\"#)];

            assert_eq!(
                shell_parameters(&args).to_string_lossy(),
                r#""C:\Bridge \"Test\"\\""#
            );
        }
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
    use std::{
        fs, process,
        time::{SystemTime, UNIX_EPOCH},
    };

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
        let directory = TempTestDir::new("legacy-native-names");
        fs::write(directory.path.join("eos_probe_injector.exe"), [])
            .expect("write legacy injector fixture");
        fs::write(directory.path.join("isaac_eos_probe.dll"), [])
            .expect("write legacy hook fixture");

        assert!(matches!(
            resolve_native_hook_paths_in(std::slice::from_ref(&directory.path)),
            Err(InjectorError::MissingInjector)
        ));

        let injector = directory.path.join(NATIVE_INJECTOR_EXE);
        fs::write(&injector, []).expect("write injector fixture");
        assert!(matches!(
            resolve_native_hook_paths_in(std::slice::from_ref(&directory.path)),
            Err(InjectorError::MissingNativeHook)
        ));

        let hook = directory.path.join(NATIVE_HOOK_DLL);
        fs::write(&hook, []).expect("write native hook fixture");
        assert_eq!(
            resolve_native_hook_paths_in(std::slice::from_ref(&directory.path))
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

    struct TempTestDir {
        path: PathBuf,
    }

    impl TempTestDir {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "basement-isaac-injector-{name}-{}-{nonce}",
                process::id()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self { path }
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
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
