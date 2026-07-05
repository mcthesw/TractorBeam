//! Compile-time build provenance shared by the GUI, relay, and diagnostics.

use serde::Serialize;

const SHORT_HASH_LEN: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_hash: Option<&'static str>,
}

impl BuildInfo {
    #[must_use]
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            git_hash: normalized_git_hash(),
        }
    }

    #[must_use]
    pub fn version_label(self) -> String {
        match self.git_hash {
            Some(hash) => format!("{}+{}", self.version, short_git_hash(hash)),
            None => self.version.to_owned(),
        }
    }
}

#[must_use]
pub fn current() -> BuildInfo {
    BuildInfo::current()
}

#[must_use]
pub fn version_label() -> String {
    current().version_label()
}

fn normalized_git_hash() -> Option<&'static str> {
    option_env!("TRACTOR_BEAM_GIT_HASH").and_then(|hash| {
        let hash = hash.trim();
        (!hash.is_empty()).then_some(hash)
    })
}

fn short_git_hash(hash: &str) -> &str {
    hash.char_indices()
        .nth(SHORT_HASH_LEN)
        .map_or(hash, |(index, _)| &hash[..index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_exposes_package_version() {
        let info = BuildInfo::current();

        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.version_label().is_empty());
    }

    #[test]
    fn display_uses_short_git_hash() {
        let info = BuildInfo {
            version: "1.2.3",
            git_hash: Some("0123456789abcdef"),
        };

        assert_eq!(info.version_label(), "1.2.3+0123456789ab");
    }
}
