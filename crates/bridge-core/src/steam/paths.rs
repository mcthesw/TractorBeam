use std::{
    cmp::Reverse,
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
};

use super::{
    SteamAccount,
    registry::registry_steam_dirs,
    vdf::{dedup_paths, parse_appmanifest_install_dir, parse_libraryfolders, parse_loginusers},
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
    paths.extend(registry_steam_dirs());
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
        paths.extend(read_libraryfolders(&steam_dir));
    }
    dedup_paths(paths)
}

/// Returns likely Isaac installation directories.
#[must_use]
pub fn isaac_install_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for library in steam_library_candidates() {
        let steamapps = library.join("steamapps");
        if let Some(install_dir) =
            read_appmanifest_install_dir(&steamapps.join(format!("appmanifest_{ISAAC_APP_ID}.acf")))
        {
            paths.push(steamapps.join("common").join(install_dir));
        }
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

fn read_libraryfolders(steam_dir: &Path) -> Vec<PathBuf> {
    fs::read_to_string(steam_dir.join("steamapps").join("libraryfolders.vdf"))
        .map(|contents| parse_libraryfolders(&contents))
        .unwrap_or_default()
}

fn read_appmanifest_install_dir(path: &Path) -> Option<String> {
    parse_appmanifest_install_dir(&fs::read_to_string(path).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_launch_uri() {
        assert_eq!(isaac_launch_uri(), "steam://rungameid/250900");
    }
}
