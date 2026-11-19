//! Staged replacement of the running executable, with rollback.
//!
//! The sequence (docs/UPDATE_SYSTEM.md §8): download to a staging file beside
//! the executable (same filesystem, so the final move is a rename), verify
//! size + SHA-256 + a plausible executable header, then ask the staged binary
//! to identify itself and require the manifest's channel, version and full
//! commit — only then is anything allowed to touch the installed file.
//!
//! Replacement keeps a backup of the current executable until the new one has
//! answered `--version --json` from its installed path; on any failure the
//! backup is restored and the staging file removed, so a failed upgrade leaves
//! the installation exactly as it was.
//!
//! Installations owned by a package manager are refused with a pointer to the
//! package manager instead of silently clobbered.

use std::path::{Path, PathBuf};

use super::manifest::{Asset, Manifest};
use super::{Channel, download, homepath, hsize, ok, step, verify};

/// Run the whole installation: stage, verify, replace, re-verify, roll back on
/// failure. Returns the new binary's `0.10.0-canary-4c3eeb`-style label for the
/// closing message.
pub fn perform(
    exe: &Path,
    url: &str,
    manifest: &Manifest,
    asset: &Asset,
    _channel: Channel,
) -> Result<String, String> {
    refuse_if_package_managed(exe)?;

    let staged = beside(exe, ".apkscope-upgrade-new");
    let _ = std::fs::remove_file(&staged); // a leftover from an interrupted run

    step(format!("downloading {}", asset.name));
    download::file(url, &staged).map_err(|e| format!("could not download {} ({e})", asset.name))?;
    let size = std::fs::metadata(&staged).map(|m| m.len()).unwrap_or(0);
    ok(format!("downloaded ({})", hsize(size)));

    replace_verified(exe, &staged, manifest, asset)
}

/// Everything after the download: verify the staged file, replace the
/// executable with a rollback path, and re-verify from the installed path.
/// Split from `perform` so tests can drive the whole safety contract with a
/// locally staged file instead of a network download.
fn replace_verified(
    exe: &Path,
    staged: &Path,
    manifest: &Manifest,
    asset: &Asset,
) -> Result<String, String> {
    let backup = beside(exe, ".apkscope-upgrade-old");

    step("verifying".into());
    verify::staged_file(staged, asset)?;
    // the staged binary must be runnable for its identity check (and is made
    // executable here rather than at replacement, so the check exercises the
    // same permissions the installed file will have)
    make_executable(staged)?;
    verify::staged_identity(staged, manifest)?;
    ok("checksum and identity verified".into());

    // keep the old binary until the new one has proven itself from its
    // installed path; a backup left by an earlier run is replaced
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(exe, &backup).map_err(|e| format!("cannot back up {}: {e}", homepath(exe)))?;
    if let Err(e) = install_staged(staged, exe) {
        // the working binary goes back first, the staging file second
        let _ = std::fs::rename(&backup, exe);
        let _ = std::fs::remove_file(staged);
        return Err(format!("cannot install the new binary: {e}"));
    }

    // post-install verification: the new binary, from its installed path, must
    // still report the manifest's identity; if it does not, roll back
    if let Err(e) = verify::staged_identity(exe, manifest) {
        let _ = std::fs::remove_file(exe);
        let _ = std::fs::rename(&backup, exe);
        let _ = std::fs::remove_file(staged);
        return Err(format!("the installed binary failed verification: {e}"));
    }
    ok(format!("installed {}", homepath(exe)));
    cleanup(&backup, staged);

    Ok(installed_label(exe, manifest))
}

/// The label the closing message shows: what the new binary itself says it is,
/// asked at its installed path. The exiting process's own `identity()` is the
/// old build's, so it would always report the version being replaced.
///
/// The label must not fail the upgrade after it succeeded; if the just-
/// verified binary somehow does not answer, fall back to the manifest, which
/// verification has already proven the file carries.
fn installed_label(exe: &Path, manifest: &Manifest) -> String {
    use std::process::{Command, Stdio};
    let asked = Command::new(exe)
        .args(["--version", "--json"])
        .stdin(Stdio::null())
        .output();
    if let Ok(out) = &asked
        && out.status.success()
        && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        && let (Some(version), Some(commit)) = (
            v.get("version").and_then(|x| x.as_str()),
            v.get("commit").and_then(|x| x.as_str()),
        )
    {
        let channel = v.get("channel").and_then(|x| x.as_str());
        return label(version, channel, commit);
    }
    label(
        &manifest.version,
        Some(manifest.channel.name()),
        &manifest.commit,
    )
}

/// `0.10.0` for a stable build, `0.10.0-canary-4c3eeb` for a canary — the same
/// shape as [`super::current_label`], over borrowed strings (the identity it
/// renders was just read from the new binary, not compiled in).
fn label(version: &str, channel: Option<&str>, commit: &str) -> String {
    if channel == Some(Channel::Canary.name()) && commit.len() >= 6 {
        format!("{version}-canary-{}", &commit[..6])
    } else {
        version.to_string()
    }
}

/// Remove the backup and any staging leftovers; failures are only noted — by
/// this point the upgrade has succeeded and the files are just clutter.
fn cleanup(backup: &Path, staged: &Path) {
    if std::fs::remove_file(backup).is_err() && !cfg!(target_os = "windows") {
        // not on Windows: there the replaced exe is still running (renamed
        // aside, it keeps running), so removal fails every upgrade — the next
        // run overwrites the leftover, no note needed
        println!(
            "  note: the previous binary stays at {} (it could not be removed; delete it manually)",
            homepath(backup)
        );
    }
    let _ = std::fs::remove_file(staged);
}

/// A staging/backup file next to the executable: same directory, so the final
/// move is a same-filesystem rename.
fn beside(exe: &Path, suffix: &str) -> PathBuf {
    let name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "apkscope".into());
    exe.parent()
        .unwrap_or(Path::new("."))
        .join(format!("{name}{suffix}"))
}

/// Move the staged file onto `exe`.
///
/// Unix: a rename — atomic, and allowed while the target runs. Windows: a
/// running exe cannot be replaced or deleted, but it can be renamed aside; the
/// leftover is cleaned up by a later run (this process is still running, so
/// deleting it here would fail anyway).
/// Move the staged file onto `exe` — a rename, atomic and allowed while the
/// target runs. The running exe itself was already moved onto `backup` by the
/// caller; on Windows a running exe can be renamed but not replaced or
/// deleted, which is exactly why the backup move comes first (and why the
/// backup's removal may be deferred to a later run — see `cleanup`).
fn install_staged(staged: &Path, exe: &Path) -> Result<(), String> {
    std::fs::rename(staged, exe).map_err(|e| e.to_string())
}

/// The downloaded file gets the downloader's default permissions; make it run.
#[cfg(unix)]
fn make_executable(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn make_executable(_p: &Path) -> Result<(), String> {
    Ok(())
}

/// Refuse to upgrade an installation a package manager owns: replacing its
/// file under it breaks the database (an upgrade would resurrect the old
/// binary) and the manager would never see the update. The mechanisms are
/// best-effort and documented as such in docs/UPDATE_SYSTEM.md.
fn refuse_if_package_managed(exe: &Path) -> Result<(), String> {
    // Windows has no package-manager detection below, so `exe` would be an
    // unused parameter there (`-D warnings` errors on it; CI lints on Linux
    // only, so it slipped through).
    #[cfg(windows)]
    let _ = exe;
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // dpkg: the file is recorded in /var/lib/dpkg/info/<pkg>.list
        if let Ok(out) = std::process::Command::new("dpkg")
            .args(["-S", exe.to_string_lossy().as_ref()])
            .stdin(std::process::Stdio::null())
            .output()
            && out.status.success()
            && !out.stdout.is_empty()
        {
            let pkg = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .and_then(|l| l.split(':').next())
                .unwrap_or("a package")
                .to_string();
            return Err(format!(
                "{} is installed by dpkg ({}); upgrade it with your package manager (apt) instead",
                homepath(exe),
                pkg
            ));
        }
        // rpm: `rpm -qf` prints the owning package
        if let Ok(out) = std::process::Command::new("rpm")
            .args(["-qf", exe.to_string_lossy().as_ref()])
            .stdin(std::process::Stdio::null())
            .output()
            && out.status.success()
            && !out.stdout.is_empty()
        {
            return Err(format!(
                "{} is installed by rpm ({}); upgrade it with your package manager (dnf/yum) instead",
                homepath(exe),
                String::from_utf8_lossy(&out.stdout).trim()
            ));
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Homebrew: a real brew install is a symlink from a bin dir
        // (/opt/homebrew/bin on Apple Silicon, /usr/local/bin on Intel) into
        // the Cellar, so canonicalizing the exe resolves through it. The bin
        // dir alone says nothing — /usr/local/bin is a normal manual install
        // dir on Intel Macs (the installer documents it), and there is no
        // apkscope formula — so only a resolved path inside a Cellar/ is
        // refused.
        if let Ok(real) = std::fs::canonicalize(exe) {
            if real.to_string_lossy().contains("/Cellar/") {
                return Err(format!(
                    "{} is a Homebrew install ({}); upgrade it with `brew upgrade apkscope` instead",
                    homepath(exe),
                    real.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/apkscope/upgrade_install.rs"]
mod tests;
