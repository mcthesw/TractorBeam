#[cfg(all(windows, target_arch = "x86"))]
fn main() -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::{LibraryLoader::LoadLibraryW, Threading::Sleep};

    let mut arguments = std::env::args_os().skip(1);
    let steam_api = arguments.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing steam_api.dll path",
        )
    })?;
    let hook = arguments.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing Native Hook DLL path",
        )
    })?;
    if arguments.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unexpected argument",
        ));
    }

    let steam_module = unsafe { LoadLibraryW(wide_null(&steam_api).as_ptr()) };
    if steam_module.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let hook_module = unsafe { LoadLibraryW(wide_null(&hook).as_ptr()) };
    if hook_module.is_null() {
        unsafe { FreeLibrary(steam_module) };
        return Err(std::io::Error::last_os_error());
    }

    unsafe { Sleep(2_000) };
    unsafe {
        FreeLibrary(hook_module);
        FreeLibrary(steam_module);
    }
    Ok(())
}

#[cfg(all(windows, target_arch = "x86"))]
fn wide_null(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;

    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(not(all(windows, target_arch = "x86")))]
fn main() {
    eprintln!("hook_loader must be built for i686-pc-windows-msvc");
    std::process::exit(2);
}
