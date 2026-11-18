use super::*;

#[test]
fn flutter_routes() {
    for ok in [
        "/GreenEnergy/Pay",
        "/contactauth/verify-otp",
        "/contact/conversation/active/get",
        "/update/v2",
    ] {
        assert!(is_route(ok), "should accept {ok}");
    }
    for bad in [
        "/svgs/logo.svg",
        "/ca/ca.crt",              // assets / files (dot)
        "/api.shamcash.sy/v4/api", // host fragment (dot)
        "/MM/yyyy",
        "/dd/aaaa",                         // date formats (repeated chars)
        "/H/pF",                            // too-short segments
        "/EutsZ9kdvN/iZ8N4Zze77ayD16CEpux", // base64 blob (digits in segment)
        "/Adobe-Identity-UCS/ProcSet",      // PDF dict (3-uppercase run)
        "no-leading-slash",
        "/single", // not 2+ segments
    ] {
        assert!(!is_route(bad), "should reject {bad}");
    }
}

#[test]
fn case_insensitive_contains() {
    for (h, n) in [
        ("openConnection", "connect"),
        ("OPEN", "open"),
        ("abc", "x"),
        ("Ã‰tÃ©", "Ã©tÃ©"),
        ("abc", "Ã©"),
        ("a", ""),
    ] {
        assert_eq!(contains_ci(h, n), h.to_lowercase().contains(n), "{h} {n}");
    }
}

#[test]
fn manifest_permissions() {
    let xml = "<manifest>\n    <uses-permission\n        android:name=\"android.permission.INTERNET\" />\n    <uses-permission-sdk-23\n        android:name=\"android.permission.CAMERA\" />\n    <application\n        android:name=\"App\" />\n</manifest>";
    assert_eq!(
        permissions(xml),
        ["android.permission.INTERNET", "android.permission.CAMERA"]
    );
}

#[test]
fn mutf8_nul_and_supplementary() {
    assert_eq!(dex::mutf8(b"plain"), "plain");
    assert_eq!(dex::mutf8(&[b'a', 0xc0, 0x80, b'b']), "a\0b");
    // U+1F600 as two 3-byte surrogates
    assert_eq!(
        dex::mutf8(&[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]),
        "\u{1F600}"
    );
}

#[test]
fn descriptors() {
    assert_eq!(descriptor("com.a.B"), "Lcom/a/B;");
    assert_eq!(descriptor("Lcom/a/B;"), "Lcom/a/B;");
    assert_eq!(descriptor("com/a/B$C"), "Lcom/a/B$C;");
}

#[test]
fn cuts_one_method() {
    let java = "class A {\n    @Override\n    public void run(int x) {\n        go(x);\n    }\n\n    void other() {\n        run(1);\n    }\n}\n";
    let m = method_of(java, "run").unwrap();
    assert!(m.contains("@Override") && m.contains("go(x);") && !m.contains("other"));
    assert!(method_of(java, "missing").is_none());
}
