use super::*;

/// The demo bundle: one of every element and attribute the format has.
const BUNDLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<bundle name="Unlock" author="rohan" version="3" description="the demo bundle">
  <target package="com.example.app" versionName="2.1" minVersionCode="21" maxVersionCode="29"/>
  <patch name="Unlock" description="license check off">
    <return class="com.example.app.License" method="isPurchased" sig="()Z" value="true"/>
    <return class="com.example.app.Config" method="level"/>
  </patch>
  <patch name="Remove ads">
    <replace class="com.example.app.AdLoader" method="load" from="const-string v0, &quot;ca-app-pub-394&quot;" to="const-string v0, &quot;0&quot;"/>
    <replace class="com.example.app.AdLoader" method="banner" from="sget" to="sput" once="yes"/>
  </patch>
  <patch name="Strings">
    <string name="app_name" config="night">Patched</string>
    <string name="promo">0</string>
  </patch>
  <patch name="Files">
    <manifest file="edited/AndroidManifest.xml"/>
    <entry name="assets/config.json" src="config.json"/>
    <entry name="META-INF/old.SF" delete="yes"/>
  </patch>
</bundle>"#;

#[test]
fn parses_every_op() {
    let b = parse(BUNDLE).unwrap();
    assert_eq!(b.name.as_deref(), Some("Unlock"));
    assert_eq!(b.author.as_deref(), Some("rohan"));
    assert_eq!(b.version.as_deref(), Some("3"));
    assert_eq!(b.description.as_deref(), Some("the demo bundle"));
    let t = b.target.as_ref().unwrap();
    assert_eq!(t.package, "com.example.app");
    assert_eq!(t.version_name.as_deref(), Some("2.1"));
    assert_eq!(t.min_version_code, Some(21));
    assert_eq!(t.max_version_code, Some(29));
    assert_eq!(b.patches.len(), 4);
    assert_eq!(b.patches[0].name, "Unlock");
    assert_eq!(
        b.patches[0].description.as_deref(),
        Some("license check off")
    );
    assert_eq!(b.patches[0].ops.len(), 2);
    match &b.patches[0].ops[0] {
        Op::Return {
            class,
            method,
            sig,
            value,
            ..
        } => {
            assert_eq!(class, "com.example.app.License");
            assert_eq!(method, "isPurchased");
            assert_eq!(sig.as_deref(), Some("()Z"));
            assert_eq!(value.as_deref(), Some("true"));
        }
        _ => panic!("not a return"),
    }
    match &b.patches[0].ops[1] {
        Op::Return { value, sig, .. } => {
            assert!(value.is_none());
            assert!(sig.is_none());
        }
        _ => panic!("not a return"),
    }
    match &b.patches[1].ops[0] {
        Op::Replace {
            class,
            from,
            to,
            once,
            ..
        } => {
            assert_eq!(class, "com.example.app.AdLoader");
            assert_eq!(from, "const-string v0, \"ca-app-pub-394\"");
            assert_eq!(to, "const-string v0, \"0\"");
            assert!(!once);
        }
        _ => panic!("not a replace"),
    }
    match &b.patches[1].ops[1] {
        Op::Replace { once, .. } => assert!(once),
        _ => panic!("not a replace"),
    }
    match &b.patches[2].ops[0] {
        Op::Str {
            name,
            config,
            value,
        } => {
            assert_eq!(name, "app_name");
            assert_eq!(config.as_deref(), Some("night"));
            assert_eq!(value, "Patched");
        }
        _ => panic!("not a string"),
    }
    match &b.patches[3].ops[0] {
        Op::Manifest { file } => assert_eq!(file, "edited/AndroidManifest.xml"),
        _ => panic!("not a manifest"),
    }
    match &b.patches[3].ops[1] {
        Op::Entry { name, src, delete } => {
            assert_eq!(name, "assets/config.json");
            assert_eq!(src.as_deref(), Some("config.json"));
            assert!(!delete);
        }
        _ => panic!("not an entry"),
    }
    match &b.patches[3].ops[2] {
        Op::Entry { name, src, delete } => {
            assert_eq!(name, "META-INF/old.SF");
            assert!(src.is_none());
            assert!(delete);
        }
        _ => panic!("not an entry"),
    }
}

#[test]
fn refuses_a_wrong_element() {
    assert_eq!(
        parse("<bundle><target package=\"a\"/><wat/></bundle>").unwrap_err(),
        "line 1: <bundle> has no <wat> inside"
    );
    assert_eq!(
        parse("<bundle><patch name=\"x\"><wat/></patch></bundle>").unwrap_err(),
        "line 1: <patch> has no <wat> inside"
    );
    assert_eq!(
        parse("<wat/>").unwrap_err(),
        "line 1: the root element is <bundle>, not <wat>"
    );
}

#[test]
fn refuses_an_unknown_attribute() {
    assert_eq!(
        parse("<bundle name=\"x\" wat=\"1\"/>").unwrap_err(),
        "line 1: <bundle> has no attribute wat"
    );
    assert_eq!(
        parse("<bundle><target package=\"a\" versionCode=\"1\"/></bundle>").unwrap_err(),
        "line 1: <target> has no attribute versionCode"
    );
}

#[test]
fn refuses_a_missing_attribute() {
    assert_eq!(
        parse("<bundle><target/></bundle>").unwrap_err(),
        "line 1: <target> needs package=..."
    );
    assert_eq!(
        parse("<bundle><patch><return class=\"a\" method=\"b\"/></patch></bundle>").unwrap_err(),
        "line 1: <patch> needs name=..."
    );
}

#[test]
fn refuses_an_entry_that_edits_and_deletes() {
    let err = parse(
        "<bundle><patch name=\"x\"><entry name=\"a\" src=\"f\" delete=\"yes\"/></patch></bundle>",
    )
    .unwrap_err();
    assert_eq!(err, "line 1: <entry> needs one of src=... or delete=yes");
    let err = parse("<bundle><patch name=\"x\"><entry name=\"a\"/></patch></bundle>").unwrap_err();
    assert_eq!(err, "line 1: <entry> needs one of src=... or delete=yes");
}

#[test]
fn refuses_two_patches_of_one_name() {
    let err =
        parse("<bundle><patch name=\"x\"/><patch name=\"x\" description=\"also x\"/></bundle>")
            .unwrap_err();
    assert_eq!(err, "line 1: two patches named \"x\"");
}

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

/// The stub text has to be smali apkscope itself assembles: a class holding just the stub is
/// parsed and written back.
#[test]
fn stubs_are_assemblable_smali() {
    for (ret, value) in [
        ("V", None),
        ("Z", Some("true")),
        ("Z", Some("false")),
        ("I", Some("7")),
        ("I", Some("-8")),
        ("I", Some("-9")),
        ("I", Some("-10")),
        ("I", Some("-100")),
        ("I", Some("-9000")),
        ("I", Some("0x7fffffff")),
        ("I", Some("-2147483648")),
        ("J", Some("42")),
        ("J", Some("-300")),
        ("J", Some("0x123456789")),
        ("J", Some("-9223372036854775808")),
        ("F", Some("1.5")),
        ("D", Some("2.5")),
        ("Ljava/lang/String;", Some("null")),
        ("[Ljava/lang/Object;", None),
    ] {
        let body = stub(ret, value, "t").unwrap_or_else(|e| panic!("{ret} {value:?}: {e}"));
        let class = format!(
            ".class public Ltest/T;\n.super Ljava/lang/Object;\n\n.method public t(){ret}\n{body}.end method\n"
        );
        let mut src = apkscope_core::smali::Source::default();
        let model = apkscope_core::smali::text::parse(&mut src, &class)
            .unwrap_or_else(|e| panic!("{ret} {value:?}: {e}\n{class}"));
        let printed = apkscope_core::smali::text::print(&src, &model);
        for want in body.lines().filter(|l| !l.trim().is_empty()) {
            assert!(
                printed.contains(want),
                "{ret} {value:?}: {want:?} is not in\n{printed}"
            );
        }
    }
}

#[test]
fn a_stub_matches_its_return_type() {
    assert_eq!(
        stub("V", None, "t").unwrap(),
        "    .locals 0\n    return-void\n"
    );
    assert_eq!(
        stub("Z", Some("true"), "t").unwrap(),
        "    .locals 1\n    const/4 v0, 0x1\n    return v0\n"
    );
    assert_eq!(
        stub("J", Some("0x123456789"), "t").unwrap(),
        "    .locals 2\n    const-wide v0, 0x123456789\n    return-wide v0\n"
    );
    assert_eq!(
        stub("Ljava/lang/String;", Some("null"), "t").unwrap(),
        "    .locals 1\n    const/4 v0, 0x0\n    return-object v0\n"
    );
    // a negative value is hex after the minus, not decimal: -100 is -0x64, never -0x100 (-256)
    assert_eq!(
        stub("I", Some("-100"), "t").unwrap(),
        "    .locals 1\n    const/16 v0, -0x64\n    return v0\n"
    );
    assert_eq!(
        stub("I", Some("-9000"), "t").unwrap(),
        "    .locals 1\n    const/16 v0, -0x2328\n    return v0\n"
    );
    assert_eq!(
        stub("I", Some("-2147483648"), "t").unwrap(),
        "    .locals 1\n    const v0, -0x80000000\n    return v0\n"
    );
    assert_eq!(
        stub("J", Some("-300"), "t").unwrap(),
        "    .locals 2\n    const-wide/16 v0, -0x12c\n    return-wide v0\n"
    );
    // values the type cannot hold, and types smali does not have
    assert!(stub("V", Some("0"), "t").is_err());
    assert!(stub("Z", Some("1"), "t").is_err());
    assert!(stub("I", Some("x"), "t").is_err());
    assert!(stub("C", Some("-1"), "t").is_err());
    assert!(stub("B", Some("128"), "t").is_err());
    assert!(stub("X", None, "t").is_err());
    assert!(stub("Ljava/lang/String;", Some("yes"), "t").is_err());
}

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
    // and the parse side: an entry that is not a file path is refused before anything runs
    for name in ["", "assets/"] {
        let err = parse(&format!(
            r#"<bundle><patch name="A"><entry name="{name}" src="x"/></patch></bundle>"#
        ))
        .unwrap_err();
        assert!(err.contains("is not one"), "entry name {name:?}: {err}");
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
