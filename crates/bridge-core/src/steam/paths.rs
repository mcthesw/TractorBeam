use std::{
    cmp::Reverse,
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
};

use steamlocate::SteamDir;

use super::{
    SteamAccount,
    vdf::{dedup_paths, parse_loginusers},
};

/// Steam app id for The Binding of Isaac: Rebirth.
pub const ISAAC_APP_ID: u32 = 250_900;

/// Default Steam install directory name for The Binding of Isaac: Rebirth.
pub const ISAAC_DEFAULT_INSTALL_DIR: &str = "The Binding of Isaac Rebirth";

/// Returns the Steam URI used to ask Steam to launch Isaac.
#[must_use]
pub fn isaac_launch_uri() -> String {
    format!("steam://rungameid/{ISAAC_APP_ID}")
}

/// Asks Steam to launch Isaac through the platform shell integration.
pub fn launch_isaac() -> io::Result<()> {
    open::that_detached(isaac_launch_uri())
}

/// Returns likely `loginusers.vdf` paths on Windows.
#[must_use]
pub fn loginusers_candidates() -> Vec<PathBuf> {
    steam_install_candidates()
        .into_iter()
        .map(|path| path.join("config").join("loginusers.vdf"))
        .collect()
}

/// Returns likely Steam installation directories.
#[must_use]
pub fn steam_install_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = env::var_os("STEAM_DIR") {
        paths.push(PathBuf::from(path));
    }
    if let Some(path) = current_user_steam_dir() {
        paths.push(path);
    }
    if let Ok(steam_dirs) = steamlocate::locate_all() {
        paths.extend(
            steam_dirs
                .into_iter()
                .map(|steam_dir| steam_dir.path().to_path_buf()),
        );
    }
    if let Some(path) = env::var_os("ProgramFiles(x86)") {
        paths.push(PathBuf::from(path).join("Steam"));
    }
    if let Some(path) = env::var_os("ProgramFiles") {
        paths.push(PathBuf::from(path).join("Steam"));
    }
    dedup_paths(paths)
}

/// Returns likely Steam library root directories.
#[must_use]
pub fn steam_library_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for steam_dir in steam_install_candidates() {
        paths.push(steam_dir.clone());
        if let Ok(steam_dir) = SteamDir::from_dir(&steam_dir)
            && let Ok(libraries) = steam_dir.library_paths()
        {
            paths.extend(libraries);
        }
    }
    dedup_paths(paths)
}

/// Returns likely Isaac installation directories.
#[must_use]
pub fn isaac_install_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for steam_path in steam_install_candidates() {
        if let Ok(steam_dir) = SteamDir::from_dir(&steam_path)
            && let Ok(Some((app, library))) = steam_dir.find_app(ISAAC_APP_ID)
        {
            paths.push(library.resolve_app_dir(&app));
        }
    }
    for library in steam_library_candidates() {
        let steamapps = library.join("steamapps");
        paths.push(steamapps.join("common").join(ISAAC_DEFAULT_INSTALL_DIR));
    }
    dedup_paths(paths)
}

/// Detects Steam accounts from the local Steam configuration.
#[must_use]
pub fn detect_accounts() -> Vec<SteamAccount> {
    let mut accounts = BTreeMap::new();
    for path in loginusers_candidates() {
        for account in read_loginusers(&path) {
            accounts.insert(account.steam_id64.clone(), account);
        }
    }
    let mut values = accounts.into_values().collect::<Vec<_>>();
    values.sort_by_key(|account| Reverse(account.most_recent));
    values
}

fn read_loginusers(path: &Path) -> Vec<SteamAccount> {
    fs::read_to_string(path)
        .map(|contents| parse_loginusers(&contents))
        .unwrap_or_default()
}

#[cfg(windows)]
fn current_user_steam_dir() -> Option<PathBuf> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Valve\\Steam")
        .ok()?;
    let path = key.get_value::<String, _>("SteamPath").ok()?;
    (!path.trim().is_empty()).then(|| PathBuf::from(path))
}

#[cfg(not(windows))]
fn current_user_steam_dir() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_launch_uri() {
        assert_eq!(isaac_launch_uri(), "steam://rungameid/250900");
    }
}
