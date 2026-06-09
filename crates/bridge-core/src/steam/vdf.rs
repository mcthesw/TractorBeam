use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::SteamAccount;

/// Parses Steam's `loginusers.vdf` KeyValues file.
#[must_use]
pub fn parse_loginusers(contents: &str) -> Vec<SteamAccount> {
    let mut accounts = Vec::new();
    let mut current = None;

    for line in contents.lines().map(str::trim) {
        let tokens = quoted_tokens(line);
        if tokens.len() == 1 && is_steam_id64(&tokens[0]) {
            if let Some(account) = current.take() {
                accounts.push(account);
            }
            current = Some(SteamAccount {
                steam_id64: tokens[0].clone(),
                account_name: None,
                persona_name: None,
                most_recent: false,
            });
            continue;
        }

        if line.starts_with('}') {
            if let Some(account) = current.take() {
                accounts.push(account);
            }
            continue;
        }

        if tokens.len() >= 2
            && let Some(account) = current.as_mut()
        {
            match tokens[0].as_str() {
                "AccountName" => account.account_name = Some(tokens[1].clone()),
                "PersonaName" => account.persona_name = Some(tokens[1].clone()),
                "MostRecent" => account.most_recent = tokens[1] == "1",
                _ => {}
            }
        }
    }

    if let Some(account) = current {
        accounts.push(account);
    }
    accounts
}

/// Parses Steam's `libraryfolders.vdf` and returns configured library roots.
#[must_use]
pub fn parse_libraryfolders(contents: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    for line in contents.lines().map(str::trim) {
        let tokens = quoted_tokens(line);
        if tokens.len() < 2 {
            continue;
        }
        if tokens[0] == "path" || tokens[0].bytes().all(|byte| byte.is_ascii_digit()) {
            paths.push(PathBuf::from(tokens[1].replace("\\\\", "\\")));
        }
    }

    dedup_paths(paths)
}

/// Parses Steam's app manifest and returns the install directory if present.
#[must_use]
pub fn parse_appmanifest_install_dir(contents: &str) -> Option<String> {
    contents.lines().map(str::trim).find_map(|line| {
        let tokens = quoted_tokens(line);
        (tokens.len() >= 2 && tokens[0] == "installdir").then(|| tokens[1].clone())
    })
}

fn quoted_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('"') else {
            break;
        };
        tokens.push(after_start[..end].to_owned());
        rest = &after_start[end + 1..];
    }
    tokens
}

fn is_steam_id64(value: &str) -> bool {
    value.len() == 17 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn dedup_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut unique = Vec::new();
    for path in paths {
        let key = path_key(&path);
        if !key.is_empty() && seen.insert(key) {
            unique.push(path);
        }
    }
    unique
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::super::paths::ISAAC_DEFAULT_INSTALL_DIR;
    use super::*;

    #[test]
    fn parses_loginusers() {
        let accounts = parse_loginusers(
            r#"
            "users"
            {
                "76561198000000001"
                {
                    "AccountName" "alice"
                    "PersonaName" "Alice"
                    "MostRecent" "1"
                }
                "76561198000000002"
                {
                    "AccountName" "bob"
                    "PersonaName" "Bob"
                    "MostRecent" "0"
                }
            }
            "#,
        );

        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].steam_id64, "76561198000000001");
        assert_eq!(accounts[0].display_name(), "Alice");
        assert!(accounts[0].most_recent);
        assert_eq!(accounts[1].display_name(), "Bob");
    }

    #[test]
    fn parses_libraryfolders() {
        let paths = parse_libraryfolders(
            r#"
            "libraryfolders"
            {
                "0"
                {
                    "path" "D:\\SteamLibrary"
                }
                "1" "E:\\Games\\Steam"
            }
            "#,
        );

        assert_eq!(
            paths,
            [
                PathBuf::from(r"D:\SteamLibrary"),
                PathBuf::from(r"E:\Games\Steam")
            ]
        );
    }

    #[test]
    fn parses_appmanifest_install_dir() {
        let install_dir = parse_appmanifest_install_dir(
            r#"
            "AppState"
            {
                "appid" "250900"
                "installdir" "The Binding of Isaac Rebirth"
            }
            "#,
        );

        assert_eq!(install_dir.as_deref(), Some(ISAAC_DEFAULT_INSTALL_DIR));
    }
}
