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

/// An attribute written twice is a typo: the XML is refused, not quietly read as whichever
/// of the two values comes first.
#[test]
fn refuses_an_attribute_written_twice() {
    assert_eq!(
        parse("<bundle name=\"a\" name=\"b\"/>").unwrap_err(),
        "line 1: <bundle> has attribute name twice"
    );
    assert_eq!(
        parse("<bundle><target package=\"a\" package=\"b\"/></bundle>").unwrap_err(),
        "line 1: <target> has attribute package twice"
    );
}

/// An entry name that is not a file path would add a broken entry to the APK that nothing on
/// Android can read: it is refused when the bundle is parsed, before anything runs.
#[test]
fn refuses_an_entry_name_that_is_not_a_file_path() {
    for name in ["", "assets/"] {
        let err = parse(&format!(
            r#"<bundle><patch name="A"><entry name="{name}" src="x"/></patch></bundle>"#
        ))
        .unwrap_err();
        assert!(err.contains("is not one"), "entry name {name:?}: {err}");
    }
}

#[test]
fn parses_body_and_any() {
    let b = parse(
        r#"<bundle>
  <patch name="A">
    <body class="a.B" method="run" sig="(I)Z">
      .locals 1
      const/4 v0, 0x1
      return v0
    </body>
    <any>
      <replace class="a.B" method="run" from="0x0" to="0x1"/>
      <return class="a.C" method="ok" value="true"/>
      <body class="a.D" method="go">.registers 1&#10;return-void</body>
    </any>
  </patch>
</bundle>"#,
    )
    .unwrap();
    // the body is the element's text, as written: the XML's indentation and all, trimmed
    // line by line when the op applies
    match &b.patches[0].ops[0] {
        Op::Body {
            class,
            method,
            sig,
            body,
            ..
        } => {
            assert_eq!(class, "a.B");
            assert_eq!(method, "run");
            assert_eq!(sig.as_deref(), Some("(I)Z"));
            assert_eq!(
                body,
                "\n      .locals 1\n      const/4 v0, 0x1\n      return v0\n    "
            );
        }
        _ => panic!("not a body"),
    }
    match &b.patches[0].ops[1] {
        Op::Any { ops, .. } => {
            assert_eq!(ops.len(), 3);
            assert!(matches!(ops[0], Op::Replace { .. }));
            assert!(matches!(ops[1], Op::Return { .. }));
            assert!(matches!(ops[2], Op::Body { .. }));
        }
        _ => panic!("not an any"),
    }
}

#[test]
fn a_body_can_be_cdata() {
    // smali strings can hold < and &, and a CDATA section says them without entities
    let b = parse(
        r#"<bundle><patch name="A"><body class="a.B" method="m"><![CDATA[.locals 1
const-string v0, "a<b & c"
return-void]]></body></patch></bundle>"#,
    )
    .unwrap();
    match &b.patches[0].ops[0] {
        Op::Body { body, .. } => {
            assert_eq!(body, ".locals 1\nconst-string v0, \"a<b & c\"\nreturn-void")
        }
        _ => panic!("not a body"),
    }
}

#[test]
fn refuses_body_and_any_written_wrong() {
    // a body holds smali text, not markup
    assert_eq!(
        parse("<bundle><patch name=\"x\"><body class=\"a\" method=\"m\"><wat/></body></patch></bundle>")
            .unwrap_err(),
        "line 1: <body> has nothing inside, found <wat>"
    );
    assert_eq!(
        parse("<bundle><patch name=\"x\"><body class=\"a\">.locals 0</body></patch></bundle>")
            .unwrap_err(),
        "line 1: <body> needs method=..."
    );
    // <any> of nothing, nested, with an attribute, or of ops that cannot be alternatives
    assert_eq!(
        parse("<bundle><patch name=\"x\"><any/></patch></bundle>").unwrap_err(),
        "line 1: <any> has no alternative inside: give it a <return>, <replace> or <body>"
    );
    assert_eq!(
        parse("<bundle><patch name=\"x\"><any><any><return class=\"a\" method=\"m\"/></any></any></patch></bundle>")
            .unwrap_err(),
        "line 1: <any> cannot sit inside another <any>; the alternatives inside it already are the either/or"
    );
    assert_eq!(
        parse("<bundle><patch name=\"x\"><any name=\"a\"><return class=\"a\" method=\"m\"/></any></patch></bundle>")
            .unwrap_err(),
        "line 1: <any> has no attribute name"
    );
    assert_eq!(
        parse(
            "<bundle><patch name=\"x\"><any><string name=\"s\">v</string></any></patch></bundle>"
        )
        .unwrap_err(),
        "line 1: <string> cannot be an alternative in <any>: <return>, <replace> and <body> can"
    );
}
