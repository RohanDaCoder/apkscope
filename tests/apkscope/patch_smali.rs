use super::*;

/// An `At` for the fixture class: `note` is what errors say.
fn at(sig: Option<&str>) -> At {
    At {
        class: "Ltest/Over;".into(),
        method: "uri".into(),
        sig: sig.map(str::to_string),
        note: "at".into(),
    }
}

const CLASS: &str = r#".class public Ltest/Over;
.super Ljava/lang/Object;

.method public uri()I
    .locals 1
    const v0, 0xb3b83
    return v0
.end method

.method public uri(Ljava/lang/String;)I
    .locals 2
    const v0, 0x2a
    return v0
.end method

.method public other()V
    .locals 0
    return-void
.end method
"#;

#[test]
fn methods_are_found_by_name_or_signature() {
    let lines: Vec<&str> = CLASS.lines().collect();
    assert_eq!(methods(&lines, "uri", None).len(), 2);
    assert_eq!(methods(&lines, "uri", Some("()I")).len(), 1);
    assert_eq!(
        methods(&lines, "uri", Some("(Ljava/lang/String;)I")).len(),
        1
    );
    // a longer name that starts with the one asked for is not it
    assert_eq!(methods(&lines, "ur", None).len(), 0);
    assert_eq!(methods(&lines, "other", None).len(), 1);
}

#[test]
fn returns_stubs_every_overload_of_a_name() {
    let mut text = CLASS.to_string();
    let n = returns(&mut text, &at(None), None).unwrap();
    assert_eq!(n, 2);
    assert_eq!(text.matches("const/4 v0, 0x0").count(), 2);
    assert_eq!(text.matches("return v0").count(), 2);
    assert!(!text.contains("0xb3b83"));
    // one signature only
    let mut text = CLASS.to_string();
    let n = returns(&mut text, &at(Some("(Ljava/lang/String;)I")), Some("5")).unwrap();
    assert_eq!(n, 1);
    assert!(text.contains("const/4 v0, 0x5"));
    // a method that is not there is an error, not a skip
    let mut text = CLASS.to_string();
    let miss = At {
        method: "nope".into(),
        ..at(None)
    };
    assert_eq!(
        returns(&mut text, &miss, None).unwrap_err(),
        "at: Ltest/Over; has no method nope"
    );
}

#[test]
fn replaces_swaps_text_in_a_method() {
    let mut text = CLASS.to_string();
    let n = replaces(&mut text, &at(Some("()I")), "0xb3b83", "0x195", false).unwrap();
    assert_eq!(n, 1);
    assert!(text.contains("const v0, 0x195"));
    // from that no method has is an error, not a skip
    let mut text = CLASS.to_string();
    let miss = At {
        method: "other".into(),
        sig: None,
        ..at(None)
    };
    assert_eq!(
        replaces(&mut text, &miss, "0xb3b83", "0x1", false).unwrap_err(),
        "at: \"0xb3b83\" is not in Ltest/Over;#other"
    );
}

/// A DEX with code for an `abstract` or `native` method is invalid: Android refuses the whole
/// APK. Both edits have to say so, not write a body under the flag.
const BODYLESS: &str = r#".class public interface abstract Ltest/NoBody;
.super Ljava/lang/Object;

.method public abstract uri()I
.end method

.method public static native uri(Ljava/lang/String;)I
.end method
"#;

#[test]
fn abstract_and_native_methods_have_no_body_to_edit() {
    let native = At {
        sig: Some("(Ljava/lang/String;)I".into()),
        ..at(None)
    };
    for (sig, flag) in [(None, "abstract"), (native.sig.clone(), "native")] {
        let mut text = BODYLESS.to_string();
        let want = format!(
            "at: Ltest/Over;#uri{} is {flag}: it has no body to edit",
            sig.clone().unwrap_or_default()
        );
        assert_eq!(
            returns(&mut text, &At { sig, ..at(None) }, None).unwrap_err(),
            want
        );
        let mut text = BODYLESS.to_string();
        assert_eq!(
            replaces(
                &mut text,
                &At {
                    sig: native.sig.clone(),
                    ..at(None)
                },
                "x",
                "y",
                false
            )
            .unwrap_err(),
            "at: Ltest/Over;#uri(Ljava/lang/String;)I is native: it has no body to edit"
        );
    }
}

#[test]
fn replaces_once_takes_only_the_first_line() {
    let twice = CLASS.replace(
        "    .locals 1\n    const v0, 0xb3b83\n",
        "    .locals 1\n    const v0, 0x1\n    const v0, 0x1\n",
    );
    let mut text = twice.clone();
    let n = replaces(
        &mut text,
        &at(Some("()I")),
        "const v0, 0x1",
        "const v0, 0x3",
        true,
    )
    .unwrap();
    assert_eq!(n, 1);
    assert_eq!(text.matches("const v0, 0x3").count(), 1);
    assert_eq!(text.matches("const v0, 0x1").count(), 1);
    let mut text = twice;
    let n = replaces(
        &mut text,
        &at(Some("()I")),
        "const v0, 0x1",
        "const v0, 0x3",
        false,
    )
    .unwrap();
    assert_eq!(n, 2);
    assert_eq!(text.matches("const v0, 0x1").count(), 0);
}

/// A class with a constructor that calls `super()` and a static initializer, as real classes
/// have: the constructor cannot be stubbed, the static initializer can.
const INITED: &str = r#".class public Ltest/Init;
.super Ljava/lang/Object;

.method public constructor <init>(I)V
    .locals 1
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method

.method static constructor <clinit>()V
    .locals 1
    const/4 v0, 0x1
    return-void
.end method
"#;

/// A stubbed constructor drops its `super()` call, and a constructor that returns without one
/// is a verify error Android refuses the class for: `<return>` on `<init>` is an error, not a
/// silent brick. `<clinit>` has no such rule and stubs fine.
#[test]
fn a_constructor_cannot_be_stubbed_but_clinit_can() {
    let at = |method: &str, sig: &str| At {
        class: "Ltest/Init;".into(),
        method: method.into(),
        sig: Some(sig.into()),
        note: "at".into(),
    };
    let mut text = INITED.to_string();
    assert_eq!(
        returns(&mut text, &at("<init>", "(I)V"), None).unwrap_err(),
        "at: Ltest/Init;#<init>(I)V is a constructor: a stub drops the super() call and \
         Android refuses the class; <replace> inside it instead"
    );
    // every constructor of the name is covered when no sig names one
    let mut text = INITED.to_string();
    assert_eq!(
        returns(
            &mut text,
            &At {
                sig: None,
                ..at("<init>", "(I)V")
            },
            None
        )
        .unwrap_err(),
        "at: Ltest/Init;#<init> is a constructor: a stub drops the super() call and Android \
         refuses the class; <replace> inside it instead"
    );
    // <replace> inside a constructor keeps the super() call: allowed
    let mut text = INITED.to_string();
    let at2 = At {
        method: "<init>".into(),
        ..at("<init>", "(I)V")
    };
    assert_eq!(
        replaces(&mut text, &at2, "return-void", "nop", false).unwrap(),
        1
    );
    // the static initializer is a plain void method to a stub
    let mut text = INITED.to_string();
    assert_eq!(returns(&mut text, &at("<clinit>", "()V"), None).unwrap(), 1);
    assert!(text.contains(".locals 0\n    return-void"));
}
