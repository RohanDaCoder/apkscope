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

/// A minimal binary manifest, so the diff's permissions section has something to read.
fn manifest_bytes() -> Vec<u8> {
    let names = apkscope_core::res::Names::new(&apkscope_core::table::Table {
        strings: apkscope_core::table::StringPool {
            utf8: true,
            sorted: false,
            strings: vec![],
            styles: vec![],
        },
        packages: vec![],
    });
    apkscope_core::axml::encode(
        r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.example.app" />"#,
        None,
        &names,
    )
    .unwrap()
}

/// The diff's constants section is the exact rotated-key answer, and the strings pairing
/// names likely rotations deterministically, with its rule printed next to the pairs.
#[test]
fn diff_reports_rotated_constants_and_near_pairs() {
    use std::io::Write as _;
    // bounded edit distance: near within the limit, far beyond it
    assert_eq!(bounded_edit_distance("qbznP4Xj9kLm", "qbznP4Xj9kLn", 3), 1);
    assert!(bounded_edit_distance("qbznP4Xj9kLm", "totally-different", 3) > 3);
    // the pairing: a rotated value pairs with its successor, unrelated strings do not
    let old_val = "qbznP4Xj9kLm2Rst8Vwx3Yz5Abc8Def1Ghi4";
    let new_val = "qbznP4Xj9kLm2Rst8Vwx3Yz5Abc8Def1Ghi5";
    let removed = [
        old_val.to_string(),
        "https://old.example.com/api/v1".to_string(),
    ];
    let added = [new_val.to_string(), "unrelated-new-string".to_string()];
    let pairs = near_pairs(
        &removed.iter().collect::<Vec<_>>(),
        &added.iter().collect::<Vec<_>>(),
    );
    assert_eq!(pairs.len(), 1, "{pairs:?}");
    assert!(
        pairs[0].contains(old_val) && pairs[0].contains(new_val),
        "{pairs:?}"
    );

    // two builds of a fixture app, the second with the rotated constant
    let mk = |key: &str| -> String {
        let src = format!(
            ".class public Lcom/example/app/BuildConfig;\n.super Ljava/lang/Object;\n\n\
             .field public static final API_KEY:Ljava/lang/String; = \"{key}\"\n\n\
             .method public constructor <init>()V\n    .locals 0\n    \
             invoke-direct {{p0}}, Ljava/lang/Object;-><init>()V\n    return-void\n.end method\n"
        );
        let dex = apkscope_core::smali::assemble(&[&src], *b"035").unwrap();
        let dir = std::env::temp_dir().join(format!(
            "apkscope-diff-{}-{}",
            std::process::id(),
            &key[key.len() - 4..]
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let apk = dir.join("fixture.apk");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&apk).unwrap());
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            z.start_file("classes.dex", stored).unwrap();
            z.write_all(&dex).unwrap();
            z.start_file("AndroidManifest.xml", stored).unwrap();
            z.write_all(&manifest_bytes()).unwrap();
            z.finish().unwrap();
        }
        apk.display().to_string()
    };
    let (old, new) = (mk(old_val), mk(new_val));
    let out = diff(&old, &new, true, false).unwrap();
    let text = out.join("\n");
    // exact: the constant's old and new values, keyed Class#FIELD
    assert!(text.contains("## constants (changed values): 1"), "{text}");
    assert!(
        text.contains(&format!(
            "~ com.example.app.BuildConfig#API_KEY: {old_val} -> {new_val}"
        )),
        "{text}"
    );
    // the pairing names the likely rotation and its rule
    assert!(text.contains("! likely rotated/renamed"), "{text}");
    assert!(
        text.contains(&format!("! {old_val} -> {new_val} (edit distance 1)")),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(std::path::Path::new(&old).parent().unwrap());
    let _ = std::fs::remove_dir_all(std::path::Path::new(&new).parent().unwrap());
}

/// A Flutter XAPK keeps `libapp.so` / `libflutter.so` in `config.<abi>.apk` splits, not in
/// `base.apk`, so flattening a bundle to its base split (what `search::prepare` does) hides
/// them: `flutter` used to report "no" on a valid Flutter bundle. Build a bundle the way real
/// bundle tools write them — splits stored in place, one per ABI — and check that `so_entries`,
/// `read_entry` and `flutter` all reach the config splits while non-`lib` entries still come
/// from `base.apk` first; a deflated bundle (the buffered path) answers the same.
#[test]
fn flutter_sees_libs_in_xapk_splits() {
    use std::io::Write as _;
    let (libapp, libflutter) = (b"libapp-bytes", b"libflutter-bytes");
    let stored = zip::CompressionMethod::Stored;
    let deflated = zip::CompressionMethod::Deflated;
    // inner APK: (name, bytes, method)
    let apk = |entries: &[(&str, &[u8], zip::CompressionMethod)]| -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut out);
        for (n, b, m) in entries {
            let opts = zip::write::SimpleFileOptions::default().compression_method(*m);
            w.start_file(*n, opts).unwrap();
            w.write_all(b).unwrap();
        }
        w.finish().unwrap();
        out.into_inner()
    };
    // bundle of inner APKs, each stored (real bundles) or deflated (the buffered path)
    let bundle = |splits: &[(&str, Vec<u8>)], stored: bool| -> String {
        let path = std::env::temp_dir().join(format!(
            "apkscope_xapk_splits_{}_{}.xapk",
            if stored { "stored" } else { "deflated" },
            std::process::id()
        ));
        let mut w = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default().compression_method(if stored {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        });
        for (n, b) in splits {
            w.start_file(*n, opts).unwrap();
            w.write_all(b).unwrap();
        }
        w.finish().unwrap();
        path.display().to_string()
    };

    // the realistic case: code and manifest in base, ABI libs in config splits, all stored
    let xapk = bundle(
        &[
            (
                "base.apk",
                apk(&[
                    ("classes.dex", b"dex-bytes", stored),
                    ("AndroidManifest.xml", b"manifest-of-base", stored),
                ]),
            ),
            (
                "config.arm64_v8a.apk",
                apk(&[
                    ("lib/arm64-v8a/libapp.so", libapp, deflated),
                    ("lib/arm64-v8a/libflutter.so", libflutter, stored),
                ]),
            ),
            (
                "config.armeabi_v7a.apk",
                apk(&[("lib/armeabi-v7a/libapp.so", b"32-bit-libapp", stored)]),
            ),
        ],
        true,
    );
    // so_entries lists the libs of every config split, not just base.apk
    let libs = so_entries(&xapk).unwrap();
    let names: Vec<&str> = libs.iter().map(|(n, _)| n.as_str()).collect();
    for want in [
        "lib/arm64-v8a/libapp.so",
        "lib/arm64-v8a/libflutter.so",
        "lib/armeabi-v7a/libapp.so",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    // read_entry reads entries out of the splits: a deflated lib, and the second config split
    assert_eq!(
        read_entry(&xapk, "lib/arm64-v8a/libapp.so").unwrap(),
        Some(libapp.to_vec())
    );
    assert_eq!(
        read_entry(&xapk, "lib/armeabi-v7a/libapp.so").unwrap(),
        Some(b"32-bit-libapp".to_vec())
    );
    // non-lib entries still come from base.apk first
    assert_eq!(
        read_entry(&xapk, "AndroidManifest.xml").unwrap(),
        Some(b"manifest-of-base".to_vec())
    );
    // flutter reports yes (decided from the lib list before libapp.so is parsed)
    let out = flutter(&xapk, None, false, false).unwrap();
    assert_eq!(out[0], "Flutter app: yes", "got {out:?}");
    let _ = std::fs::remove_file(&xapk);

    // a stale copy of the same lib in base.apk loses to the config split, which is searched
    // first for lib/ — and a deflated bundle (the buffered path) gives the same answer
    let xapk = bundle(
        &[
            (
                "base.apk",
                apk(&[
                    ("classes.dex", b"dex-bytes", stored),
                    ("lib/arm64-v8a/libapp.so", b"stale-base-libapp", stored),
                ]),
            ),
            (
                "config.arm64_v8a.apk",
                apk(&[("lib/arm64-v8a/libapp.so", libapp, stored)]),
            ),
        ],
        false,
    );
    assert_eq!(
        read_entry(&xapk, "lib/arm64-v8a/libapp.so").unwrap(),
        Some(libapp.to_vec()),
        "the config split should win over base.apk"
    );
    let _ = std::fs::remove_file(&xapk);
}
