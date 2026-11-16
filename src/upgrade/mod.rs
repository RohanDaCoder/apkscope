//! `apkscope upgrade` — replace this binary with the latest release of a channel.
//!
//! Two channels, both distributed as plain GitHub Release assets:
//!
//! - **stable**: the newest versioned release, discovered through the
//!   `releases/latest/download` redirect, no API call involved.
//! - **canary**: the persistent prerelease on the moving `canary` tag, which
//!   CI republishes on every push to `main`.
//!
//! `apkscope-update.json`, attached to every release, is the single source of
//! update metadata: channel, package version, full build commit and, per
//! platform, the exact asset name, size and SHA-256. The updater downloads it
//! through the ordinary public release URL, validates it, downloads the one
//! asset that matches this platform, verifies size and digest, asks the staged
//! binary to identify itself and only then replaces the running executable —
//! with a rollback copy until the new binary has proven itself.
//!
//! The CLI never calls the GitHub REST or GraphQL API, never touches GitHub
//! Actions artifacts and never asks for a token: everything arrives through
//! public HTTPS release downloads, the same URLs a browser would use.
//!
//! Module layout (one responsibility each, see docs/UPDATE_SYSTEM.md):
//!
//! - [`self`]: build identity embedded in every official binary
//! - [`platform`]: OS, arch and libc detection → manifest platform ID
//! - [`manifest`]: the update manifest: schema and validation
//! - [`download`]: fetch a URL to disk with curl or wget, streaming
//! - [`verify`]: size, SHA-256 and staged-identity checks
//! - [`install`]: staged replacement, rollback, Windows helper

pub mod download;
pub mod install;
pub mod manifest;
pub mod platform;
pub mod verify;

use std::path::PathBuf;

/// What this build identifies as.
///
/// Every official binary carries three facts compiled in: the package version,
/// the channel it was released on, and the full commit it was built from
/// (`APKSCOPE_BUILD_CHANNEL` / `APKSCOPE_GIT_SHA`, set by the release workflow
/// for every matrix target). A local `cargo build` has none of them and says
/// `dev`, so it can never pose as a released build. A binary from before this
/// scheme existed (or a local one) reports no channel; upgrade defaults it to
/// stable, the documented fallback.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Stable,
    Canary,
}

/// The identity of the running binary: version, channel and full commit.
#[derive(Clone, Copy, Debug)]
pub struct Identity {
    pub version: &'static str,
    pub channel: Option<Channel>,
    pub commit: &'static str,
}

/// The [`Identity`] of this binary, compiled in.
pub fn identity() -> Identity {
    Identity {
        version: env!("CARGO_PKG_VERSION"),
        channel: match option_env!("APKSCOPE_BUILD_CHANNEL") {
            Some("stable") => Some(Channel::Stable),
            Some("canary") => Some(Channel::Canary),
            _ => None,
        },
        commit: option_env!("APKSCOPE_GIT_SHA").unwrap_or_default(),
    }
}

impl Channel {
    /// The lowercase name, as it appears in the manifest and in messages.
    pub fn name(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Canary => "canary",
        }
    }

    /// Parse a channel exactly as the manifest and the CLI spell it.
    pub fn parse(s: &str) -> Option<Channel> {
        match s {
            "stable" => Some(Channel::Stable),
            "canary" => Some(Channel::Canary),
            _ => None,
        }
    }

    /// The release tag this channel's assets live under: the moving `canary`
    /// tag, or `latest` — the redirect that GitHub points at the newest stable
    /// release, whose target tag need not be known to download from it.
    pub fn tag(self) -> &'static str {
        match self {
            Channel::Stable => "latest",
            Channel::Canary => "canary",
        }
    }
}

/// What the user asked `apkscope upgrade` for.
#[derive(Clone, Copy, Debug)]
enum Request {
    /// Stay on the channel the binary was installed from.
    Current,
    /// `--stable` / `--canary`: switch to a channel.
    Switch(Channel),
    /// `--version X.Y.Z`: that exact stable release.
    Version(&'static str),
}

/// The `apkscope upgrade` command; `args` is everything after `upgrade`.
pub fn upgrade(mut args: Vec<String>) -> Result<(), String> {
    let (stable, canary, check) = (
        crate::flag(&mut args, "--stable"),
        crate::flag(&mut args, "--canary"),
        crate::flag(&mut args, "--check"),
    );
    let version = crate::opt(&mut args, &["--version"])?;
    if !args.is_empty() {
        return Err(USAGE.into());
    }
    if stable && canary {
        return Err("choose one channel: --stable or --canary".into());
    }
    // --version picks a stable release, so it cannot combine with a channel
    // switch (and, implicitly, not with --canary either)
    if version.is_some() && (stable || canary) {
        return Err("--version selects a stable release; drop --stable/--canary".into());
    }
    let request = if let Some(v) = version {
        Request::Version(v.leak())
    } else if stable {
        Request::Switch(Channel::Stable)
    } else if canary {
        Request::Switch(Channel::Canary)
    } else {
        Request::Current
    };

    // channel: an explicit switch, the versioned-stable request, or what this
    // binary was released as; a binary without channel metadata (a local or
    // pre-upgrade build) falls back to stable, as documented
    let me = identity();
    let channel = match request {
        Request::Switch(c) => c,
        Request::Version(_) => Channel::Stable,
        Request::Current => me.channel.unwrap_or(Channel::Stable),
    };
    // the tag the manifest and asset come from: the moving canary tag, the
    // `latest` redirect, or the exact version tag asked for
    let tag = match &request {
        Request::Version(v) => tag_of_version(v)?,
        _ => channel.tag().to_string(),
    };

    let manifest = manifest::fetch(&channel, &tag, request_version(&request))?;
    let asset = manifest::select_asset(&manifest, &tag)?;

    // what the upgrade would change: stable by version, canary by commit
    let up_to_date = match channel {
        Channel::Stable => manifest.version == me.version,
        // canary compares by full commit; a local build (no commit) always updates
        Channel::Canary => !me.commit.is_empty() && manifest.commit == me.commit,
    };
    let label = channel_label(channel, &manifest);
    if up_to_date {
        println!(
            "apkscope {} is already the latest {label}",
            current_label(&me)
        );
        return Ok(());
    }
    println!("\n  {}apkscope upgrade{}\n", tty().bold, tty().reset);
    field("channel", &label);
    field("current", &current_label(&me));
    field("binary", &homepath(&exe()?));
    if let (Some(from), true) = (me.channel, me.channel != Some(channel)) {
        println!(
            "  note: switching from {} to {}",
            from.name(),
            channel.name()
        );
    }
    if check {
        println!("  update available; run apkscope upgrade to install it");
        return Ok(());
    }
    println!();

    let exe = exe()?;
    let url = download_url(&tag, &asset.name);
    let new = install::perform(&exe, &url, &manifest, asset, channel)?;
    println!();
    println!("  upgraded to {label}");
    if !new.is_empty() {
        field("now", &new);
    }
    println!();
    Ok(())
}

/// `apkscope upgrade --version 0.10.0` → `v0.10.0` (the `v` is how release
/// tags are spelled; accepting it on input too keeps the flag forgiving).
pub(crate) fn tag_of_version(v: &str) -> Result<String, String> {
    let v = v.strip_prefix('v').unwrap_or(v);
    if v.is_empty() || !v.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(format!("invalid version '{v}', expected like 0.10.0"));
    }
    Ok(format!("v{v}"))
}

/// The `version` the manifest must carry: a `--version` request pins it, any
/// other request accepts whatever the release says (the latest stable moves;
/// canary always carries the package version of its commit).
fn request_version(request: &Request) -> Option<String> {
    match request {
        Request::Version(v) => Some((*v).to_string()),
        _ => None,
    }
}

/// The path of this binary, resolved through the kernel's own table entry so a
/// symlinked or renamed invocation still updates the real file.
fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot find this binary's path: {e}"))
}

/// The repo releases are downloaded from, overridable for forks and mirrors.
fn repo() -> String {
    std::env::var("APKSCOPE_REPO").unwrap_or_else(|_| "suffixing/apkscope".into())
}

/// The GitHub host (or Enterprise / mirror base URL), like the installers.
fn base_url() -> String {
    std::env::var("APKSCOPE_BASE_URL").unwrap_or_else(|_| "https://github.com".into())
}

/// The one-line usage, shown for unknown options.
const USAGE: &str = "usage: apkscope upgrade [--stable | --canary] [--check] [--version X.Y.Z]";

/// `0.10.0` / `0.10.0-canary-4c3eeb` — how the running build is named in
/// messages; the revision suffix carries the 6-char commit, canary-style.
pub fn current_label(me: &Identity) -> String {
    match (me.channel, me.commit) {
        (Some(Channel::Canary), c) if c.len() >= 6 => {
            format!("{}-canary-{}", me.version, &c[..6])
        }
        _ => me.version.to_string(),
    }
}

/// The public download URL of one file of a release, through the ordinary
/// GitHub HTTPS redirect — the sole place these addresses are spelled, for
/// the manifest and the asset alike.
///
/// The two tag spellings are genuinely different addresses, not synonyms:
///
/// - `latest` is not a tag but a redirect GitHub resolves to the newest
///   stable release, and it lives at `releases/latest/download/...`.
///   `releases/download/latest/...` is a 404.
/// - every real tag (`canary`, `v0.10.0`) downloads at
///   `releases/download/<tag>/...`; `releases/tag/<tag>/download/...` is the
///   release's *page*, an HTML document, not the asset.
pub fn download_url(tag: &str, name: &str) -> String {
    let base = format!("{}/{}/releases", base_url(), repo());
    if tag == "latest" {
        format!("{base}/latest/download/{name}")
    } else {
        format!("{base}/download/{tag}/{name}")
    }
}

/// What the channel is shown as: `stable (v0.10.0)`, `canary (1a2b3c)` — the
/// installers print the same labels, so both sides of an update agree.
pub fn channel_label(channel: Channel, m: &manifest::Manifest) -> String {
    match channel {
        Channel::Stable => format!("stable (v{})", m.version),
        Channel::Canary => format!("canary ({})", &m.commit[..6]),
    }
}

/// Human-readable byte count, like the installers' `hsize`.
pub(crate) fn hsize(n: u64) -> String {
    if n >= 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{} B", n)
    }
}

/// Show a path under $HOME as `~/...`, like the installers.
pub(crate) fn homepath(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

// --- output helpers, matching the installers' look ------------------------------

/// Colors only on a terminal, off with NO_COLOR or TERM=dumb; the unicode tick
/// only where it renders (same rules as the installers).
pub(crate) struct Tty {
    pub bold: &'static str,
    pub dim: &'static str,
    pub green: &'static str,
    pub reset: &'static str,
    pub tick: &'static str,
}

pub(crate) static TTY: std::sync::OnceLock<Tty> = std::sync::OnceLock::new();

pub(crate) fn tty() -> &'static Tty {
    TTY.get_or_init(|| {
        use std::io::IsTerminal;
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

pub(crate) fn field(label: &str, value: &str) {
    let t = tty();
    println!("  {}{label:<11}{} {value}", t.dim, t.reset);
}

pub(crate) fn ok(msg: String) {
    let t = tty();
    println!("  {}{}{} {msg}", t.green, t.tick, t.reset);
}

pub(crate) fn step(msg: String) {
    println!("  {msg} ...");
}

#[cfg(test)]
#[path = "../../tests/apkscope/upgrade.rs"]
mod tests;
