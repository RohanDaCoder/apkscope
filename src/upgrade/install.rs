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

    Ok(super::current_label(&super::identity()))
}

/// Remove the backup and any staging leftovers; failures are only noted — by
/// this point the upgrade has succeeded and the files are just clutter.
fn cleanup(backup: &Path, staged: &Path) {
    if std::fs::remove_file(backup).is_err() {
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
fn install_staged(staged: &Path, exe: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let old = beside(exe, ".apkscope-upgrade-previous");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old)
            .map_err(|e| format!("cannot move the current binary aside: {e}"))?;
        if let Err(e) = std::fs::rename(staged, exe) {
            let _ = std::fs::rename(&old, exe); // keep the working binary working
            return Err(e.to_string());
        }
        // `backup` (from perform) also still holds the pre-upgrade binary; the
        // two copies are fine — cleanup removes what it can
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(staged, exe).map_err(|e| e.to_string())?;
    }
    Ok(())
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
        // Homebrew: /opt/homebrew/bin (Apple Silicon) or /usr/local/bin
        if let Some(dir) = exe.parent().and_then(|d| d.to_str()) {
            for prefix in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/homebrew/opt"] {
                if dir.starts_with(prefix) {
                    return Err(format!(
                        "{} looks like a Homebrew install ({}); upgrade it with `brew upgrade apkscope` instead",
                        homepath(exe),
                        dir
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/apkscope/upgrade_install.rs"]
mod tests;
