//! Release discovery is restricted to Cuttlefish's published GitHub releases.
use crate::Error;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub(super) const CATALOG: &str =
    "https://api.github.com/repos/Code-Mount-Labs/cuttlefish/releases?per_page=100";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Release {
    pub tag_name: String,
    pub draft: bool,
    pub assets: Vec<Asset>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Asset {
    pub name: String,
}

pub(super) fn version(tag: &str) -> Result<semver::Version, Error> {
    let value = tag
        .strip_prefix('v')
        .ok_or(Error::Invalid("invalid_release_version"))?;
    let version =
        semver::Version::parse(value).map_err(|_| Error::Invalid("invalid_release_version"))?;
    if !version.build.is_empty()
        || (!version.pre.is_empty() && !version.pre.as_str().starts_with("alpha."))
    {
        return Err(Error::Invalid("unsupported_release_channel"));
    }
    Ok(version)
}
pub(super) fn target(os: &str, arch: &str) -> Result<&'static str, Error> {
    match (os, arch) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        _ => Err(Error::Invalid("updates_not_supported_on_this_platform")),
    }
}
pub(super) fn latest(
    releases: Vec<Release>,
    current: &str,
    target: &str,
) -> Result<Option<String>, Error> {
    let current = version(current)?;
    Ok(releases
        .into_iter()
        .filter_map(|release| {
            let parsed = version(&release.tag_name).ok()?;
            let archive = format!("cuttlefish-{}-{target}.tar.gz", release.tag_name);
            (!release.draft
                && parsed > current
                && release.assets.iter().any(|asset| asset.name == archive)
                && release
                    .assets
                    .iter()
                    .any(|asset| asset.name == "SHA256SUMS"))
            .then_some((parsed, release.tag_name))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, tag)| tag))
}
pub(super) async fn fetch(url: &str) -> Result<Vec<Release>, Error> {
    fetch_with(url, reqwest::Client::builder()).await
}
async fn fetch_with(url: &str, builder: reqwest::ClientBuilder) -> Result<Vec<Release>, Error> {
    let client = builder
        .timeout(Duration::from_secs(20))
        .user_agent("cuttlefish-updater")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::Dependency("release_check_unavailable"))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| Error::Dependency("release_check_unavailable"))?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::Dependency("release_check_interrupted"))?
    {
        if bytes.len() + chunk.len() > 2_000_000 {
            return Err(Error::Dependency("release_catalog_too_large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Dependency("invalid_release_catalog"))
}

#[cfg(test)]
#[path = "../../tests/internal/update_catalog.rs"]
mod tests;
