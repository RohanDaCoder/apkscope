use super::*;

#[test]
fn jni_names() {
    assert_eq!(
        jni_target("Java_com_example_Foo_bar"),
        Some(("com.example.Foo".into(), "bar".into()))
    );
    // _1 is a literal underscore, __ starts the overload signature
    assert_eq!(
        jni_target("Java_com_my_1app_Native_init__ILjava_lang_String_2"),
        Some(("com.my_app.Native".into(), "init".into()))
    );
    assert_eq!(jni_target("JNI_OnLoad"), None);
}

#[test]
fn string_runs() {
    assert_eq!(
        strings(b"\x00\x01https://a.b/c\x00ab\x00token=xyz", 6),
        vec!["https://a.b/c".to_string(), "token=xyz".to_string()]
    );
}

#[test]
fn rejects_non_elf() {
    assert!(
        parse(b"not an elf at all, just some bytes here.....................................")
            .is_err()
    );
}
