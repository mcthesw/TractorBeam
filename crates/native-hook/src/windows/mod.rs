use std::{ffi::c_void, ptr, thread, time::Duration};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HINSTANCE},
    System::{
        LibraryLoader::{DisableThreadLibraryCalls, GetModuleHandleW},
        Threading::{CreateThread, Sleep},
    },
};

mod bridge;
mod iat;
mod steam;

const DLL_PROCESS_DETACH: u32 = 0;
const DLL_PROCESS_ATTACH: u32 = 1;

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    module: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    match reason {
        DLL_PROCESS_ATTACH => unsafe {
            DisableThreadLibraryCalls(module);
            let thread = CreateThread(
                ptr::null(),
                0,
                Some(install_thread),
                ptr::null(),
                0,
                ptr::null_mut(),
            );
            if !thread.is_null() {
                CloseHandle(thread);
            }
        },
        DLL_PROCESS_DETACH => {
            bridge::shutdown();
        }
        _ => {}
    }
    1
}

unsafe extern "system" fn install_thread(_parameter: *mut c_void) -> u32 {
    for _ in 0..300 {
        if unsafe { GetModuleHandleW(wide_null("steam_api.dll").as_ptr()) }.is_null() {
            unsafe { Sleep(100) };
        } else {
            break;
        }
    }

    bridge::initialize();
    unsafe {
        steam::install_hooks();
    }

    thread::sleep(Duration::from_millis(1));
    0
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
