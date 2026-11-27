use super::*;

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
