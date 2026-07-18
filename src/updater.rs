use std::{process::Command, time::Duration};

#[cfg(windows)]
use std::fs;

use anyhow::{Context as _, Result, anyhow, bail};
use semver::Version;
use serde::Deserialize;

const RELEASE_API: &str = "https://api.github.com/repos/nikolas-sturm/preprint/releases/latest";
const DOWNLOAD_PREFIX: &str = "https://github.com/nikolas-sturm/preprint/releases/download/";
const RELEASE_PREFIX: &str = "https://github.com/nikolas-sturm/preprint/releases/tag/";
const USER_AGENT: &str = concat!("preprint/", env!("CARGO_PKG_VERSION"));
const MAX_RELEASE_RESPONSE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableUpdate {
    pub version: String,
    release_url: String,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

pub fn is_setup_installation() -> bool {
    #[cfg(not(windows))]
    {
        false
    }

    #[cfg(windows)]
    {
        cleanup_stale_installers();
        let Ok(executable) = std::env::current_exe() else {
            return false;
        };
        let Some(directory) = executable.parent() else {
            return false;
        };
        if directory.join(".preprint-setup").is_file() {
            return true;
        }

        fs::read_dir(directory).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                name.starts_with("unins") && name.ends_with(".exe")
            })
        })
    }
}

pub fn check_for_update() -> Result<Option<AvailableUpdate>> {
    let agent = http_agent(Duration::from_secs(30));
    let mut response = agent
        .get(RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", USER_AGENT)
        .header("X-GitHub-Api-Version", "2026-03-10")
        .call()
        .context("request latest GitHub release")?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_RELEASE_RESPONSE_BYTES)
        .read_to_string()
        .context("read latest GitHub release")?;
    let release: GitHubRelease =
        serde_json::from_str(&body).context("parse latest GitHub release")?;
    update_from_release(release, env!("CARGO_PKG_VERSION"))
}

pub fn open_release_page(update: &AvailableUpdate) -> Result<()> {
    let mut command = if cfg!(target_os = "windows") {
        Command::new("explorer")
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(&update.release_url)
        .spawn()
        .context("open update release page")?;
    Ok(())
}

#[cfg(windows)]
fn cleanup_stale_installers() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("preprint-update-") && name.ends_with(".exe") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn http_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(timeout))
        .max_redirects(5)
        .build()
        .into()
}

fn update_from_release(
    release: GitHubRelease,
    current_version: &str,
) -> Result<Option<AvailableUpdate>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }

    let version_text = release
        .tag_name
        .strip_prefix('v')
        .ok_or_else(|| anyhow!("latest release tag must start with v"))?;
    let version = Version::parse(version_text).context("parse latest release version")?;
    let current = Version::parse(current_version).context("parse current application version")?;
    if version <= current {
        return Ok(None);
    }

    let expected_name = format!("Preprint-{version}-x86_64-setup.exe");
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == expected_name)
        .ok_or_else(|| anyhow!("latest release has no {expected_name} asset"))?;
    if !asset.browser_download_url.starts_with(DOWNLOAD_PREFIX) {
        bail!("update installer has unexpected download URL");
    }

    Ok(Some(AvailableUpdate {
        version: version.to_string(),
        release_url: format!("{RELEASE_PREFIX}v{version}"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, asset_version: &str) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.to_owned(),
            draft: false,
            prerelease: false,
            assets: vec![GitHubAsset {
                name: format!("Preprint-{asset_version}-x86_64-setup.exe"),
                browser_download_url: format!(
                    "{DOWNLOAD_PREFIX}v{asset_version}/Preprint-{asset_version}-x86_64-setup.exe"
                ),
            }],
        }
    }

    #[test]
    fn selects_newer_setup_release() {
        let update = update_from_release(release("v1.2.0", "1.2.0"), "1.1.0")
            .unwrap()
            .unwrap();

        assert_eq!(update.version, "1.2.0");
        assert_eq!(update.release_url, format!("{RELEASE_PREFIX}v1.2.0"));
    }

    #[test]
    fn ignores_current_and_older_releases() {
        assert!(
            update_from_release(release("v1.1.0", "1.1.0"), "1.1.0")
                .unwrap()
                .is_none()
        );
        assert!(
            update_from_release(release("v1.0.9", "1.0.9"), "1.1.0")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn ignores_prereleases() {
        let mut candidate = release("v1.2.0-beta.1", "1.2.0-beta.1");
        candidate.prerelease = true;

        assert!(update_from_release(candidate, "1.1.0").unwrap().is_none());
    }

    #[test]
    fn rejects_untrusted_assets() {
        let mut untrusted = release("v1.2.0", "1.2.0");
        untrusted.assets[0].browser_download_url = "https://example.com/setup.exe".into();
        assert!(update_from_release(untrusted, "1.1.0").is_err());
    }

    #[test]
    fn setup_detection_is_disabled_outside_windows() {
        #[cfg(not(windows))]
        assert!(!is_setup_installation());
    }
}
