use super::*;

#[test]
fn kinds_and_reports() {
    let missing = std::env::temp_dir().join(format!("apkscope-missing-{}.apk", std::process::id()));
    let e = crate::apk::open(missing.to_str().unwrap()).err().unwrap();
    assert!(matches!(&e, Error::Io { path, .. } if *path == missing));
    assert!(e.report().starts_with(&format!("{}: ", missing.display())));

    let not_zip = std::env::temp_dir().join(format!("apkscope-not-zip-{}.apk", std::process::id()));
    std::fs::write(&not_zip, b"junk").unwrap();
    let e = crate::apk::open(not_zip.to_str().unwrap()).err().unwrap();
    let _ = std::fs::remove_file(&not_zip);
    assert!(matches!(&e, Error::In { source, .. } if matches!(**source, Error::Zip(_))));
    assert!(e.report().starts_with(&format!("{}: ", not_zip.display())));

    let mut src = crate::smali::Source::default();
    let bogus = ".class LA;\n.method static f()V\n    bogus v0\n.end method\n";
    let e = crate::smali::text::parse(&mut src, bogus).unwrap_err();
    assert!(matches!(e, Error::Smali { line: 3, .. }), "{e}");
    let text: String = e.into();
    assert!(text.starts_with("line 3: "), "{text}");

    assert!(matches!(
        crate::dex::Dex::parse(b"nope"),
        Err(Error::Dex(_))
    ));
    let wide = ".class LA;\n.super Ljava/lang/Object;\n.method static f()V\n    .locals 1\n    const/4 v20, 0x1\n    return-void\n.end method\n";
    let class = crate::smali::text::parse(&mut src, wide).unwrap();
    assert!(matches!(
        crate::smali::write::write(&src, &[class], *b"035"),
        Err(Error::Assemble(_))
    ));
}
