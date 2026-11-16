use super::*;

#[test]
fn channel_names_are_canonical() {
    assert_eq!(Channel::Stable.name(), "stable");
    assert_eq!(Channel::Canary.name(), "canary");
    assert_eq!(Channel::parse("stable"), Some(Channel::Stable));
    assert_eq!(Channel::parse("canary"), Some(Channel::Canary));
    assert_eq!(Channel::parse("nightly"), None);
    assert_eq!(Channel::parse(""), None);
}

#[test]
fn channel_tags_point_at_the_release_urls() {
    // stable downloads from the `latest` redirect, canary from the moving tag
    assert_eq!(Channel::Stable.tag(), "latest");
    assert_eq!(Channel::Canary.tag(), "canary");
}

#[test]
fn a_local_build_identifies_as_dev() {
    // this test binary is a local build: no channel, no commit
    let me = identity();
    assert_eq!(me.version, env!("CARGO_PKG_VERSION"));
    // (a released CI build would carry Some(channel) and a full sha; here in
    // cargo test both are unset, which is exactly the dev case)
    if option_env!("APKSCOPE_BUILD_CHANNEL").is_none() {
        assert_eq!(me.channel, None);
        assert_eq!(me.commit, "");
        assert_eq!(current_label(&me), me.version);
    }
}

#[test]
fn current_label_appends_the_6_char_commit_for_canaries() {
    let me = Identity {
        version: "0.10.0",
        channel: Some(Channel::Canary),
        commit: "4c3eeb0123456789abcdef0123456789abcdef01",
    };
    assert_eq!(current_label(&me), "0.10.0-canary-4c3eeb");
}

#[test]
fn current_label_is_bare_for_stable() {
    let me = Identity {
        version: "0.10.0",
        channel: Some(Channel::Stable),
        commit: "0123456789abcdef0123456789abcdef01234567",
    };
    assert_eq!(current_label(&me), "0.10.0");
}

#[test]
fn channel_labels_read_like_the_installers() {
    let m = manifest::Manifest {
        channel: Channel::Stable,
        version: "0.10.0".into(),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        asset: manifest::Asset {
            platform: "test".into(),
            name: "apkscope_test".into(),
            size: 1,
            sha256: "0".repeat(64),
        },
    };
    assert_eq!(channel_label(Channel::Stable, &m), "stable (v0.10.0)");
    let m2 = manifest::Manifest {
        channel: Channel::Canary,
        commit: "4c3eeb0123456789abcdef0123456789abcdef01".into(),
        ..m
    };
    assert_eq!(channel_label(Channel::Canary, &m2), "canary (4c3eeb)");
}

#[test]
fn version_tags_are_spelled_with_a_v() {
    assert_eq!(tag_of_version("0.10.0").unwrap(), "v0.10.0");
    assert_eq!(tag_of_version("v0.10.0").unwrap(), "v0.10.0");
    assert!(tag_of_version("").is_err());
    assert!(tag_of_version("0.10.0-rc.1").is_err()); // prerelease: not via --version
    assert!(tag_of_version("../etc/passwd").is_err());
}

#[test]
fn homepath_folds_the_home_prefix() {
    // not under $HOME -> unchanged
    let p = std::path::Path::new("/opt/apkscope/bin/apkscope");
    assert_eq!(homepath(p), "/opt/apkscope/bin/apkscope");
}

/// The option matrix of `upgrade`: exercised through `upgrade()`'s argument
/// validation. The network path is not driven here (unit tests stay offline);
/// these cover the rejections the spec requires.
#[test]
fn incompatible_options_are_rejected() {
    // --stable and --canary together
    let err = upgrade(vec!["--stable".into(), "--canary".into()]).unwrap_err();
    assert!(err.contains("choose one channel"), "got: {err}");
    // --version with --canary
    let err = upgrade(vec!["--version".into(), "0.10.0".into(), "--canary".into()]).unwrap_err();
    assert!(
        err.contains("--version selects a stable release"),
        "got: {err}"
    );
    // --version with --stable
    let err = upgrade(vec!["--version".into(), "0.10.0".into(), "--stable".into()]).unwrap_err();
    assert!(
        err.contains("--version selects a stable release"),
        "got: {err}"
    );
    // unknown flags
    let err = upgrade(vec!["--nightly".into()]).unwrap_err();
    assert!(err.contains("usage:"), "got: {err}");
    // --version without a value
    let err = upgrade(vec!["--version".into()]).unwrap_err();
    assert!(err.contains("needs a value"), "got: {err}");
    // a bad version is rejected before any network access
    let err = upgrade(vec!["--version".into(), "abc".into()]).unwrap_err();
    assert!(err.contains("invalid version"), "got: {err}");
}
