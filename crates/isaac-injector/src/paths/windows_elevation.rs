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
    let parameters = wide_null(&shell_parameters(&injector_args(pid, &paths.hook)));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            return Err(InjectorError::AdminPermissionCancelled);
        }
        return Err(InjectorError::elevated_retry_failed(format!(
            "could not launch elevated injector helper: {error}"
        )));
    }
    let Some(process) = OwnedHandle::new(process_handle(&info)) else {
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
        || argument
            .iter()
            .any(|character| matches!(*character, 32 | 9 | 34));
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
