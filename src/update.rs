//! `apkscope update` — replace this binary with the latest release of a channel.
//!
//! Two channels, one mechanism each:
//!
//! - **stable**: the newest non-prerelease GitHub Release. Its tag is read from the
//!   `releases/latest` redirect (a HEAD that answers `location: .../tag/v0.10.0`),
//!   so no API and no change to the existing release pipeline; the binaries come
//!   from `releases/latest/download`, exactly what the installers fetch.
//! - **canary**: the moving `canary` prerelease that CI republishes on every push to
//!   `main` (`.github/workflows/canary.yml`), so "the latest commit's artifacts" is
//!   just "the `canary` release's assets". A tiny `version.txt` asset carries the
//!   build's version, `X.Y+1.0-canary.<sha6>` — also compiled into the binary
//!   through the `APKSCOPE_CANARY_VERSION` build env, so a canary answers
//!   `--version` with it, keys its caches by it, and always sorts above the stable
//!   it builds on.
//!
//! The download itself is delegated to `curl` or `wget` — the same tools the
//! installers rely on, and the ones that put this binary here in the first place —
//! instead of linking an HTTP client: the project's promise is one small binary
//! with no new code in it. The SHA-256 from `checksums.txt` is verified with the
//! `sha2` crate (already compiled in via `apkscope-core`), and the running binary
//! replaces itself by rename: atomic on Unix, and on Windows a rename of the
//! running exe to `<name>.exe.old` first, because a running exe can be renamed but
//! not deleted.
//!
//! Forks and mirrors: `APKSCOPE_REPO` and `APKSCOPE_BASE_URL` override the GitHub
//! host and `owner/name`, like the installers.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What this build identifies as: `0.10.0`, or `0.11.0-canary.1a2b3c` for a canary.
///
/// `--version`, the cache file names and the MCP `serverInfo` all report this, so a
/// canary says so everywhere a version is shown.
pub fn current() -> &'static str {
    match option_env!("APKSCOPE_CANARY_VERSION") {
        Some(v) => v,
        None => env!("CARGO_PKG_VERSION"),
    }
}

/// True when this binary is a canary build (`-canary.` in its version).
fn on_canary() -> bool {
    current().contains("-canary.")
}

/// The repo releases are downloaded from, overridable for forks and mirrors.
fn repo() -> String {
    std::env::var("APKSCOPE_REPO").unwrap_or_else(|_| "suffixing/apkscope".into())
}

/// The GitHub host (or Enterprise / mirror base URL), like the installers.
fn base_url() -> String {
    std::env::var("APKSCOPE_BASE_URL").unwrap_or_else(|_| "https://github.com".into())
}

/// The update channel: stable by default, canary when asked.
enum Channel {
    Stable,
    Canary,
}

/// The `apkscope update` command; `args` is everything after `update`.
pub fn update(mut args: Vec<String>) -> Result<(), String> {
    let (canary, stable) = (
        crate::flag(&mut args, "--canary"),
        crate::flag(&mut args, "--stable"),
    );
    if canary && stable {
        return Err("choose one channel: --stable or --canary".into());
    }
    if !args.is_empty() {
        return Err("usage: apkscope update [--stable | --canary]".into());
    }
    let channel = if canary {
        Channel::Canary
    } else {
        Channel::Stable
    };

    let exe =
        std::env::current_exe().map_err(|e| format!("cannot find this binary's path: {e}"))?;

    // Resolve what the channel would install, for the label and the up-to-date
    // check. Stable is best effort (the tag read can fail; then it just installs
    // "latest"); canary needs its version.txt — without it there is nothing to
    // compare against and the download would fail anyway.
    let target = match &channel {
        Channel::Stable => latest_stable_tag(),
        Channel::Canary => Some(canary_version()?),
    };
    let label = channel_label(&channel, target.as_deref());

    if let Some(t) = target.as_deref()
        && plain(t) == current()
    {
        println!("apkscope {} is already the latest {}", current(), label);
        return Ok(());
    }

    println!("\n  {}apkscope updater{}\n", tty().bold, tty().reset);
    field("channel", &label);
    field("current", current());
    field("binary", &homepath(&exe));
    if on_canary() != matches!(channel, Channel::Canary) {
        println!(
            "  note: switching from {} to {label}",
            if on_canary() { "canary" } else { "stable" }
        );
    }
    println!();

    let asset = asset_name();
    let dir = match &channel {
        Channel::Stable => format!("{}/{}/releases/latest/download", base_url(), repo()),
        Channel::Canary => format!("{}/{}/releases/download/canary", base_url(), repo()),
    };

    let result = install(&exe, &dir, &asset);
    // the temp files are siblings of the binary; clean them up on every path
    let _ = std::fs::remove_file(update_tmp(&exe, "bin"));
    let _ = std::fs::remove_file(update_tmp(&exe, "sums"));
    result
}

/// Download `asset` from `dir`, verify its checksum and put it in `exe`'s place.
fn install(exe: &Path, dir: &str, asset: &str) -> Result<(), String> {
    let tmp = update_tmp(exe, "bin");
    let sums = update_tmp(exe, "sums");

    step(format!("downloading {asset}"));
    fetch(&format!("{dir}/{asset}"), &tmp).map_err(|e| {
        format!(
            "could not download {asset} ({e}); check {}/{}/releases",
            base_url(),
            repo()
        )
    })?;
    let bytes = std::fs::read(&tmp).map_err(|e| e.to_string())?;
    ok(format!("downloaded ({})", hsize(bytes.len())));

    // checksums.txt is verified when published (it always is for our releases);
    // when it cannot be fetched, continue with a note — like the installers
    let sums = fetch(&format!("{dir}/checksums.txt"), &sums)
        .and_then(|_| std::fs::read_to_string(&sums).map_err(|e| e.to_string()))
        .unwrap_or_default();
    match expected_sha(&sums, asset) {
        Some(want) => {
            let got = hex(&Sha256::digest(&bytes));
            if got != want {
                return Err(format!(
                    "SHA-256 mismatch for {asset}\n  expected {want}\n  got      {got}"
                ));
            }
            ok("checksum verified (sha256)".to_string());
        }
        None => println!("  note: {asset} not in checksums.txt, skipping verification"),
    }
    drop(bytes);

    install_binary(exe, &tmp)?;
    ok(format!("installed {}", homepath(exe)));

    // confirm with the new binary itself, like the installers' final --version run
    println!();
    if let Ok(out) = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        && out.status.success()
    {
        print!("{}", String::from_utf8_lossy(&out.stdout));
    }
    println!();
    Ok(())
}

/// A temp file next to the binary, so the final rename never crosses filesystems.
fn update_tmp(exe: &Path, tag: &str) -> PathBuf {
    exe.parent()
        .unwrap_or(Path::new("."))
        .join(format!(".apkscope.update.{tag}.{}", std::process::id()))
}

/// The release asset for this platform, matching release.yml's names. The musl
/// build is its own asset and knows itself at compile time (`target_env`), so no
/// runtime libc sniffing is needed.
fn asset_name() -> String {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "windows") {
        format!("apkscope_windows_{arch}.exe")
    } else if cfg!(target_os = "macos") {
        format!("apkscope_macos_{arch}")
    } else if cfg!(target_env = "musl") {
        format!("apkscope_linux_musl_{arch}")
    } else {
        format!("apkscope_linux_{arch}")
    }
}

/// The tag of the newest stable release, from the `releases/latest` redirect
/// (`location: https://github.com/<repo>/releases/tag/v0.10.0`). `None` when the
/// probe fails — the download URL does not depend on it, only the label does.
fn latest_stable_tag() -> Option<String> {
    let url = format!("{}/{}/releases/latest", base_url(), repo());
    // -I: HEAD, headers on stdout; the redirect is not an error for -f, and not
    // followed (no -L), so the location header is right there
    let headers = Command::new("curl")
        .args(["-fsSI", &url])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .or_else(|| {
            // no curl: wget prints the response headers to stderr, even while
            // refusing to follow the redirect (--max-redirect 0 fails the run,
            // the headers are already printed)
            Command::new("wget")
                .args(["-S", "--spider", "--max-redirect=0", &url])
                .stdin(Stdio::null())
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
        })?;
    let loc = headers
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("location:"))?
        .split_whitespace()
        .nth(1)?;
    let tag = loc.trim_end_matches('\r').rsplit("/tag/").next()?;
    (!tag.is_empty()).then(|| tag.to_string())
}

/// The version the moving `canary` release was built from, from its `version.txt`
/// asset (`0.11.0-canary.1a2b3c`).
fn canary_version() -> Result<String, String> {
    let url = format!(
        "{}/{}/releases/download/canary/version.txt",
        base_url(),
        repo()
    );
    let v = fetch_text(&url)?.trim().to_string();
    if v.is_empty() {
        return Err(format!(
            "empty version.txt for the canary channel; check {}/{}/releases",
            base_url(),
            repo()
        ));
    }
    Ok(v)
}

/// Fetch `url` into `to`, with curl or wget (whichever exists), like the installers.
fn fetch(url: &str, to: &Path) -> Result<(), String> {
    match Command::new("curl")
        .args(["-fsSL", url, "-o"])
        .arg(to)
        .stdin(Stdio::null())
        .status()
    {
        Ok(s) if s.success() => Ok(()),
        // curl exists but the download failed (404, network): retrying with wget
        // would fail the same way, so report it
        Ok(s) => Err(format!("curl exited with {}", s.code().unwrap_or(-1))),
        // no curl on the machine
        Err(_) => match Command::new("wget")
            .args(["-q", url, "-O"])
            .arg(to)
            .stdin(Stdio::null())
            .status()
        {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(format!("wget exited with {}", s.code().unwrap_or(-1))),
            Err(_) => Err("need curl or wget to download".into()),
        },
    }
}

/// Fetch a small text file to a string, with curl or wget.
fn fetch_text(url: &str) -> Result<String, String> {
    match Command::new("curl")
        .args(["-fsSL", url])
        .stdin(Stdio::null())
        .output()
    {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => Err(format!(
            "curl exited with {}",
            o.status.code().unwrap_or(-1)
        )),
        Err(_) => match Command::new("wget")
            .args(["-q", "-O", "-", url])
            .stdin(Stdio::null())
            .output()
        {
            Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
            Ok(o) => Err(format!(
                "wget exited with {}",
                o.status.code().unwrap_or(-1)
            )),
            Err(_) => Err("need curl or wget to download".into()),
        },
    }
}

/// Move `tmp` onto `exe`. Rename is atomic on Unix, even while `exe` runs. On
/// Windows a running exe cannot be replaced or deleted, but it can be renamed
/// aside — the leftover `.old` is dropped by the next update (its process is gone
/// by then; deleting it here fails while this one still runs).
fn install_binary(exe: &Path, tmp: &Path) -> Result<(), String> {
    make_executable(tmp)?;
    #[cfg(windows)]
    {
        let old = with_suffix(exe, ".old");
        let _ = std::fs::remove_file(&old); // a leftover from a previous update
        std::fs::rename(exe, &old)
            .map_err(|e| format!("cannot move the current binary aside: {e}"))?;
        if let Err(e) = std::fs::rename(tmp, exe) {
            let _ = std::fs::rename(&old, exe); // keep the working binary working
            return Err(format!("cannot install the new binary: {e}"));
        }
        if std::fs::remove_file(&old).is_err() {
            println!(
                "  note: the previous binary stays at {} until the next update (Windows cannot delete a running executable)",
                old.display()
            );
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(tmp, exe).map_err(|e| format!("cannot replace {}: {e}", exe.display()))?;
    }
    Ok(())
}

/// `apkscope.exe` -> `apkscope.exe.old` (`with_extension` would eat the `.exe`).
#[cfg(windows)]
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    s.into()
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

// --- labels and small parsing ---------------------------------------------------

/// `v0.10.0` / `0.10.0` -> `0.10.0`, for comparing a tag with the running version.
fn plain(version: &str) -> &str {
    version.trim().trim_start_matches('v')
}

/// The 6-char commit hash of a canary version: `0.11.0-canary.1a2b3c` -> `1a2b3c`.
fn canary_hash(version: &str) -> Option<&str> {
    version.split("-canary.").nth(1).map(str::trim)
}

/// What the channel is shown as: `stable (v0.10.0)`, `canary (1a2b3c)` — the
/// installers print the same labels, so both sides of an update agree.
fn channel_label(channel: &Channel, version: Option<&str>) -> String {
    match channel {
        Channel::Stable => match version {
            Some(v) => format!("stable ({v})"),
            None => "stable (latest)".into(),
        },
        Channel::Canary => match version.and_then(canary_hash) {
            Some(h) => format!("canary ({h})"),
            None => "canary".into(),
        },
    }
}

/// The expected SHA-256 of `asset` from a `sha256sum`-format checksums.txt.
/// The name must match as a whole word: `apkscope_linux_x86_64` must not pick up
/// the line for `apkscope_linux_musl_x86_64`.
fn expected_sha(checksums: &str, asset: &str) -> Option<String> {
    checksums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        match (it.next(), it.next()) {
            (Some(h), Some(n)) if h.len() == 64 && n == asset => Some(h.to_string()),
            _ => None,
        }
    })
}

/// Lowercase hex, the format `sha256sum` prints.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Human-readable byte count, like the installers' `hsize`.
fn hsize(n: usize) -> String {
    if n >= 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{} B", n)
    }
}

/// Show a path under $HOME as `~/...`, like the installers.
fn homepath(p: &Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

// --- output helpers, matching the installers' look ------------------------------

/// Colors only on a terminal, off with NO_COLOR or TERM=dumb; the unicode tick
/// only where it renders (same rules as the installers).
struct Tty {
    bold: &'static str,
    dim: &'static str,
    green: &'static str,
    reset: &'static str,
    tick: &'static str,
}

static TTY: std::sync::OnceLock<Tty> = std::sync::OnceLock::new();

fn tty() -> &'static Tty {
    TTY.get_or_init(|| {
        let terminal =
            std::io::stdout().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb");
        let color = terminal && std::env::var_os("NO_COLOR").is_none();
        Tty {
            bold: if color { "\x1b[1m" } else { "" },
            dim: if color { "\x1b[2m" } else { "" },
            green: if color { "\x1b[32m" } else { "" },
            reset: if color { "\x1b[0m" } else { "" },
            tick: if terminal { "✓" } else { "+" },
        }
    })
}

fn field(label: &str, value: &str) {
    let t = tty();
    println!("  {}{label:<11}{} {value}", t.dim, t.reset);
}

fn ok(msg: String) {
    let t = tty();
    println!("  {}{}{} {msg}", t.green, t.tick, t.reset);
}

fn step(msg: String) {
    println!("  {msg} ...");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_strips_the_v_prefix() {
        assert_eq!(plain("v0.10.0"), "0.10.0");
        assert_eq!(plain("0.11.0-canary.1a2b3c"), "0.11.0-canary.1a2b3c");
    }

    #[test]
    fn canary_hash_is_the_commit_suffix() {
        assert_eq!(canary_hash("0.11.0-canary.1a2b3c"), Some("1a2b3c"));
        assert_eq!(canary_hash("0.11.0"), None);
    }

    #[test]
    fn stable_label_keeps_the_tag() {
        assert_eq!(
            channel_label(&Channel::Stable, Some("v0.10.0")),
            "stable (v0.10.0)"
        );
        assert_eq!(channel_label(&Channel::Stable, None), "stable (latest)");
    }

    #[test]
    fn canary_label_shows_the_hash() {
        assert_eq!(
            channel_label(&Channel::Canary, Some("0.11.0-canary.1a2b3c")),
            "canary (1a2b3c)"
        );
        assert_eq!(channel_label(&Channel::Canary, None), "canary");
    }

    #[test]
    fn checksum_matches_the_whole_asset_name_only() {
        let sums = "0000000000000000000000000000000000000000000000000000000000000000  apkscope_linux_musl_x86_64\n\
                    1111111111111111111111111111111111111111111111111111111111111111  apkscope_linux_x86_64\n";
        assert_eq!(
            expected_sha(sums, "apkscope_linux_x86_64").as_deref(),
            Some("1111111111111111111111111111111111111111111111111111111111111111")
        );
        // a prefix of another asset's name must not match, and vice versa
        assert_eq!(expected_sha(sums, "apkscope_linux"), None);
        assert_eq!(
            expected_sha(sums, "apkscope_linux_musl_x86_64").as_deref(),
            Some("0000000000000000000000000000000000000000000000000000000000000000")
        );
        // a two-space gap before the name, like sha256sum prints
        assert_eq!(
            expected_sha(&format!("{}  y", "a".repeat(64)), "y").as_deref(),
            Some("a".repeat(64).as_str())
        );
    }

    #[test]
    fn hex_is_lowercase_pairs() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }

    #[test]
    fn hsize_reads_like_the_installers() {
        assert_eq!(hsize(512), "512 B");
        assert_eq!(hsize(2048), "2 KB");
        assert_eq!(hsize(17_489_384), "16.7 MB");
    }

    #[test]
    fn asset_name_is_the_release_asset_of_this_platform() {
        let arch = std::env::consts::ARCH;
        let name = asset_name();
        if cfg!(target_os = "windows") {
            assert_eq!(name, format!("apkscope_windows_{arch}.exe"));
        } else if cfg!(target_os = "macos") {
            assert_eq!(name, format!("apkscope_macos_{arch}"));
        } else if cfg!(target_env = "musl") {
            assert_eq!(name, format!("apkscope_linux_musl_{arch}"));
        } else {
            assert_eq!(name, format!("apkscope_linux_{arch}"));
        }
    }
}
