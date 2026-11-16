//! Verification: the staged file against the manifest, and the staged binary's
//! identity against the release it claims to come from.
//!
//! Everything here runs on the staged file, before anything replaces the
//! installed executable: exact byte size, SHA-256, a plausible executable
//! header, and — the last gate — the staged binary's own `--version --json`
//! must report the channel, version and full commit the manifest promises.
//! A stale CDN copy or a half-updated canary release is caught here.

use sha2::{Digest, Sha256};
use std::path::Path;

use super::manifest::{Asset, Manifest};
use super::platform;

/// The lowercase hex SHA-256 of `path`'s contents.
pub fn sha256_of(path: &Path) -> Result<String, String> {
    let mut f =
        std::fs::File::open(path).map_err(|e| format!("cannot read the staged file: {e}"))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut f, &mut hasher).map_err(|e| format!("cannot read the staged file: {e}"))?;
    Ok(hex(&hasher.finalize()))
}

/// Check the staged file: exact size and digest from the manifest, and a
/// plausible executable header for this platform.
pub fn staged_file(path: &Path, asset: &Asset) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot stat the staged file: {e}"))?;
    if meta.len() != asset.size {
        return Err(format!(
            "size mismatch for {}: manifest says {} bytes, got {}",
            asset.name,
            asset.size,
            meta.len()
        ));
    }
    let got = sha256_of(path)?;
    if got != asset.sha256 {
        return Err(format!(
            "SHA-256 mismatch for {}\n  expected {}\n  got      {got}",
            asset.name, asset.sha256
        ));
    }
    platform::plausible_executable(path)
}

/// Run the staged binary's identity check and compare it against the manifest.
///
/// The staged file answers `--version --json` with `{"version": ...,
/// "channel": ..., "commit": ...}` (see `main.rs`); anything else — a non-zero
/// exit, unparseable output, or a field that does not match the release —
/// aborts the upgrade, because the file is not the binary the manifest
/// describes.
pub fn staged_identity(path: &Path, m: &Manifest) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let out = Command::new(path)
        .args(["--version", "--json"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run the staged binary: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "the staged binary failed its identity check (exit {})",
            out.status.code().unwrap_or(-1)
        ));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("the staged binary's identity is not valid JSON: {e}"))?;
    let (version, channel, commit) = (
        v.get("version").and_then(|x| x.as_str()),
        v.get("channel").and_then(|x| x.as_str()),
        v.get("commit").and_then(|x| x.as_str()),
    );
    // a local dev build answers channel "dev"; that never matches a release
    if channel != Some(m.channel.name()) {
        return Err(format!(
            "channel mismatch: the staged binary is a {} build, the release is {}",
            channel.unwrap_or("unknown"),
            m.channel.name()
        ));
    }
    if version != Some(m.version.as_str()) {
        return Err(format!(
            "the staged binary is version {}, the release is {}",
            version.unwrap_or("unknown"),
            m.version
        ));
    }
    if commit != Some(m.commit.as_str()) {
        return Err(format!(
            "the staged binary was built from a different commit\n  manifest {}\n  binary   {}",
            m.commit,
            commit.unwrap_or("unknown")
        ));
    }
    Ok(())
}

/// Lowercase hex, the format `sha256sum` prints.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

use std::fmt::Write as _;

#[cfg(test)]
#[path = "../../tests/apkscope/upgrade_verify.rs"]
mod tests;
