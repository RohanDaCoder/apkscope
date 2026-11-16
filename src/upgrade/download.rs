//! Downloads through `curl` or `wget` — whichever the machine has — the same
//! tools the installers rely on, and the ones that put this binary here in the
//! first place. Linking an HTTP client would add new code to the binary for
//! something the OS already provides; the project's promise is one small
//! executable.
//!
//! Both tools stream to the output file, so a multi-megabyte binary never sits
//! in memory. `-L` follows GitHub's release-download redirects (to the CDN),
//! `-f` fails on HTTP errors instead of saving the error page, `--proto =https`
//! pins the whole chain (redirects included) to HTTPS, and `--max-time` bounds
//! the wait. Timeouts and retries keep a stalled CDN from hanging the upgrade.

use std::path::Path;
use std::process::{Command, Stdio};

/// How long a single transfer may take, in seconds.
const MAX_TIME: &str = "600";

/// Fetch `url` into `to`, with curl or wget (whichever exists), like the installers.
pub fn file(url: &str, to: &Path) -> Result<(), String> {
    match Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--max-time",
            MAX_TIME,
            "--retry",
            "2",
            url,
            "-o",
        ])
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
            .args([
                "-q",
                "--secure-protocol=auto",
                "--max-redirect=10",
                "--timeout=600",
                "--tries=2",
                url,
                "-O",
            ])
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

/// Fetch a small text file (the manifest) to a string, with curl or wget.
pub fn text(url: &str) -> Result<String, String> {
    match Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--max-time",
            "30",
            "--retry",
            "2",
            url,
        ])
        .stdin(Stdio::null())
        .output()
    {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => Err(format!(
            "curl exited with {}",
            o.status.code().unwrap_or(-1)
        )),
        Err(_) => match Command::new("wget")
            .args(["-q", "-O", "-", "--timeout=30", "--tries=2", url])
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
