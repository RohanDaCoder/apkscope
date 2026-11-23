use super::*;

#[test]
fn core_keeps_a_dex_word_and_gives_up_on_generated_ones() {
    // a name the app holds narrows the search, lowercased
    assert_eq!(core("getDeviceId").as_deref(), Some("getdeviceid"));
    assert_eq!(core("x.getDeviceId(ctx)").as_deref(), Some("getdeviceid"));
    // local-name affixes are cut: `intentArr2` is `Intent` + `Arr` + `2`
    assert_eq!(core("pendingIntentArr2").as_deref(), Some("pendingintent"));
    assert_eq!(core("pendingIntentVal").as_deref(), Some("pendingintent"));
    // words the decompiler writes itself can't narrow
    assert_eq!(core("return"), None);
    assert_eq!(core("= null"), None);
    assert_eq!(core("obj2"), None);
    assert_eq!(core("AnonymousClass1"), None);
    // too short, or a number / hex / escape
    assert_eq!(core("ab"), None);
    assert_eq!(core("0xdeadbeef"), None);
    assert_eq!(core("\\u00e9"), None);
    assert_eq!(core(""), None);
}
