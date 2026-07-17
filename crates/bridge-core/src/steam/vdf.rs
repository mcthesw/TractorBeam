use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::SteamAccount;

#[derive(Deserialize)]
#[serde(rename = "users")]
struct LoginUsers(BTreeMap<String, RawSteamAccount>);

#[derive(Deserialize)]
struct RawSteamAccount {
    #[serde(rename = "AccountName")]
    account_name: Option<String>,
    #[serde(rename = "PersonaName")]
    persona_name: Option<String>,
    #[serde(rename = "MostRecent", default)]
    most_recent: bool,
}

/// Parses Steam's `loginusers.vdf` KeyValues document.
#[must_use]
pub fn parse_loginusers(contents: &str) -> Vec<SteamAccount> {
    let Ok(LoginUsers(accounts)) = keyvalues_serde::from_str(contents) else {
        return Vec::new();
    };
    accounts
        .into_iter()
        .filter(|(steam_id64, _)| is_steam_id64(steam_id64))
        .map(|(steam_id64, account)| SteamAccount {
            steam_id64,
            account_name: account.account_name,
            persona_name: account.persona_name,
            most_recent: account.most_recent,
        })
        .collect()
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
    fn malformed_loginusers_is_ignored() {
        assert!(parse_loginusers("not valid VDF").is_empty());
    }
}
