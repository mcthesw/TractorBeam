use std::time::Duration;

use semver::Version;
use serde::Deserialize;

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/mcthesw/TractorBeam/releases/latest";
const RELEASE_URL_PREFIX: &str = "https://github.com/mcthesw/TractorBeam/releases/";
const GITHUB_API_VERSION: &str = "2026-03-10";
const RESPONSE_LIMIT_BYTES: u64 = 64 * 1024;
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) type UpdateCheck = Box<dyn FnOnce() -> Result<Option<AvailableUpdate>, String> + Send>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AvailableUpdate {
    pub(crate) version: String,
    pub(crate) url: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
}

pub(crate) fn check_for_update(current_version: &str) -> Result<Option<AvailableUpdate>, String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(UPDATE_CHECK_TIMEOUT))
        .build();
    let agent: ureq::Agent = config.into();
    let user_agent = format!("Tractor-Beam/{current_version}");
    let mut response = agent
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .header("User-Agent", user_agent)
        .call()
        .map_err(|error| format!("GitHub latest release request failed: {error}"))?;
    let release = response
        .body_mut()
        .with_config()
        .limit(RESPONSE_LIMIT_BYTES)
        .read_json::<GitHubRelease>()
        .map_err(|error| format!("GitHub latest release response was invalid: {error}"))?;

    available_update(current_version, &release)
}

pub(crate) fn spawn_check(
    update_check: Option<UpdateCheck>,
    on_available: impl FnOnce(AvailableUpdate) + Send + 'static,
) {
    let Some(update_check) = update_check else {
        return;
    };
    if let Err(error) = std::thread::Builder::new()
        .name("tractor-beam-update-check".to_owned())
        .spawn(move || match update_check() {
            Ok(Some(update)) => {
                tracing::info!(available_version = %update.version, "New Tractor Beam release available");
                on_available(update);
            }
            Ok(None) => tracing::debug!("Tractor Beam is up to date"),
            Err(error) => tracing::warn!(error = %error, "Update check failed"),
        })
    {
        tracing::warn!(error = %error, "Could not start update check worker");
    }
}

fn available_update(
    current_version: &str,
    release: &GitHubRelease,
) -> Result<Option<AvailableUpdate>, String> {
    let current = Version::parse(current_version)
        .map_err(|error| format!("Current version {current_version:?} is invalid: {error}"))?;
    let tag = release.tag_name.trim();
    let version_text = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    let latest = Version::parse(version_text)
        .map_err(|error| format!("Release tag {:?} is not SemVer: {error}", release.tag_name))?;
    if latest <= current {
        return Ok(None);
    }
    if !release.html_url.starts_with(RELEASE_URL_PREFIX) {
        return Err(format!(
            "Release URL {:?} is outside the Tractor Beam repository",
            release.html_url
        ));
    }

    Ok(Some(AvailableUpdate {
        version: latest.to_string(),
        url: release.html_url.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag_name: &str) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag_name.to_owned(),
            html_url: format!("{RELEASE_URL_PREFIX}tag/{tag_name}"),
        }
    }

    #[test]
    fn newer_semver_release_is_available() {
        let update = available_update("0.5.1", &release("v0.6.0"))
            .unwrap()
            .unwrap();

        assert_eq!(update.version, "0.6.0");
        assert_eq!(
            update.url,
            "https://github.com/mcthesw/TractorBeam/releases/tag/v0.6.0"
        );
    }

    #[test]
    fn equal_or_older_release_is_not_available() {
        assert_eq!(available_update("0.5.1", &release("v0.5.1")).unwrap(), None);
        assert_eq!(available_update("0.5.1", &release("v0.5.0")).unwrap(), None);
    }

    #[test]
    fn stable_release_updates_a_prerelease_build() {
        let update = available_update("0.6.0-beta.1", &release("0.6.0"))
            .unwrap()
            .unwrap();

        assert_eq!(update.version, "0.6.0");
    }

    #[test]
    fn invalid_versions_and_unexpected_urls_are_rejected() {
        assert!(available_update("dev", &release("v0.6.0")).is_err());
        assert!(available_update("0.5.1", &release("latest")).is_err());

        let mut unexpected = release("v0.6.0");
        unexpected.html_url = "https://example.com/v0.6.0".to_owned();
        assert!(available_update("0.5.1", &unexpected).is_err());
    }
}
