use super::*;

#[test]
fn smali_folders_follow_apktool() {
    assert_eq!(smali_folder("classes.dex"), "smali");
    assert_eq!(smali_folder("classes2.dex"), "smali_classes2");
    assert_eq!(smali_folder("classes12.dex"), "smali_classes12");
}

#[test]
fn smali_paths_are_safe_and_unique_without_case() {
    let mut used = HashSet::new();
    assert_eq!(
        smali_path("smali", "La/b/C;", &mut used),
        "smali/a/b/C.smali"
    );
    // obfuscated a and A would be one file on Windows and macOS
    assert_eq!(
        smali_path("smali", "La/b/c;", &mut used),
        "smali/a/b/c~2.smali"
    );
    assert_eq!(smali_path("smali", "LMain;", &mut used), "smali/Main.smali");
    // device names, characters Windows refuses, a trailing dot
    assert_eq!(safe_segment("con"), "_con");
    assert_eq!(safe_segment("COM1"), "_COM1");
    assert_eq!(safe_segment("a:b*c"), "a_b_c");
    assert_eq!(safe_segment("x."), "x._");
    assert_eq!(safe_segment("comet"), "comet");
}
