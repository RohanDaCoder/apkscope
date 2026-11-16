//! The update manifest (`apkscope-update.json`): fetch and validate.
//!
//! The manifest is the only update metadata the client consumes. It arrives
//! through the ordinary public release-asset URL (no API), and everything the
//! updater does next is derived from it: which asset this platform needs, its
//! exact size and SHA-256, the channel and package version of the release and
//! the full commit the binaries were built from.
//!
//! Validation fails closed: any unknown schema, wrong channel, bad commit,
//! missing platform entry, mismatched asset name or malformed digest aborts the
//! upgrade before a single byte of the binary is fetched.

use serde_json::Value;

use super::{Channel, platform};

/// The manifest schema this build understands; unknown major versions — a
/// future, incompatible format — are rejected rather than guessed at.
pub const SCHEMA_VERSION: u64 = 1;

/// A validated manifest: the release's channel, package version, full build
/// commit and this platform's asset entry.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub channel: Channel,
    pub version: String,
    /// The full 40-char commit the release's binaries were built from.
    pub commit: String,
    /// This platform's entry, already name-checked against the platform map.
    pub asset: Asset,
}

/// One platform's asset: exact release filename, byte size and SHA-256.
#[derive(Clone, Debug)]
pub struct Asset {
    #[allow(dead_code)] // read in tests; production always selects by platform first
    pub platform: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// Fetch the manifest of `channel`'s release (tag `tag`; `latest` for stable's
/// newest) and validate it for this platform. `expect_version`, when set, is
/// the exact stable version requested on the command line — the manifest must
/// carry it, or the release behind the URL is not what the user asked for.
pub fn fetch(
    channel: &Channel,
    tag: &str,
    expect_version: Option<String>,
) -> Result<Manifest, String> {
    let url = super::download_url(tag, "apkscope-update.json");
    let text = super::download::text(&url).map_err(|e| {
        format!(
            "cannot download the update manifest ({e}); no {} release published yet? check {}/{}/releases",
            channel.name(),
            super::base_url(),
            super::repo()
        )
    })?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| format!("the update manifest is not valid JSON: {e}"))?;
    let m = parse(&v, channel, platform::current())?;
    if let Some(want) = expect_version
        && m.version != want
    {
        return Err(format!(
            "release {tag} carries version {}, not {want}",
            m.version
        ));
    }
    Ok(m)
}

/// Validate a parsed manifest for `platform`. Every field is checked against
/// the schema; see docs/UPDATE_SYSTEM.md for the full list of rejections.
pub fn parse(v: &Value, channel: &Channel, platform: &str) -> Result<Manifest, String> {
    let schema = v
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or("manifest: schema_version missing or not a number")?;
    if schema != SCHEMA_VERSION {
        return Err(format!(
            "manifest: schema version {schema} is not supported (this build understands {SCHEMA_VERSION})"
        ));
    }
    let m_channel = v
        .get("channel")
        .and_then(Value::as_str)
        .ok_or("manifest: channel missing")?;
    if m_channel != channel.name() {
        return Err(format!(
            "manifest: channel is {m_channel}, expected {}",
            channel.name()
        ));
    }
    let version = v
        .get("version")
        .and_then(Value::as_str)
        .ok_or("manifest: version missing")?;
    if !valid_version(version) {
        return Err(format!("manifest: invalid version '{version}'"));
    }
    let commit = v
        .get("commit")
        .and_then(Value::as_str)
        .ok_or("manifest: commit missing")?;
    if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("manifest: commit is not a full 40-character hex SHA".into());
    }

    let entry = v
        .get("assets")
        .and_then(|a| a.get(platform))
        .ok_or_else(|| format!("manifest: no asset for this platform ({platform})"))?;
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .ok_or("manifest: asset name missing")?;
    // the manifest may only name the file this platform maps to; anything else
    // (path traversal, another platform's asset, a typo) is rejected
    if Some(name) != platform::asset_name(platform) {
        return Err(format!(
            "manifest: asset name '{name}' does not match platform {platform}"
        ));
    }
    let size = entry
        .get("size")
        .and_then(Value::as_u64)
        .ok_or("manifest: asset size missing or invalid")?;
    if size == 0 {
        return Err("manifest: asset size is zero".into());
    }
    let sha256 = entry
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or("manifest: asset sha256 missing")?
        .to_lowercase();
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("manifest: asset sha256 is not a 64-character hex digest".into());
    }

    Ok(Manifest {
        channel: *channel,
        version: version.to_string(),
        commit: commit.to_string(),
        asset: Asset {
            platform: platform.to_string(),
            name: name.to_string(),
            size,
            sha256,
        },
    })
}

/// `X.Y.Z` with an optional `-suffix`: exactly three numeric parts before the
/// first `-` (and the `-` only when followed by something).
fn valid_version(v: &str) -> bool {
    let (core, suffix) = match v.split_once('-') {
        Some((c, s)) => (c, Some(s)),
        None => (v, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let core_ok = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let suffix_ok = suffix.is_none_or(|s| !s.is_empty());
    core_ok && suffix_ok
}

/// The already-validated manifest's own asset — `fetch` validated this
/// platform's entry, so selecting is just unwrapping it. Kept as its own step
/// so tests can exercise `parse` and selection independently.
pub fn select_asset<'a>(m: &'a Manifest, _tag: &str) -> Result<&'a Asset, String> {
    Ok(&m.asset)
}

#[cfg(test)]
#[path = "../../tests/apkscope/upgrade_manifest.rs"]
mod tests;
