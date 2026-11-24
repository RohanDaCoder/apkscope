use super::*;
#[cfg(unix)]
use crate::upgrade::Channel;
#[cfg(unix)]
use crate::upgrade::manifest::{Asset, Manifest};
use crate::upgrade::platform::plausible_executable;

#[cfg(unix)]
fn manifest(commit: &str, version: &str, channel: Channel) -> Manifest {
    Manifest {
        channel,
        version: version.into(),
        commit: commit.into(),
        asset: Asset {
            platform: "test".into(),
            name: "apkscope_test".into(),
            size: 1,
            sha256: "0".repeat(64),
        },
    }
}

#[test]
fn hex_is_lowercase_pairs() {
    assert_eq!(hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
}

#[test]
fn sha256_of_a_known_vector() {
    // sha256("abc")
    let dir = std::env::temp_dir().join(format!("apkscope-vfy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("known");
    std::fs::write(&f, b"abc").unwrap();
    assert_eq!(
        sha256_of(&f).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn staged_file_checks_size_and_digest() {
    let dir = std::env::temp_dir().join(format!("apkscope-stg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("staged");
    std::fs::write(&f, b"abc").unwrap();

    // correct size + digest pass (with the executable-header check exempted:
    // this is a fake file, so only the size/digest part is exercised through a
    // matching Asset; plausible_executable is covered by its own test)
    let good = Asset {
        platform: "test".into(),
        name: "apkscope_test".into(),
        size: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
    };
    // note: staged_file also enforces the platform header; a plain "abc" file
    // fails that, so this test drives the size/digest errors only — the exact
    // order is: size, digest, header

    let bad_size = Asset {
        size: 4,
        ..good.clone()
    };
    let err = staged_file(&f, &bad_size).unwrap_err();
    assert!(err.contains("size mismatch"), "got: {err}");

    let bad_sha = Asset {
        sha256: "ff".repeat(32),
        ..good
    };
    let err = staged_file(&f, &bad_sha).unwrap_err();
    assert!(err.contains("SHA-256 mismatch"), "got: {err}");

    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

#[cfg(unix)]
#[test]
fn staged_identity_of_a_script_that_lies_is_rejected() {
    let dir = std::env::temp_dir().join(format!("apkscope-id-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // a fake "binary" that answers --version --json with the wrong commit
    let f = dir.join("fake");
    std::fs::write(
        &f,
        "#!/bin/sh\necho '{\"version\":\"0.10.0\",\"channel\":\"canary\",\"commit\":\"ffffffffffffffffffffffffffffffffffffffff\"}'\n",
    )
    .unwrap();
    make_executable(&f);

    let m = manifest(
        "0123456789abcdef0123456789abcdef01234567",
        "0.10.0",
        Channel::Canary,
    );
    let err = staged_identity(&f, &m).unwrap_err();
    assert!(err.contains("different commit"), "got: {err}");

    // same fake binary, wrong channel
    let m2 = manifest(
        "ffffffffffffffffffffffffffffffffffffffff",
        "0.10.0",
        Channel::Stable,
    );
    let err = staged_identity(&f, &m2).unwrap_err();
    assert!(err.contains("channel mismatch"), "got: {err}");

    // and the one combination that matches
    let m3 = manifest(
        "ffffffffffffffffffffffffffffffffffffffff",
        "0.10.0",
        Channel::Canary,
    );
    staged_identity(&f, &m3).unwrap();

    // a version that disagrees
    let m4 = manifest(
        "ffffffffffffffffffffffffffffffffffffffff",
        "0.9.9",
        Channel::Canary,
    );
    let err = staged_identity(&f, &m4).unwrap_err();
    assert!(err.contains("version"), "got: {err}");

    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

#[cfg(unix)]
#[test]
fn staged_identity_of_a_failing_binary_is_rejected() {
    let dir = std::env::temp_dir().join(format!("apkscope-id2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("failing");
    std::fs::write(&f, "#!/bin/sh\nexit 3\n").unwrap();
    make_executable(&f);
    let m = manifest(
        "0123456789abcdef0123456789abcdef01234567",
        "0.10.0",
        Channel::Stable,
    );
    assert!(staged_identity(&f, &m).is_err());
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}

#[cfg(unix)]
fn make_executable(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn plausible_executable_accepts_this_platforms_magic() {
    let dir = std::env::temp_dir().join(format!("apkscope-mgc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // this test binary's own first bytes are by definition plausible
    let me = std::env::current_exe().unwrap();
    assert!(plausible_executable(&me).is_ok(), "{}", me.display());
    // a text file is not
    let f = dir.join("text");
    std::fs::write(&f, b"just text, definitely not an executable").unwrap();
    assert!(plausible_executable(&f).is_err());
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&dir);
}
