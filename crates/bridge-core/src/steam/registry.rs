use std::path::PathBuf;

#[cfg(windows)]
pub(super) fn registry_steam_dirs() -> Vec<PathBuf> {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    };

    let mut paths = Vec::new();
    if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Valve\\Steam") {
        push_registry_path(&key, "SteamPath", &mut paths);
    }
    let machine = RegKey::predef(HKEY_LOCAL_MACHINE);
    for subkey in [
        "SOFTWARE\\WOW6432Node\\Valve\\Steam",
        "SOFTWARE\\Valve\\Steam",
    ] {
        if let Ok(key) = machine.open_subkey(subkey) {
            push_registry_path(&key, "InstallPath", &mut paths);
        }
    }
    paths
}

#[cfg(windows)]
fn push_registry_path(key: &winreg::RegKey, name: &str, paths: &mut Vec<PathBuf>) {
    if let Ok(value) = key.get_value::<String, _>(name)
        && !value.trim().is_empty()
    {
        paths.push(PathBuf::from(value));
    }
}

#[cfg(not(windows))]
pub(super) fn registry_steam_dirs() -> Vec<PathBuf> {
    Vec::new()
}
