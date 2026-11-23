use super::*;

#[test]
fn reads_the_abi_of_a_config_split() {
    assert_eq!(split_abi("config.arm64_v8a.apk"), Some("arm64-v8a"));
    assert_eq!(
        split_abi("splits/config.armeabi_v7a.apk"),
        Some("armeabi-v7a")
    );
    assert_eq!(split_abi("config.x86_64.apk"), Some("x86_64"));
    assert_eq!(split_abi("config.xxhdpi.apk"), None);
    assert_eq!(split_abi("config.en.apk"), None);
    assert_eq!(split_abi("base.apk"), None);
}

#[test]
fn picks_the_first_device_abi_the_app_ships() {
    let arm = vec!["arm64-v8a".to_string()];
    // x86_64 emulator with ARM translation: arm64 is in its list, after its own
    assert_eq!(
        pick_abi(&["x86_64", "x86", "arm64-v8a", "armeabi-v7a"], &arm).unwrap(),
        Some("arm64-v8a".into())
    );
    // both shipped: the device's own wins
    let both = vec!["arm64-v8a".to_string(), "x86_64".to_string()];
    assert_eq!(
        pick_abi(&["x86_64", "arm64-v8a"], &both).unwrap(),
        Some("x86_64".into())
    );
    // no native code: any ABI
    assert_eq!(pick_abi(&["x86_64"], &[]).unwrap(), None);
    // no translation: it can't run, and says why
    assert!(
        pick_abi(&["x86_64", "x86"], &arm)
            .unwrap_err()
            .contains("translation")
    );
}
