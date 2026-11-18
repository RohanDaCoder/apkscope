//! Platform detection: this machine's manifest platform ID and asset name.
//!
//! The manifest maps IDs like `linux-x86_64-musl` to exact asset filenames;
//! [`current()`] is this machine's entry in that map. The gnu/musl split is a
//! compile-time fact for the musl builds (they know their `target_env`), and a
//! runtime sniff for a glibc build — the same detection the shell installer
//! uses, so an installed binary and a fresh install pick the same asset.

/// This machine's platform ID, as it appears in the manifest's `assets` map.
pub fn current() -> &'static str {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "windows") {
        if arch == "aarch64" {
            "windows-aarch64"
        } else {
            "windows-x86_64"
        }
    } else if cfg!(target_os = "macos") {
        if arch == "aarch64" {
            "macos-aarch64"
        } else {
            "macos-x86_64"
        }
    } else if cfg!(target_env = "musl") {
        if arch == "aarch64" {
            "linux-aarch64-musl"
        } else {
            "linux-x86_64-musl"
        }
    } else if arch == "aarch64" {
        "linux-aarch64-gnu"
    } else {
        "linux-x86_64-gnu"
    }
}

/// The release asset filename a platform ID maps to — the exact names
/// `release.yml` uploads. Used to cross-check the manifest's `name` field, so a
/// manifest can never redirect a platform to some other file.
pub fn asset_name(platform: &str) -> Option<&'static str> {
    Some(match platform {
        "linux-x86_64-gnu" => "apkscope_linux_x86_64",
        "linux-aarch64-gnu" => "apkscope_linux_aarch64",
        "linux-x86_64-musl" => "apkscope_linux_musl_x86_64",
        "linux-aarch64-musl" => "apkscope_linux_musl_aarch64",
        "windows-x86_64" => "apkscope_windows_x86_64.exe",
        "windows-aarch64" => "apkscope_windows_aarch64.exe",
        "macos-aarch64" => "apkscope_macos_aarch64",
        "macos-x86_64" => "apkscope_macos_x86_64",
        _ => return None,
    })
}

/// The full list, in manifest order.
#[cfg(test)]
pub const ALL: [&str; 8] = [
    "linux-x86_64-gnu",
    "linux-aarch64-gnu",
    "linux-x86_64-musl",
    "linux-aarch64-musl",
    "windows-x86_64",
    "windows-aarch64",
    "macos-aarch64",
    "macos-x86_64",
];

/// True when a candidate downloaded for this platform plausibly is an
/// executable for it: the right magic number up front. A glibc/musl mixup or an
/// HTML error page caught by a redirect fails here, before any identity check.
pub fn plausible_executable(path: &std::path::Path) -> Result<(), String> {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|e| format!("cannot read the staged file: {e}"))?;
    let good = if cfg!(target_os = "windows") {
        // MZ, the DOS header every PE file starts with
        magic[..2] == *b"MZ"
    } else if cfg!(target_os = "macos") {
        // Mach-O 64: feedfacefeed (little-endian) or cefaedfacf (big-endian)
        magic == [0xcf, 0xfa, 0xed, 0xfe] || magic == [0xce, 0xfa, 0xed, 0xfc]
    } else {
        // ELF
        magic == [0x7f, b'E', b'L', b'F']
    };
    if good {
        Ok(())
    } else {
        Err("the staged file is not an executable for this platform".into())
    }
}
