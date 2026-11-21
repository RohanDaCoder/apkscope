use super::*;

#[test]
fn api_scopes_match() {
    let urls = Matcher::new("", Scope::Urls, false, false).unwrap();
    assert!(urls.line(r#"String u = "https://api.example.com/v1/login";"#));
    assert!(urls.line(r#"@GET("/api/users/{id}")"#));
    assert!(!urls.line(r#"String s = "hello world";"#));
    assert!(!urls.line(r#"xmlns:android="http://schemas.android.com/apk/res/android""#));
    let http = Matcher::new("", Scope::Http, false, false).unwrap();
    assert!(
        http.line(r#"@POST("v2/token")"#)
            && http.line("Request request = new Request.Builder().url(str).build();")
    );
    assert!(!http.line("int getHeight() {"));
    assert!(
        !http.line("IntentSenderRequest.Builder builder = new IntentSenderRequest.Builder(x);")
    );
    assert!(!http.line("import okhttp3.Request.Builder;"));
    let sec = Matcher::new("", Scope::Secrets, false, false).unwrap();
    assert!(
        sec.line(
            r#"<string name="google_api_key">AIzaSyA1234567890abcdefghijklmnopqrstuv</string>"#
        )
    );
    assert!(sec.line(r#"builder.addHeader("Authorization", "Bearer " + token);"#));
    assert!(!sec.line("return x + 1;"));
    assert!(
        !sec.line(r#"<string name="password_toggle_content_description">Show password</string>"#)
    );
    assert!(sec.line(r#"private static final String API_KEY = "k3y-1234567890";"#));
    assert!(sec.line(r#"<string name="facebook_client_token">abcdef123456</string>"#));
    // query narrows a scope; String scope looks only inside literals
    let narrowed = Matcher::new("example", Scope::Urls, false, false).unwrap();
    assert!(narrowed.line(r#""https://example.com""#) && !narrowed.line(r#""https://other.com""#));
    let lit = Matcher::new("abc", Scope::String, false, false).unwrap();
    assert!(lit.line(r#"f("xabcx");"#) && !lit.line("abc();"));
    assert!(Matcher::new("(", Scope::Code, true, false).is_err());
}

#[test]
fn excerpts_and_formats() {
    let src = "class A {\n    void f(int x) {\n        if (x > 0) {\n            call(\"/api/auth\");\n        }\n    }\n\n    void g() {\n    }\n}";
    let lines: Vec<&str> = src.lines().collect();
    // whole method: from the header to its closing brace, skipping the `if`
    assert_eq!(excerpt(&lines, 3, 0, true), (1, 6));
    assert_eq!(excerpt(&lines, 3, 1, false), (2, 5));
    assert_eq!(excerpt(&lines, 0, 5, false), (0, 6));
    let hits = vec![("A".to_string(), 4, "call(\"/api/auth\");".to_string())];
    let json = export(&hits, 0, true, Format::Json, |_| Some(src.to_string()));
    assert!(
        json.contains("\"excerpt\":\"    void f(int x) {\\n") && json.trim_end().ends_with(']'),
        "{json}"
    );
    let csv = export(&hits, 0, false, Format::Csv, |_| None);
    assert_eq!(
        csv.lines().nth(1),
        Some("\"A\",4,\"call(\"\"/api/auth\"\");\",\"call(\"\"/api/auth\"\");\"")
    );
}

#[test]
fn origins() {
    assert_eq!(
        origin("oncode.apps.zestle.MainActivity", "oncode.apps.zestle"),
        Origin::App
    );
    // a sibling package with the same prefix is not the app
    // code beside the launcher package is the app's too, a different organization is not
    assert_eq!(
        origin("com.unciv.logic.GameInfo", "com.unciv.app"),
        Origin::App
    );
    assert_eq!(origin("com.uncivx.A", "com.unciv.app"), Origin::Other);
    assert_eq!(
        origin("androidx.core.view.ViewCompat", "oncode.apps.zestle"),
        Origin::Library
    );
    assert_eq!(origin("pkg_a.ClassB", "oncode.apps.zestle"), Origin::Other);
    assert_eq!(origin("com.google.gson.Gson", ""), Origin::Library);
}

#[test]
fn manifest_attrs() {
    assert_eq!(
        attr("<x android:name=\"a.B\" package=\"p.q\">", "package").as_deref(),
        Some("p.q")
    );
    assert_eq!(attr("<x>", "package"), None);
}

/// `.apkm` is a plain ZIP of split APKs, like `.apks`/`.xapk`; `prepare` must pick
/// `base.apk` even when a resource split is larger, and ignore non-APK entries
/// like `info.json`.
#[test]
fn apkm_extract_picks_base_apk() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("apkscope-apkm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let apkm = dir.join("sample.apkm");
    {
        let mut z = zip::ZipWriter::new(std::fs::File::create(&apkm).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        // base.apk: a minimal APK-ish blob; smaller than the xxhdpi split on purpose
        z.start_file("base.apk", opts).unwrap();
        z.write_all(b"PK\x03\x04base code").unwrap();
        z.start_file("split_config.xxhdpi.apk", opts).unwrap();
        z.write_all(b"PK\x03\x04resourcesresourcesresources")
            .unwrap();
        z.start_file("split_config.arm64_v8a.apk", opts).unwrap();
        z.write_all(b"PK\x03\x04native").unwrap();
        z.start_file("info.json", opts).unwrap();
        z.write_all(b"{}").unwrap();
        z.finish().unwrap();
    }
    let out = prepare(apkm.to_str().unwrap()).unwrap();
    let picked = std::path::Path::new(&out)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        picked.ends_with("base.apk"),
        "expected base.apk, got {picked}"
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"PK\x03\x04base code",
        "extracted the base split, not a larger resource split"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `info` shows the native-library meanings and the dex-measured code share, and `scan`
/// marks URL hits that carry a key-like parameter — the secrets/URL cross-reference.
#[test]
fn info_native_libs_sdk_share_and_scan_crossref() {
    use std::io::Write as _;
    // the dictionary: one line per known library, nothing for unknowns
    assert_eq!(
        native_lib_meaning("lib/arm64-v8a/libpdfium.so"),
        Some("in-app PDF rendering")
    );
    assert_eq!(native_lib_meaning("libflutter.so"), Some("Flutter engine"));
    assert_eq!(
        native_lib_meaning("lib/x86_64/libjiagu.so"),
        Some("packer: 360 Jiagu")
    );
    assert_eq!(native_lib_meaning("lib/arm64-v8a/libfoo.so"), None);

    let app = r#"
.class public Lcom/example/app/Net;
.super Ljava/lang/Object;

.field public static final CALL:Ljava/lang/String; = "https://api.example.com/v1/x?key=abc1234567890"

.method public constructor <init>()V
    .locals 0
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method

.method public static call()V
    .locals 0
    return-void
.end method
"#;
    let lib = r#"
.class public Landroidx/core/Widget;
.super Ljava/lang/Object;

.method public constructor <init>()V
    .locals 0
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method
"#;
    let classes = [app, lib]
        .iter()
        .map(|s| apkscope_core::smali::text::parse(s).unwrap())
        .collect::<Vec<_>>();
    let dex = apkscope_core::smali::write::write(&classes, *b"035").unwrap();
    let dir = std::env::temp_dir().join(format!("apkscope-info-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let apk = dir.join("fixture.apk");
    {
        let mut z = zip::ZipWriter::new(std::fs::File::create(&apk).unwrap());
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        z.start_file("classes.dex", stored).unwrap();
        z.write_all(&dex).unwrap();
        z.finish().unwrap();
    }
    let st = load(apk.to_str().unwrap(), false).unwrap();
    st.build_index();
    // code share, counted from the DEX: the app class declares 2 methods, the library 1
    let share = sdk_share(&st);
    assert!(share.contains(&("androidx".to_string(), 1, 1)), "{share:?}");
    assert!(
        share
            .iter()
            .any(|(g, c, m)| g == "other code" && *c == 1 && *m == 2),
        "{share:?}"
    );
    // the overview carries both new sections
    let text = overview(&st);
    assert!(text.contains("code by origin"), "{text}");
    assert!(text.contains("androidx"), "{text}");
    // scan marks the URL that carries a key-like parameter
    let scan = crate::re::scan(&st);
    assert!(
        scan.contains("1 carrying a key-like parameter") && scan.contains("<= key-like"),
        "{scan}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
