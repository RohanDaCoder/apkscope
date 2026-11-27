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

#[test]
fn bodies_rewrite_whole_bodies() {
    let mut text = CLASS.to_string();
    let n = bodies(
        &mut text,
        &at(Some("()I")),
        ".locals 1
          const v0, 0x2a
          return v0",
    )
    .unwrap();
    assert_eq!(n, 1);
    // the XML's indentation is not the body's: lines are trimmed and re-indented to the
    // printer's four spaces
    assert!(
        text.contains(".locals 1\n    const v0, 0x2a\n    return v0"),
        "{text}"
    );
    assert!(!text.contains("0xb3b83"), "{text}");
    // without a sig, every overload of the name
    let mut text = CLASS.to_string();
    let n = bodies(&mut text, &at(None), ".locals 0\nreturn-void").unwrap();
    assert_eq!(n, 2);
    // no const of the old bodies is left in either of them
    assert!(!text.contains("const v0"), "{text}");
}

#[test]
fn a_body_must_say_how_many_registers_it_uses() {
    let mut text = CLASS.to_string();
    assert_eq!(
        bodies(&mut text, &at(None), "return v0").unwrap_err(),
        "at: the body of Ltest/Over;#uri must start with `.locals N` (or `.registers N`), \
         which says how many registers it uses"
    );
    // an empty body is a stub, and <return> without a value is that
    let mut text = CLASS.to_string();
    assert_eq!(
        bodies(&mut text, &at(None), "   \n").unwrap_err(),
        "at: the body of Ltest/Over;#uri is empty: <return> stubs a method to a constant, \
         and a method with no code at all is <return> without a value"
    );
    // a method that is not there is the same error as any other edit's
    let mut text = CLASS.to_string();
    assert_eq!(
        bodies(
            &mut text,
            &At {
                method: "nope".into(),
                ..at(None)
            },
            ".locals 0\nreturn-void"
        )
        .unwrap_err(),
        "at: Ltest/Over; has no method nope"
    );
    // abstract and native methods have no body for a new one to replace
    let mut text = BODYLESS.to_string();
    assert_eq!(
        bodies(&mut text, &at(None), ".locals 0\nreturn-void").unwrap_err(),
        "at: Ltest/Over;#uri is abstract: it has no body to edit"
    );
}

#[test]
fn a_constructor_body_must_keep_the_super_call() {
    let at = || At {
        class: "Ltest/Init;".into(),
        method: "<init>".into(),
        sig: Some("(I)V".into()),
        note: "at".into(),
    };
    // one that returns without it is a verify error Android refuses the class for
    let mut text = INITED.to_string();
    assert_eq!(
        bodies(&mut text, &at(), ".locals 0\nreturn-void").unwrap_err(),
        "at: the new body of Ltest/Init;#<init>(I)V never calls super() or this(...): a \
         constructor needs `invoke-direct {p0}, ...-><init>()` before it returns, or Android \
         refuses the class"
    );
    // an invoke-direct <init> on another register only builds an object; it is not the call
    let mut text = INITED.to_string();
    assert!(
        bodies(
            &mut text,
            &at(),
            ".locals 1\ninvoke-direct {v0}, Ljava/lang/Object;-><init>()V\nreturn-void"
        )
        .unwrap_err()
        .contains("never calls super()")
    );
    // a super() call is what the guard asks for, and the body lands
    let mut text = INITED.to_string();
    let n = bodies(
        &mut text,
        &at(),
        ".locals 1
invoke-direct {p0}, Ljava/lang/Object;-><init>()V
const/4 v0, 0x1
return-void",
    )
    .unwrap();
    assert_eq!(n, 1);
    assert!(
        text.contains(
            "invoke-direct {p0}, Ljava/lang/Object;-><init>()V\n    const/4 v0, 0x1\n    return-void"
        ),
        "{text}"
    );
    // a this(...) delegation counts: it is an invoke-direct <init> on p0 too
    let mut text = INITED.to_string();
    assert_eq!(
        bodies(
            &mut text,
            &at(),
            ".locals 0\ninvoke-direct {p0}, Ltest/Init;-><init>(I)V\nreturn-void"
        )
        .unwrap(),
        1
    );
}

#[test]
fn a_from_of_several_lines_replaces_the_block_whole() {
    // the (String)I body is `const v0, 0x2a` then `return v0`: both lines, as a block
    let mut text = CLASS.to_string();
    let n = replaces(
        &mut text,
        &at(Some("(Ljava/lang/String;)I")),
        "const v0, 0x2a
           return v0",
        "const/4 v0, 0x2
return v0",
        false,
    )
    .unwrap();
    assert_eq!(n, 2);
    assert!(
        text.contains(".locals 2\n    const/4 v0, 0x2\n    return v0"),
        "{text}"
    );
    assert!(!text.contains("0x2a"), "{text}");

    // to can be shorter than the block, or nothing at all: the block is replaced whole
    let mut text = CLASS.to_string();
    let n = replaces(
        &mut text,
        &at(Some("(Ljava/lang/String;)I")),
        "const v0, 0x2a\nreturn v0",
        "return v0",
        false,
    )
    .unwrap();
    assert_eq!(n, 2);
    assert!(
        text.contains(".locals 2\n    return v0\n.end method"),
        "{text}"
    );
    let mut text = CLASS.to_string();
    let n = replaces(
        &mut text,
        &at(Some("(Ljava/lang/String;)I")),
        "const v0, 0x2a\nreturn v0",
        "",
        false,
    )
    .unwrap();
    assert_eq!(n, 2);
    assert!(text.contains(".locals 2\n.end method"), "{text}");

    // a block no method has is an error, like a from no method has
    let mut text = CLASS.to_string();
    assert_eq!(
        replaces(
            &mut text,
            &at(Some("()I")),
            "const v0, 0x99\nreturn v0",
            "return v0",
            false
        )
        .unwrap_err(),
        "at: \"const v0, 0x99\\nreturn v0\" is not in Ltest/Over;#uri()I"
    );
}

#[test]
fn a_block_replace_once_takes_only_the_first_block() {
    let twice = CLASS.replace(
        "    .locals 1\n    const v0, 0xb3b83\n    return v0\n",
        "    .locals 1\n    const v0, 0x1\n    const v0, 0x2\n    const v0, 0x1\n    const v0, 0x2\n    return v0\n",
    );
    let mut text = twice.clone();
    let n = replaces(
        &mut text,
        &at(Some("()I")),
        "const v0, 0x1\nconst v0, 0x2",
        "const/4 v0, 0x3",
        true,
    )
    .unwrap();
    assert_eq!(n, 2);
    assert_eq!(text.matches("const/4 v0, 0x3").count(), 1, "{text}");
    assert_eq!(text.matches("const v0, 0x1").count(), 1, "{text}");
    let mut text = twice;
    let n = replaces(
        &mut text,
        &at(Some("()I")),
        "const v0, 0x1\nconst v0, 0x2",
        "const/4 v0, 0x3",
        false,
    )
    .unwrap();
    assert_eq!(n, 4);
    assert_eq!(text.matches("const v0, 0x1").count(), 0, "{text}");
    assert_eq!(text.matches("const/4 v0, 0x3").count(), 2, "{text}");
}

#[test]
fn line_shapes_mixed_wrong_are_refused() {
    let mut text = CLASS.to_string();
    assert_eq!(
        replaces(
            &mut text,
            &at(Some("()I")),
            "const v0",
            "const v0, 0x1\nnop",
            false
        )
        .unwrap_err(),
        "at: a from of one line needs a to of one line: 2 lines cannot replace a piece of \
         one; a from of several lines is what a to of several replaces"
    );
    let mut text = CLASS.to_string();
    assert_eq!(
        replaces(&mut text, &at(Some("()I")), "", "x", false).unwrap_err(),
        "at: <replace> from is empty"
    );
}
