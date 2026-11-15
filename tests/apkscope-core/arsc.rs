use super::*;

/// change app_name in a real table, then read it back with the regular decoder
#[test]
fn real_table_edit() {
    let apk = "D:/APKS/Zestle_v0.6.8-prealpha.apk";
    if !std::path::Path::new(apk).exists() {
        return;
    }
    let d = crate::apk::open(apk)
        .unwrap()
        .read("resources.arsc")
        .unwrap();
    let all = strings(&d).unwrap();
    let name = all
        .iter()
        .find(|r| r.config.is_empty() && r.name == "app_name")
        .expect("app_name")
        .clone();
    let other = all
        .iter()
        .find(|r| r.config.is_empty() && r.name != "app_name")
        .unwrap()
        .clone();
    let mut ch = BTreeMap::new();
    ch.insert(
        (String::new(), "app_name".to_string()),
        "Zestle (apkscope) – ünïcode".to_string(),
    );
    let out = set_strings(&d, &ch).unwrap();
    let after = strings(&out).unwrap();
    assert_eq!(
        after
            .iter()
            .find(|r| r.config.is_empty() && r.name == "app_name")
            .unwrap()
            .value,
        "Zestle (apkscope) – ünïcode"
    );
    // everything else unchanged
    assert_eq!(
        after
            .iter()
            .find(|r| r.config == other.config && r.name == other.name)
            .unwrap()
            .value,
        other.value
    );
    assert_eq!(after.len(), all.len());
    // the whole table still reads
    let t = crate::table::read(&out).unwrap();
    assert!(!t.strings.strings.is_empty() && name.value != "Zestle (apkscope) – ünïcode");
}
