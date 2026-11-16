use crate::upgrade::manifest::Manifest;
use crate::upgrade::{Channel, platform};

/// The manifest in docs/UPDATE_SYSTEM.md, complete and valid, as the baseline
/// every other test breaks on purpose.
fn base_manifest() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "channel": "canary",
        "version": "0.10.0",
        "commit": "0123456789abcdef0123456789abcdef01234567",
        "assets": {
            "linux-aarch64-gnu": {
                "name": "apkscope_linux_aarch64",
                "size": 12345678,
                "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            },
            "windows-x86_64": {
                "name": "apkscope_windows_x86_64.exe",
                "size": 5156938,
                "sha256": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
            }
        }
    })
}

fn parse(v: &serde_json::Value, channel: Channel, platform: &str) -> Result<Manifest, String> {
    super::parse(v, &channel, platform)
}

#[test]
fn valid_canary_manifest_parses() {
    let m = parse(&base_manifest(), Channel::Canary, "linux-aarch64-gnu").unwrap();
    assert_eq!(m.channel, Channel::Canary);
    assert_eq!(m.version, "0.10.0");
    assert_eq!(m.commit, "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(m.asset.name, "apkscope_linux_aarch64");
    assert_eq!(m.asset.size, 12345678);
    assert_eq!(
        m.asset.sha256,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    );
}

#[test]
fn valid_stable_manifest_parses() {
    let mut v = base_manifest();
    v["channel"] = "stable".into();
    let m = parse(&v, Channel::Stable, "windows-x86_64").unwrap();
    assert_eq!(m.channel, Channel::Stable);
    assert_eq!(m.asset.name, "apkscope_windows_x86_64.exe");
}

#[test]
fn unsupported_schema_version_is_rejected() {
    let mut v = base_manifest();
    v["schema_version"] = 2.into();
    let err = parse(&v, Channel::Canary, "linux-aarch64-gnu").unwrap_err();
    assert!(err.contains("schema version 2"), "got: {err}");
}

#[test]
fn missing_schema_version_is_rejected() {
    let mut v = base_manifest();
    v.as_object_mut().unwrap().remove("schema_version");
    assert!(parse(&v, Channel::Canary, "linux-aarch64-gnu").is_err());
}

#[test]
fn wrong_channel_is_rejected() {
    // a stable manifest must not answer a canary request, and vice versa
    let err = parse(&base_manifest(), Channel::Stable, "linux-aarch64-gnu").unwrap_err();
    assert!(err.contains("channel is canary"), "got: {err}");
}

#[test]
fn invalid_channel_string_is_rejected() {
    let mut v = base_manifest();
    v["channel"] = "nightly".into();
    assert!(parse(&v, Channel::Canary, "linux-aarch64-gnu").is_err());
}

#[test]
fn invalid_version_is_rejected() {
    for bad in ["", "0.10", "0.10.0.0-alpha", "x.10.0", "0.10.0-"] {
        let mut v = base_manifest();
        v["version"] = bad.into();
        let err = parse(&v, Channel::Canary, "linux-aarch64-gnu").unwrap_err();
        assert!(
            err.contains("invalid version"),
            "version '{bad}' gave: {err}"
        );
    }
    // valid: plain semver, and a prerelease suffix
    for good in ["0.10.0", "1.0.0-rc.1"] {
        let mut v = base_manifest();
        v["version"] = good.into();
        assert!(
            parse(&v, Channel::Canary, "linux-aarch64-gnu").is_ok(),
            "version '{good}' should parse"
        );
    }
}

#[test]
fn short_commit_is_rejected() {
    let mut v = base_manifest();
    v["commit"] = "0123456789abcdef0123456789abcdef0123456".into(); // 39 chars
    let err = parse(&v, Channel::Canary, "linux-aarch64-gnu").unwrap_err();
    assert!(err.contains("40-character"), "got: {err}");
}

#[test]
fn non_hex_commit_is_rejected() {
    let mut v = base_manifest();
    v["commit"] = "zz23456789abcdef0123456789abcdef01234567".into();
    assert!(parse(&v, Channel::Canary, "linux-aarch64-gnu").is_err());
}

#[test]
fn malformed_sha256_is_rejected() {
    let mut v = base_manifest();
    // 63 chars
    v["assets"]["linux-aarch64-gnu"]["sha256"] =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde".into();
    let err = parse(&v, Channel::Canary, "linux-aarch64-gnu").unwrap_err();
    assert!(err.contains("sha256"), "got: {err}");
    // non-hex characters are also rejected
    let mut v2 = base_manifest();
    v2["assets"]["linux-aarch64-gnu"]["sha256"] =
        "g123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde".into();
    assert!(parse(&v2, Channel::Canary, "linux-aarch64-gnu").is_err());
}

#[test]
fn invalid_asset_size_is_rejected() {
    let mut v = base_manifest();
    v["assets"]["linux-aarch64-gnu"]["size"] = 0.into();
    let err = parse(&v, Channel::Canary, "linux-aarch64-gnu").unwrap_err();
    assert!(err.contains("size"), "got: {err}");
    let mut v2 = base_manifest();
    v2["assets"]["linux-aarch64-gnu"]["size"] = (-5).into();
    assert!(parse(&v2, Channel::Canary, "linux-aarch64-gnu").is_err());
    let mut v3 = base_manifest();
    v3["assets"]["linux-aarch64-gnu"]
        .as_object_mut()
        .unwrap()
        .remove("size");
    assert!(parse(&v3, Channel::Canary, "linux-aarch64-gnu").is_err());
}

#[test]
fn missing_platform_asset_is_rejected() {
    // this machine's platform is not in the manifest
    let err = parse(&base_manifest(), Channel::Canary, "macos-x86_64").unwrap_err();
    assert!(err.contains("no asset"), "got: {err}");
    let mut v = base_manifest();
    v.as_object_mut().unwrap().remove("assets");
    assert!(parse(&v, Channel::Canary, "linux-aarch64-gnu").is_err());
}

#[test]
fn mismatched_asset_name_is_rejected() {
    // the entry claims another platform's file
    let mut v = base_manifest();
    v["assets"]["linux-aarch64-gnu"]["name"] = "apkscope_linux_musl_aarch64".into();
    let err = parse(&v, Channel::Canary, "linux-aarch64-gnu").unwrap_err();
    assert!(err.contains("does not match"), "got: {err}");
}

#[test]
fn path_traversal_in_asset_name_is_rejected() {
    let mut v = base_manifest();
    v["assets"]["linux-aarch64-gnu"]["name"] = "../../etc/passwd".into();
    assert!(parse(&v, Channel::Canary, "linux-aarch64-gnu").is_err());
    let mut v2 = base_manifest();
    v2["assets"]["linux-aarch64-gnu"]["name"] = "dist/apkscope_linux_aarch64".into();
    assert!(parse(&v2, Channel::Canary, "linux-aarch64-gnu").is_err());
}

#[test]
fn malformed_json_is_rejected() {
    let err = serde_json::from_str::<serde_json::Value>("{not json")
        .map_err(|e| e.to_string())
        .unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn every_documented_platform_id_has_one_canonical_asset_name() {
    for p in platform::ALL {
        let name = platform::asset_name(p).unwrap();
        assert!(name.starts_with("apkscope_"), "{p} -> {name}");
        assert!(!name.contains('/'), "{p} -> {name}");
        if p.starts_with("windows") {
            assert!(name.ends_with(".exe"), "{p} -> {name}");
        } else {
            assert!(!name.contains('.'), "{p} -> {name}");
        }
    }
}
