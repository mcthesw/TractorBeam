pub(crate) fn version_label() -> String {
    match embedded_git_hash() {
        Some(hash) => format!("{}+{}", env!("CARGO_PKG_VERSION"), short_hash(hash)),
        None => env!("CARGO_PKG_VERSION").to_owned(),
    }
}

fn embedded_git_hash() -> Option<&'static str> {
    option_env!("TRACTOR_BEAM_GIT_HASH")
        .or(option_env!("BASEMENT_BRIDGE_GIT_HASH"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_label_contains_package_version() {
        assert!(version_label().starts_with(env!("CARGO_PKG_VERSION")));
    }
}
