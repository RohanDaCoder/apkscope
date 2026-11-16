use super::*;
use crate::upgrade::Channel;
use crate::upgrade::manifest::{Asset, Manifest};
use crate::upgrade::verify::sha256_of;

/// A real fixture binary, compiled on the fly: answers `--version --json`
/// with the identity it was given — exactly the interface the updater checks.
/// (A shell script would work on Unix but not Windows; a compiled fixture is
/// the honest stand-in for a released executable everywhere.)
fn fixture_binary(
    dir: &std::path::Path,
    version: &str,
    channel: &str,
    commit: &str,
) -> std::path::PathBuf {
    let src = dir.join("fixture.rs");
    // plain raw string with @TOKENS@ (no format! brace escaping), substituted
    // below: the program prints one JSON identity line for --version --json
    let program = r#"fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a == ["--version".to_string(), "--json".to_string()] {
        println!("{{\"version\":\"@VERSION@\",\"channel\":\"@CHANNEL@\",\"commit\":\"@COMMIT@\"}}");
    } else if a == ["--version".to_string()] {
        println!("apkscope @VERSION@");
    }
}
"#
    .replace("@VERSION@", version)
    .replace("@CHANNEL@", channel)
    .replace("@COMMIT@", commit);
    std::fs::write(&src, program).unwrap();
    let out = dir.join(format!("fixture-{}-{}", channel, &commit[..6]));
    let status =
        std::process::Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .arg("--edition=2021")
            .status()
            .unwrap();
    assert!(status.success(), "fixture binary must compile");
    out
}

fn manifest_for(path: &std::path::Path, channel: Channel, version: &str, commit: &str) -> Manifest {
    Manifest {
        channel,
        version: version.into(),
        commit: commit.into(),
        asset: Asset {
            platform: "test".into(),
            name: "apkscope_test".into(),
            size: std::fs::metadata(path).unwrap().len(),
            sha256: sha256_of(path).unwrap(),
        },
    }
}

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("apkscope-inst-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fixture "installed executable": an earlier fixture standing in for the
/// real binary, so replacement and rollback run against files that exist and
/// execute.
fn installed(dir: &std::path::Path, fixture: &std::path::Path) -> std::path::PathBuf {
    let exe = dir.join("apkscope");
    std::fs::copy(fixture, &exe).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    exe
}

#[test]
fn beside_places_staging_files_next_to_the_executable() {
    let dir = tmpdir("beside");
    let exe = installed(
        &dir,
        &fixture_binary(&dir, "0.9.0", "stable", &"a".repeat(40)),
    );
    let staged = beside(&exe, ".apkscope-upgrade-new");
    assert_eq!(staged.parent(), exe.parent());
    assert!(
        staged
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("apkscope")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refuse_if_package_managed_leaves_normal_installs_alone() {
    let dir = tmpdir("pkg");
    let exe = installed(
        &dir,
        &fixture_binary(&dir, "0.9.0", "stable", &"a".repeat(40)),
    );
    // a plain temp-directory install is not package-manager owned; where
    // dpkg/rpm exist, a temp file is not in their database
    assert!(refuse_if_package_managed(&exe).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The full happy path: a new fixture with a matching manifest replaces the
/// old one, permissions are preserved, backups cleaned up.
#[test]
fn replace_verified_installs_a_verified_binary() {
    let dir = tmpdir("happy");
    let old = fixture_binary(&dir, "0.9.0", "stable", &"a".repeat(40));
    let new = fixture_binary(&dir, "0.10.0", "canary", &"b".repeat(40));
    let exe = installed(&dir, &old);
    let m = manifest_for(&new, Channel::Canary, "0.10.0", &"b".repeat(40));
    // stage a copy: replace_verified renames the staged file into place
    let staged = beside(&exe, ".apkscope-upgrade-new");
    std::fs::copy(&new, &staged).unwrap();

    replace_verified(&exe, &staged, &m, &m.asset).unwrap();

    // the installed binary is the new one, byte for byte
    assert_eq!(std::fs::read(&exe).unwrap(), std::fs::read(&new).unwrap());
    // and it is executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
    }
    // no leftovers: staged file moved, backup removed
    assert!(!beside(&exe, ".apkscope-upgrade-old").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A manifest that lies about the commit: the staged binary passes size and
/// digest but fails the identity check, and the installation is untouched.
#[test]
fn replace_verified_rejects_an_identity_mismatch() {
    let dir = tmpdir("identity");
    let old = fixture_binary(&dir, "0.9.0", "stable", &"a".repeat(40));
    let new = fixture_binary(&dir, "0.10.0", "canary", &"b".repeat(40));
    let exe = installed(&dir, &old);
    let before = std::fs::read(&exe).unwrap();
    let staged = beside(&exe, ".apkscope-upgrade-new");
    std::fs::copy(&new, &staged).unwrap();

    let mut m = manifest_for(&new, Channel::Canary, "0.10.0", &"b".repeat(40));
    m.commit = "f".repeat(40); // the lie
    let err = replace_verified(&exe, &staged, &m, &m.asset).unwrap_err();
    assert!(err.contains("different commit"), "got: {err}");
    assert_eq!(std::fs::read(&exe).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A corrupted download (wrong digest): rejected before anything is replaced.
#[test]
fn replace_verified_rejects_a_digest_mismatch() {
    let dir = tmpdir("digest");
    let old = fixture_binary(&dir, "0.9.0", "stable", &"a".repeat(40));
    let exe = installed(&dir, &old);
    let before = std::fs::read(&exe).unwrap();
    let staged = beside(&exe, ".apkscope-upgrade-new");
    std::fs::write(&staged, b"garbage, definitely not the asset").unwrap();

    let m = Manifest {
        channel: Channel::Stable,
        version: "0.10.0".into(),
        commit: "b".repeat(40),
        asset: Asset {
            platform: "test".into(),
            name: "apkscope_test".into(),
            size: std::fs::metadata(&staged).unwrap().len(),
            sha256: "0".repeat(64), // the mismatch
        },
    };
    let err = replace_verified(&exe, &staged, &m, &m.asset).unwrap_err();
    assert!(err.contains("SHA-256 mismatch"), "got: {err}");
    assert_eq!(std::fs::read(&exe).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A post-install failure must roll back: the installed binary is replaced by
/// one whose identity matches while staged, but something else when run from
/// the installed path. Simulated by making the destination directory's new
/// file answer differently — which cannot be faked with an honest fixture, so
/// instead this drives the rollback branch through a manifest whose channel
/// disagrees with the fixture only after install. The honest way to test
/// rollback is a fixture that changes behavior based on its own path:
#[test]
fn replace_verified_rolls_back_when_post_install_identity_fails() {
    let dir = tmpdir("rollback");
    // the "old" binary: a real fixture
    let old = fixture_binary(&dir, "0.9.0", "stable", &"a".repeat(40));
    // the "new" binary: reports the right identity only when invoked from a
    // path that is NOT the installed one (it checks its own exe path) — so the
    // staged check passes and the post-install check fails
    let src = dir.join("tricky.rs");
    let program = r#"fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a == ["--version".to_string(), "--json".to_string()] {
        let staged = !std::env::current_exe().unwrap().ends_with("apkscope");
        let commit = if staged { "@STAGED_COMMIT@" } else { "@BAD_COMMIT@" };
        println!("{{\"version\":\"0.10.0\",\"channel\":\"canary\",\"commit\":\"{}\"}}", commit);
    }
}
"#
    .replace("@STAGED_COMMIT@", &"b".repeat(40))
    .replace("@BAD_COMMIT@", &"c".repeat(40));
    std::fs::write(&src, program).unwrap();
    let new = dir.join("tricky");
    let status =
        std::process::Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
            .arg(&src)
            .arg("-o")
            .arg(&new)
            .arg("--edition=2021")
            .status()
            .unwrap();
    assert!(status.success());

    let exe = installed(&dir, &old);
    let before = std::fs::read(&exe).unwrap();
    let staged = beside(&exe, ".apkscope-upgrade-new");
    std::fs::copy(&new, &staged).unwrap();

    let m = Manifest {
        channel: Channel::Canary,
        version: "0.10.0".into(),
        commit: "b".repeat(40),
        asset: Asset {
            platform: "test".into(),
            name: "apkscope_test".into(),
            size: std::fs::metadata(&staged).unwrap().len(),
            sha256: sha256_of(&staged).unwrap(),
        },
    };
    let err = replace_verified(&exe, &staged, &m, &m.asset).unwrap_err();
    assert!(err.contains("failed verification"), "got: {err}");
    // the old binary is back, byte for byte
    assert_eq!(std::fs::read(&exe).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hsize_reads_like_the_installers() {
    assert_eq!(hsize(512), "512 B");
    assert_eq!(hsize(2048), "2 KB");
    assert_eq!(hsize(17_489_384), "16.7 MB");
}
