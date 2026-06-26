use std::path::Path;

use crate::InjectorError;

pub fn inject(pid: u32, hook: &Path) -> Result<(), InjectorError> {
    inject_platform(pid, hook)
}

#[cfg(windows)]
fn inject_platform(pid: u32, hook: &Path) -> Result<(), InjectorError> {
    use std::{ffi::OsStr, io, iter, os::windows::ffi::OsStrExt, ptr};

    use crate::InjectionStep;

    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::{
            Diagnostics::Debug::WriteProcessMemory,
            LibraryLoader::{GetModuleHandleW, GetProcAddress},
            Memory::{MEM_COMMIT, MEM_RELEASE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx},
            Threading::{
                CreateRemoteThread, GetExitCodeThread, OpenProcess, PROCESS_CREATE_THREAD,
                PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
                WaitForSingleObject,
            },
        },
    };

    if !hook.is_file() {
        return Err(InjectorError::MissingNativeHook);
    }

    let hook = std::fs::canonicalize(hook)?;
    let hook_wide = wide_null(hook.as_os_str());
    let remote_bytes = hook_wide.len() * size_of::<u16>();

    unsafe {
        let process = OpenProcess(
            PROCESS_CREATE_THREAD
                | PROCESS_QUERY_INFORMATION
                | PROCESS_VM_OPERATION
                | PROCESS_VM_WRITE
                | PROCESS_VM_READ,
            0,
            pid,
        );
        if process.is_null() {
            return Err(InjectorError::step_io(
                InjectionStep::OpenProcess,
                io::Error::last_os_error(),
            ));
        }

        let remote_path = VirtualAllocEx(
            process,
            ptr::null(),
            remote_bytes,
            MEM_COMMIT,
            PAGE_READWRITE,
        );
        if remote_path.is_null() {
            let error = io::Error::last_os_error();
            CloseHandle(process);
            return Err(InjectorError::step_io(
                InjectionStep::AllocateRemoteMemory,
                error,
            ));
        }

        let wrote = WriteProcessMemory(
            process,
            remote_path,
            hook_wide.as_ptr().cast(),
            remote_bytes,
            ptr::null_mut(),
        );
        if wrote == 0 {
            let error = io::Error::last_os_error();
            VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
            CloseHandle(process);
            return Err(InjectorError::step_io(InjectionStep::WriteDllPath, error));
        }

        let kernel32 = GetModuleHandleW(wide_null(OsStr::new("kernel32.dll")).as_ptr());
        if kernel32.is_null() {
            let error = io::Error::last_os_error();
            VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
            CloseHandle(process);
            return Err(InjectorError::step_io(
                InjectionStep::ResolveLoadLibrary,
                error,
            ));
        }

        let Some(load_library) = GetProcAddress(kernel32, c"LoadLibraryW".as_ptr().cast()) else {
            VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
            CloseHandle(process);
            return Err(InjectorError::injection(
                InjectionStep::ResolveLoadLibrary,
                "GetProcAddress(LoadLibraryW) failed",
            ));
        };

        let thread = CreateRemoteThread(
            process,
            ptr::null(),
            0,
            Some(std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
            >(load_library)),
            remote_path,
            0,
            ptr::null_mut(),
        );
        if thread.is_null() {
            let error = io::Error::last_os_error();
            VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
            CloseHandle(process);
            return Err(InjectorError::step_io(
                InjectionStep::CreateRemoteThread,
                error,
            ));
        }

        if WaitForSingleObject(thread, 10_000) != WAIT_OBJECT_0 {
            CloseHandle(thread);
            VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
            CloseHandle(process);
            return Err(InjectorError::injection(
                InjectionStep::WaitForRemoteThread,
                "timed out waiting for remote LoadLibraryW",
            ));
        }

        let mut exit_code = 0;
        if GetExitCodeThread(thread, &mut exit_code) == 0 {
            let error = io::Error::last_os_error();
            CloseHandle(thread);
            VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
            CloseHandle(process);
            return Err(InjectorError::step_io(
                InjectionStep::ReadRemoteThreadExit,
                error,
            ));
        }

        CloseHandle(thread);
        VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
        CloseHandle(process);

        if exit_code == 0 {
            return Err(InjectorError::injection(
                InjectionStep::ReadRemoteThreadExit,
                "LoadLibraryW returned null in the Isaac process",
            ));
        }
    }

    fn wide_null(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(iter::once(0)).collect()
    }

    Ok(())
}

#[cfg(not(windows))]
fn inject_platform(_pid: u32, _hook: &Path) -> Result<(), InjectorError> {
    Err(InjectorError::UnsupportedPlatform)
}
