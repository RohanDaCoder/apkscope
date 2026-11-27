use super::*;

/// The APK the bundles run on: a manifest, one string resource and two classes, built the way
/// the apkscope tools themselves build one (smali -> DEX, text XML -> binary, table -> arsc).
fn fixture(dir: &std::path::Path) {
    use apkscope_core::table::{
        Chunk, Entry, EntryValue, Package, Spec, StringPool, Table, Type, Value, config,
    };
    use std::io::Write;

    let pool = |strings: &[&str]| StringPool {
        utf8: true,
        sorted: false,
        strings: strings.iter().map(|s| s.to_string()).collect(),
        styles: vec![],
    };
    let string = |i| {
        Some(Entry {
            flags: 0,
            key: 0,
            value: EntryValue::Simple(Value {
                ty: Value::STRING,
                data: i,
            }),
        })
    };
    let table = Table {
        strings: pool(&["Example app"]),
        packages: vec![Package {
            id: 0x7f,
            name: "com.example.app".into(),
            last_public_type: 0,
            last_public_key: 0,
            type_id_offset: 0,
            types: pool(&["string"]),
            keys: pool(&["app_name"]),
            chunks: vec![
                Chunk::Spec(Spec {
                    id: 1,
                    res0: 0,
                    res1: 2,
                    flags: vec![config::bits::UI_MODE],
                }),
                Chunk::Type(Type {
                    id: 1,
                    flags: 0,
                    reserved: 0,
                    config: config::default(),
                    entries: vec![string(0)],
                }),
            ],
        }],
    };
    let manifest = apkscope_core::axml::encode(
        r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.example.app" android:versionCode="21" android:versionName="2.1"/>"#,
        None,
        &apkscope_core::res::Names::new(&table),
    )
    .unwrap();
    let license = r#"
.class public Lcom/example/app/License;
.super Ljava/lang/Object;

.method public isPurchased()Z
    .locals 1
    const/4 v0, 0x0
    return v0
.end method

.method public isPurchased(Ljava/lang/String;)Z
    .locals 1
    const/4 v0, 0x0
    return v0
.end method
"#;
    let ads = r#"
.class public Lcom/example/app/AdLoader;
.super Ljava/lang/Object;

.method public constructor <init>()V
    .locals 1
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method

.method public load(Ljava/lang/String;)V
    .locals 2
    const-string v0, "ca-app-pub-394"
    const/4 v1, 0x1
    return-void
.end method
"#;
    let dex = apkscope_core::smali::assemble(&[license, ads], *b"035").unwrap();
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    z.start_file("AndroidManifest.xml", stored).unwrap();
    z.write_all(&manifest).unwrap();
    z.start_file("resources.arsc", stored).unwrap();
    z.write_all(&apkscope_core::table::write(&table)).unwrap();
    z.start_file("classes.dex", stored).unwrap();
    z.write_all(&dex).unwrap();
    std::fs::write(dir.join("app.apk"), z.finish().unwrap().into_inner()).unwrap();
}

fn args(bundle: &str, input: &str, output: &str) -> PatchArgs {
    PatchArgs {
        bundle: bundle.into(),
        input: Some(input.into()),
        output: Some(output.into()),
        list: false,
        only: vec![],
        skip: vec![],
        sign: crate::cli::SignArgs {
            key: None,
            cert: None,
            ks: None,
            ks_pass: None,
            ks_key_pass: None,
            ks_alias: None,
            no_sign: true,
        },
    }
}

fn smali_of(path: &str, class: &str) -> String {
    apkscope_core::smali::export_class(path, class).unwrap()
}

#[test]
fn applies_a_bundle_end_to_end() {
    let dir = std::env::temp_dir().join(format!("apkscope-patch-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    fixture(&dir);
    let apk = dir.join("app.apk").to_str().unwrap().to_string();
    let out = dir.join("out.apk").to_str().unwrap().to_string();
    std::fs::write(dir.join("config.json"), br#"{"patched": true}"#).unwrap();
    // the shipped example, exactly as a user would run it
    let bundle = dir.join("bundle.xml");
    std::fs::write(&bundle, include_str!("../../assets/demo.xml")).unwrap();
    patch(&args(bundle.to_str().unwrap(), &apk, &out)).unwrap();

    // the ()Z overload now returns true, and the (String)Z one carries the body the bundle
    // wrote for it: no stub line is left, and the new code is in the class for real
    let license = smali_of(&out, "com.example.app.License");
    assert_eq!(license.matches("const/4 v0, 0x1").count(), 1, "{license}");
    assert_eq!(license.matches("const/4 v0, 0x0").count(), 0, "{license}");
    assert!(
        license.contains(r#"const-string v0, "patched""#)
            && license.contains("->startsWith(Ljava/lang/String;)Z")
            && license.contains("move-result v0"),
        "the <body> must land as written:\n{license}"
    );
    let ads = smali_of(&out, "com.example.app.AdLoader");
    // the ad unit id is emptied — by the <any> alternative that found it, the other class
    // is not in the app — and the block replace changed the line after it
    assert!(ads.contains(r#"const-string v0, "0""#), "{ads}");
    assert!(ads.contains("const/4 v1, 0x0"), "{ads}");
    // the constructor is untouched
    assert!(ads.matches("invoke-direct {p0}").count() == 1, "{ads}");

    // the string resource and the file entry landed
    let before = crate::quick::read_entry(&apk, "resources.arsc")
        .unwrap()
        .unwrap();
    let after = crate::quick::read_entry(&out, "resources.arsc")
        .unwrap()
        .unwrap();
    assert_ne!(before, after);
    assert_eq!(
        crate::quick::read_entry(&out, "assets/config.json")
            .unwrap()
            .unwrap(),
        br#"{"patched": true}"#.to_vec()
    );

    // a bundle for another app is refused before anything is written
    let other = dir.join("other.xml");
    std::fs::write(
        &other,
        br#"<bundle><target package="com.other.app"/><patch name="x"><return class="com.example.app.License" method="isPurchased" value="true"/></patch></bundle>"#,
    )
    .unwrap();
    let refused = dir.join("refused.apk");
    let err = patch(&args(
        other.to_str().unwrap(),
        &apk,
        refused.to_str().unwrap(),
    ))
    .unwrap_err();
    assert_eq!(
        err,
        "the bundle is for com.other.app, the APK is com.example.app"
    );
    assert!(!refused.exists());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_patch_that_did_not_land_is_an_error_and_writes_nothing() {
    let dir = std::env::temp_dir().join(format!("apkscope-patch-miss-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    fixture(&dir);
    let apk = dir.join("app.apk").to_str().unwrap().to_string();
    let out = dir.join("out.apk").to_str().unwrap().to_string();
    let bundle = dir.join("miss.xml");
    std::fs::write(
        &bundle,
        br#"<bundle><patch name="Nope"><return class="com.example.app.Missing" method="gone" value="true"/></patch></bundle>"#,
    )
    .unwrap();
    let err = patch(&args(bundle.to_str().unwrap(), &apk, &out)).unwrap_err();
    assert_eq!(
        err,
        "Nope: <return com.example.app.Missing#gone> (line 1): class com.example.app.Missing not found"
    );
    assert!(!std::path::Path::new(&out).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_input_is_named_as_such() {
    // the input is opened before anything else runs: a missing APK is "no such file",
    // not a later "no AndroidManifest.xml" from the <target> check that reads it first
    let dir = std::env::temp_dir().join(format!("apkscope-patch-gone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("gone.apk").to_str().unwrap().to_string();
    let out = dir.join("out.apk").to_str().unwrap().to_string();
    let bundle = dir.join("b.xml");
    std::fs::write(
        &bundle,
        br#"<bundle><patch name="A"><return class="com.example.app.License" method="isPurchased" value="true"/></patch></bundle>"#,
    )
    .unwrap();
    let err = patch(&args(bundle.to_str().unwrap(), &missing, &out)).unwrap_err();
    assert!(err.contains("gone.apk: No such file"), "{err}");
    assert!(!std::path::Path::new(&out).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn two_spellings_of_one_class_edit_the_same_text() {
    let dir = std::env::temp_dir().join(format!("apkscope-patch-alias-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    fixture(&dir);
    let apk = dir.join("app.apk").to_str().unwrap().to_string();
    let out = dir.join("out.apk").to_str().unwrap().to_string();
    // one class, two spellings: each op must land on the same printed smali, not on two
    // copies of which the second write quietly drops the first's edits
    let bundle = dir.join("alias.xml");
    std::fs::write(
        &bundle,
        br#"<bundle>
  <patch name="A"><return class="com.example.app.License" method="isPurchased" sig="()Z" value="true"/></patch>
  <patch name="B"><return class="Lcom/example/app/License;" method="isPurchased" sig="(Ljava/lang/String;)Z" value="true"/></patch>
</bundle>"#,
    )
    .unwrap();
    patch(&args(bundle.to_str().unwrap(), &apk, &out)).unwrap();
    let license = smali_of(&out, "com.example.app.License");
    assert_eq!(
        license.matches("const/4 v0, 0x1").count(),
        2,
        "both spellings' edits must land:\n{license}"
    );
    assert_eq!(license.matches("const/4 v0, 0x0").count(), 0, "{license}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn only_and_skip_pick_patches_and_list_shows_them() {
    let dir = std::env::temp_dir().join(format!("apkscope-patch-pick-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    fixture(&dir);
    let apk = dir.join("app.apk").to_str().unwrap().to_string();
    let bundle = dir.join("pick.xml");
    std::fs::write(
        &bundle,
        br#"<bundle name="Pick">
  <patch name="Unlock"><return class="com.example.app.License" method="isPurchased" sig="()Z" value="true"/></patch>
  <patch name="Strings"><string name="app_name">Patched</string></patch>
</bundle>"#,
    )
    .unwrap();
    // --skip: the string resource is untouched, the smali patch still lands
    let out = dir.join("skip.apk").to_str().unwrap().to_string();
    let mut a = args(bundle.to_str().unwrap(), &apk, &out);
    a.skip = vec!["Strings".into()];
    patch(&a).unwrap();
    assert_eq!(
        crate::quick::read_entry(&apk, "resources.arsc")
            .unwrap()
            .unwrap(),
        crate::quick::read_entry(&out, "resources.arsc")
            .unwrap()
            .unwrap()
    );
    assert!(smali_of(&out, "com.example.app.License").contains("const/4 v0, 0x1"));

    // --only: the same, the other way round
    let out = dir.join("only.apk").to_str().unwrap().to_string();
    let mut a = args(bundle.to_str().unwrap(), &apk, &out);
    a.only = vec!["Strings".into()];
    patch(&a).unwrap();
    assert!(smali_of(&out, "com.example.app.License").contains("const/4 v0, 0x0"));
    assert_ne!(
        crate::quick::read_entry(&apk, "resources.arsc")
            .unwrap()
            .unwrap(),
        crate::quick::read_entry(&out, "resources.arsc")
            .unwrap()
            .unwrap()
    );

    // a name that is not there is an error, not an empty run
    let mut a = args(bundle.to_str().unwrap(), &apk, &out);
    a.only = vec!["Nope".into()];
    let err = patch(&a).unwrap_err();
    assert_eq!(err, "no patch named \"Nope\" to apply");

    // --list needs no APK and writes nothing
    let mut a = args(bundle.to_str().unwrap(), &apk, "");
    a.input = None;
    a.output = None;
    a.list = true;
    patch(&a).unwrap();
    assert!(!dir.join("out.apk").exists());

    std::fs::remove_dir_all(&dir).unwrap();
}

/// A `<body>` that does not assemble is an error that names the class and the line of the
/// smali it choked on, and nothing is written: the assembler is the check on every body.
#[test]
fn a_body_the_assembler_rejects_writes_nothing() {
    let dir = std::env::temp_dir().join(format!("apkscope-patch-asm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    fixture(&dir);
    let apk = dir.join("app.apk").to_str().unwrap().to_string();
    let out = dir.join("out.apk").to_str().unwrap().to_string();
    let bundle = dir.join("b.xml");
    std::fs::write(
        &bundle,
        br#"<bundle><patch name="A"><body class="com.example.app.License" method="isPurchased">
  .locals 1
  frobnicate v0
  return v0
</body></patch></bundle>"#,
    )
    .unwrap();
    let err = patch(&args(bundle.to_str().unwrap(), &apk, &out)).unwrap_err();
    assert!(
        err.contains("Lcom/example/app/License;") && err.contains("unknown instruction"),
        "{err}"
    );
    assert!(!std::path::Path::new(&out).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A constructor body keeps its `super()` call or the run is refused: a `<init>` that
/// returns without one is a verify error Android refuses the whole class for.
#[test]
fn a_constructor_body_without_the_super_call_is_refused() {
    let dir = std::env::temp_dir().join(format!("apkscope-patch-init-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    fixture(&dir);
    let apk = dir.join("app.apk").to_str().unwrap().to_string();
    let out = dir.join("out.apk").to_str().unwrap().to_string();
    let bundle = dir.join("b.xml");
    std::fs::write(
        &bundle,
        br#"<bundle><patch name="A"><body class="com.example.app.AdLoader" method="&lt;init&gt;">
  .locals 0
  return-void
</body></patch></bundle>"#,
    )
    .unwrap();
    let err = patch(&args(bundle.to_str().unwrap(), &apk, &out)).unwrap_err();
    assert!(err.contains("never calls super() or this(...)"), "{err}");
    assert!(!std::path::Path::new(&out).exists());

    // with the call kept, the same body lands
    std::fs::write(
        &bundle,
        br#"<bundle><patch name="A"><body class="com.example.app.AdLoader" method="&lt;init&gt;">
  .locals 1
  invoke-direct {p0}, Ljava/lang/Object;->&lt;init&gt;()V
  const/4 v0, 0x0
  return-void
</body></patch></bundle>"#,
    )
    .unwrap();
    patch(&args(bundle.to_str().unwrap(), &apk, &out)).unwrap();
    let ads = smali_of(&out, "com.example.app.AdLoader");
    assert!(
        ads.contains("invoke-direct {p0}, Ljava/lang/Object;-><init>()V\n    const/4 v0, 0x0"),
        "{ads}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `<any>` with no alternative landing is an error that says why each one missed: a strict
/// either/or, not a maybe-patch. One landing is applied and the rest never run.
#[test]
fn any_that_lands_on_nothing_says_why_and_writes_nothing() {
    let dir = std::env::temp_dir().join(format!("apkscope-patch-any-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    fixture(&dir);
    let apk = dir.join("app.apk").to_str().unwrap().to_string();
    let out = dir.join("out.apk").to_str().unwrap().to_string();
    let bundle = dir.join("b.xml");
    std::fs::write(
        &bundle,
        br#"<bundle><patch name="Drift">
  <any>
    <replace class="com.example.app.Gone" method="unit" from="a" to="b"/>
    <replace class="com.example.app.AdLoader" method="load" from="0xdead" to="0x0"/>
  </any>
</patch></bundle>"#,
    )
    .unwrap();
    let err = patch(&args(bundle.to_str().unwrap(), &apk, &out)).unwrap_err();
    assert!(
        err.starts_with("Drift: <any> (line 2): no alternative landed"),
        "{err}"
    );
    assert!(err.contains("alternative 1 of 2"), "{err}");
    assert!(
        err.contains("class com.example.app.Gone not found"),
        "{err}"
    );
    assert!(err.contains("alternative 2 of 2"), "{err}");
    assert!(err.contains("\"0xdead\" is not in"), "{err}");
    assert!(!std::path::Path::new(&out).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A `<replace>` of several lines swaps the block whole, the line count of `to` free — and
/// a one-line `from` cannot take a several-line `to`: where the rest of the line would go
/// is not a thing the text says.
#[test]
fn a_block_replace_edits_blocks_and_refuses_line_shape_mixes() {
    let dir = std::env::temp_dir().join(format!("apkscope-patch-block-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    fixture(&dir);
    let apk = dir.join("app.apk").to_str().unwrap().to_string();
    let out = dir.join("out.apk").to_str().unwrap().to_string();
    let bundle = dir.join("b.xml");

    // a to of more lines than the from is fine: the block is replaced whole
    std::fs::write(
        &bundle,
        br#"<bundle><patch name="A"><replace class="com.example.app.AdLoader" method="load"
  from="const-string v0, &quot;ca-app-pub-394&quot;&#10;const/4 v1, 0x1"
  to="const-string v0, &quot;0&quot;&#10;const/4 v1, 0x0&#10;nop"/></patch></bundle>"#,
    )
    .unwrap();
    patch(&args(bundle.to_str().unwrap(), &apk, &out)).unwrap();
    let ads = smali_of(&out, "com.example.app.AdLoader");
    assert!(
        ads.contains("const-string v0, \"0\"\n    const/4 v1, 0x0\n    nop"),
        "the block must land as the to lines, under the body's own indent:\n{ads}"
    );

    // but a piece of one line cannot become several
    let refused = dir.join("refused.apk").to_str().unwrap().to_string();
    std::fs::write(
        &bundle,
        br#"<bundle><patch name="A"><replace class="com.example.app.AdLoader" method="load"
  from="ca-app-pub-394"
  to="0&#10;nop"/></patch></bundle>"#,
    )
    .unwrap();
    let err = patch(&args(bundle.to_str().unwrap(), &apk, &refused)).unwrap_err();
    assert!(
        err.contains("a from of one line needs a to of one line"),
        "{err}"
    );
    assert!(!std::path::Path::new(&refused).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
