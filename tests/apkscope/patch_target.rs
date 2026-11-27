use super::*;
use crate::patch::parse;

#[test]
fn a_target_check_says_what_did_not_match() {
    let t = |xml: &str| parse(xml).unwrap().target.unwrap();
    let ok = t("<bundle><target package=\"a\" versionName=\"2\" maxVersionCode=\"29\"/></bundle>");
    ok.matches(Some("a"), Some("2"), Some(21)).unwrap();
    assert_eq!(
        ok.matches(Some("b"), None, None).unwrap_err(),
        "the bundle is for a, the APK is b"
    );
    assert_eq!(
        ok.matches(Some("a"), Some("2.9"), None).unwrap_err(),
        "the bundle is for versionName \"2\", the APK has \"2.9\""
    );
    assert_eq!(
        ok.matches(Some("a"), Some("2"), Some(30)).unwrap_err(),
        "the bundle is for versionCode at most 29, the APK has 30"
    );
    let min = t("<bundle><target package=\"a\" minVersionCode=\"21\"/></bundle>");
    assert_eq!(
        min.matches(Some("a"), None, Some(20)).unwrap_err(),
        "the bundle is for versionCode at least 21, the APK has 20"
    );
    assert_eq!(
        min.matches(Some("a"), None, None).unwrap_err(),
        "the APK has no android:versionCode to check the bundle's <target> against"
    );
    assert_eq!(
        min.matches(None, None, None).unwrap_err(),
        "the APK has no package name to check the bundle's <target> against"
    );
}

#[test]
fn manifest_ids_are_read_out_of_decoded_text() {
    let (p, v, c) = manifest_id(
        "<manifest package=\"com.example.app\" android:versionCode=\"21\" android:versionName=\"2.1\">\n<application/>\n</manifest>\n",
    );
    assert_eq!(p.as_deref(), Some("com.example.app"));
    assert_eq!(v.as_deref(), Some("2.1"));
    assert_eq!(c, Some(21));
    assert_eq!(manifest_id("<application/>"), (None, None, None));
}
