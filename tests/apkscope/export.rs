use super::*;

#[test]
fn class_paths_are_unique_and_safe() {
    let taken = Mutex::new(HashSet::new());
    assert_eq!(
        class_path("Lcom/a/B;", &taken),
        PathBuf::from("com/a/B.java")
    );
    assert_eq!(
        class_path("Lcom/a/b;", &taken),
        PathBuf::from("com/a/b_2.java")
    );
    assert_eq!(
        class_path("Lcon/Aux;", &taken),
        PathBuf::from("con_/Aux_.java")
    );
}

#[test]
fn manifest_fields() {
    let m = Manifest::from_xml(
        "<manifest package=\"a.b\" android:versionCode=\"7\">
<uses-sdk android:minSdkVersion=\"24\" android:targetSdkVersion=\"34\" />",
    );
    assert_eq!(
        (
            m.package.as_str(),
            m.min_sdk.as_str(),
            m.target_sdk.as_str(),
            m.version_code.as_str()
        ),
        ("a.b", "24", "34", "7")
    );
}
