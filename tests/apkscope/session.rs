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

/// `search urls` defaults to app code only, ranks first-party hits first, and reports what
/// the filter hid; `--all` (Matcher without `app_only`) brings library code back. The host
/// summary groups by source. This fixture has no manifest, so its app class is `Other`
/// origin and its host order falls back to the count.
#[test]
fn search_urls_first_party_default() {
    use std::io::Write as _;
    // the library class sorts before the app class, so plain alphabetical order would put
    // it first — only the app-first ranking can produce the expected order
    let app = r#"
.class public Lcom/example/app/Main;
.super Ljava/lang/Object;

.field public static final API:Ljava/lang/String; = "https://api.example.com/v1/a"
.field public static final WEB:Ljava/lang/String; = "https://api.example.com/v1/b"

.method public constructor <init>()V
    .locals 0
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method
"#;
    let lib = r#"
.class public Landroid/ads/Loader;
.super Ljava/lang/Object;

.field public static final TRACK:Ljava/lang/String; = "https://ads.gstatic.net/x"

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
    let dir = std::env::temp_dir().join(format!("apkscope-urls-{}", std::process::id()));
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
    // the new default: app code only, and the skipped library candidate is counted
    let mut m = Matcher::new("", Scope::Urls, false, false).unwrap();
    m.app_only = true;
    let (hits, hidden) = search_stats(&st, &m, &Progress::default());
    assert_eq!(hidden, 1, "the library class is a skipped candidate");
    assert!(hits.iter().all(|h| h.0.contains("Main")), "{hits:?}");
    assert_eq!(host_count(&hits), 1);
    // --all: both classes, app code first even though the library name sorts first
    let m = Matcher::new("", Scope::Urls, false, false).unwrap();
    let (all, hidden) = search_stats(&st, &m, &Progress::default());
    assert_eq!(hidden, 0);
    assert_eq!(all.len(), 3);
    assert!(
        all[0].0.contains("Main") && all.iter().any(|h| h.0.contains("ads")),
        "app-first ranking:\n{all:?}"
    );
    assert_eq!(host_count(&all), 2);
    // the host summary: per-host counts by source, busiest first here (nothing is App)
    let sum = search_summary(&st, Scope::Urls, &all);
    assert!(sum[0].contains("hosts (2)"), "{sum:?}");
    assert!(sum[1].contains("api.example.com"), "{sum:?}");
    assert!(sum[2].contains("ads.gstatic.net"), "{sum:?}");
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

/// `search api` reconstructs the app's declared HTTP API from the Retrofit annotations:
/// verb, path and parameters are exact; the base URL resolves through `BuildConfig`; the
/// response model and the call-site arguments are inferred from the callers and labelled.
#[test]
fn search_api_reconstructs_retrofit_sheet() {
    use std::io::Write as _;
    let buildconfig = r#"
.class public Lcom/example/app/BuildConfig;
.super Ljava/lang/Object;

.field public static final API_URL:Ljava/lang/String; = "https://api.example.com/"
.field public static final API_KEY:Ljava/lang/String; = "qbznP4Xj9kLm2Rst8Vwx3Yz5Abc8Def1Ghi4"

.method public constructor <init>()V
    .locals 0
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method
"#;
    let api = r#"
.class public interface abstract Lcom/example/app/Api;
.super Ljava/lang/Object;

.method public abstract books(Ljava/lang/String;)Lretrofit2/Call;
    .annotation runtime Lretrofit2/http/GET;
        value = "v1/books"
    .end annotation
    .param p1
        .annotation runtime Lretrofit2/http/Query;
            value = "key"
        .end annotation
    .end param
.end method
"#;
    let book = r#"
.class public Lcom/example/app/Book;
.super Ljava/lang/Object;

.field public name:Ljava/lang/String;
.field public id:I

.method public constructor <init>()V
    .locals 0
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method
"#;
    let client = r#"
.class public Lcom/example/app/Client;
.super Ljava/lang/Object;

.method public static build(Lretrofit2/Retrofit$Builder;)V
    .locals 1
    sget-object v0, Lcom/example/app/BuildConfig;->API_URL:Ljava/lang/String;
    invoke-virtual {p0, v0}, Lretrofit2/Retrofit$Builder;->baseUrl(Ljava/lang/String;)Lretrofit2/Retrofit$Builder;
    return-void
.end method
"#;
    let repo = r#"
.class public Lcom/example/app/Repo;
.super Ljava/lang/Object;

.method public static load(Lcom/example/app/Api;)V
    .locals 2
    const-class v0, Lcom/example/app/Book;
    sget-object v1, Lcom/example/app/BuildConfig;->API_KEY:Ljava/lang/String;
    invoke-static {v0}, Lcom/example/app/Repo;->parse(Ljava/lang/Class;)V
    invoke-interface {p0, v1}, Lcom/example/app/Api;->books(Ljava/lang/String;)Lretrofit2/Call;
    return-void
.end method

.method private static parse(Ljava/lang/Class;)V
    .locals 0
    return-void
.end method
"#;
    let classes = [buildconfig, api, book, client, repo]
        .iter()
        .map(|s| apkscope_core::smali::text::parse(s).unwrap())
        .collect::<Vec<_>>();
    let dex = apkscope_core::smali::write::write(&classes, *b"035").unwrap();
    let dir = std::env::temp_dir().join(format!("apkscope-api-{}", std::process::id()));
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
    let m = Matcher::new("", Scope::Api, false, false).unwrap();
    let hits = search(&st, &m, &Progress::default());
    let text = || {
        hits.iter()
            .map(|h| h.2.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    // exact, from the annotations: verb + path, the parameter and its type
    assert!(
        text().contains("GET https://api.example.com/v1/books"),
        "{}",
        text()
    );
    assert!(text().contains("key (query, String)"), "{}", text());
    // the base URL, resolved through BuildConfig by its caller
    assert!(
        text().contains("base URL: https://api.example.com/"),
        "{}",
        text()
    );
    // inferred and labelled: the model and the resolved call argument
    assert!(text().contains("model inferred: Book{"), "{}", text());
    assert!(
        text().contains("name: String") && text().contains("id: int"),
        "{}",
        text()
    );
    assert!(
        text().contains(
            "curl 'https://api.example.com/v1/books?key=qbznP4Xj9kLm2Rst8Vwx3Yz5Abc8Def1Ghi4'"
        ),
        "{}",
        text()
    );
    // a query narrows the sheet
    let m = Matcher::new("nothing-matches-this", Scope::Api, false, false).unwrap();
    let hits = search(&st, &m, &Progress::default());
    assert!(
        hits.is_empty(),
        "a query that matches nothing filters the sheet: {hits:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `search secrets` / `scan` on a fixture app assembled from hand-written smali: the widened
/// compound names must catch first-party keys the plain word list missed (`HMAC_KEY`,
/// `PARTNER_CODE`, ...), stay silent on benign constants, and the entropy tier must pick up
/// a key-shaped constant under a neutral name — without repeating anything the patterns
/// already reported. This pins the false-positive rate next to the new hits, and the tier's
/// non-token shapes: URL regexes, alphabets and constants outside the 24–200 length cap
/// (the false-positive classes a large real app produces).
#[test]
fn secrets_fixture_first_party_keys() {
    use std::io::Write as _;
    let buildconfig = r#"
.class public Lcom/example/app/BuildConfig;
.super Ljava/lang/Object;

.field public static final API_KEY:Ljava/lang/String; = "qbznP4Xj9kLm2Rst8Vwx3Yz5Abc8Def1Ghi4Jkl6Mno0Pqr7Stu9Vwx2Yz4Ab6Cd8"
.field public static final HMAC_KEY:Ljava/lang/String; = "hmac-9f8e7d6c5b4a3210fedcba"
.field public static final PARTNER_CODE:Ljava/lang/String; = "partner-0042-example-777"
.field public static final APP_NAME:Ljava/lang/String; = "Demo App"
.field public static final BASE_URL:Ljava/lang/String; = "https://api.example.com"
.field public static final OBSCURE_BLOB:Ljava/lang/String; = "Xk7Pq2Rst9Vwx3Yz5Abc8Def1Ghi4Jkl6Mno0Pqr"
.field public static final LINKIFY_RE:Ljava/lang/String; = "[a-zA-Z0-9._%]+([a-zA-Z]{2,})+"
.field public static final RANDOM_CHARS:Ljava/lang/String; = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
.field public static final LONG_BLOB:Ljava/lang/String; = "Qk7Pq2Rst9Vwx3Yz5Abc8Def1Ghi4Jkl6Mno0Pqr7Stu9Vwx2Yz4Ab6Cd8Qk7Pq2Rst9Vwx3Yz5Abc8Def1Ghi4Jkl6Mno0Pqr7Stu9Vwx2Yz4Ab6Cd8Qk7Pq2Rst9Vwx3Yz5Abc8Def1Ghi4Jkl6Mno0Pqr7Stu9Vwx2Yz4Ab6Cd8Qk7Pq2Rst9Vwx3Yz5Abc8Def1Ghi4Jkl6Mno0Pqr7Stu9Vwx2Yz4Ab6Cd8"
.field public static final SHORT_BLOB:Ljava/lang/String; = "Zq7Pv2Rst9WwxK3Yz5AbcDe"

.method public constructor <init>()V
    .locals 0
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method
"#;
    // a Retrofit-style interface: @GET + @Query, the annotation the api-scope work reads
    let api = r#"
.class public interface abstract Lcom/example/app/Api;
.super Ljava/lang/Object;

.method public abstract books(Ljava/lang/String;)Lretrofit2/Call;
    .annotation runtime Lretrofit2/http/GET;
        value = "v1/books"
    .end annotation
    .param p1
        .annotation runtime Lretrofit2/http/Query;
            value = "key"
        .end annotation
    .end param
.end method
"#;
    // a caller reading the key, so the declaring-class indexing path is exercised too
    let repo = r#"
.class public Lcom/example/app/Repo;
.super Ljava/lang/Object;

.method public static fetch(Lcom/example/app/Api;Ljava/lang/String;)V
    .locals 1
    sget-object v0, Lcom/example/app/BuildConfig;->API_KEY:Ljava/lang/String;
    invoke-interface {p0, v0}, Lcom/example/app/Api;->books(Ljava/lang/String;)Lretrofit2/Call;
    return-void
.end method
"#;
    let classes = [buildconfig, api, repo]
        .iter()
        .map(|s| apkscope_core::smali::text::parse(s).unwrap())
        .collect::<Vec<_>>();
    let dex = apkscope_core::smali::write::write(&classes, *b"035").unwrap();
    // a minimal APK: one DEX, no manifest, no resources
    let dir = std::env::temp_dir().join(format!("apkscope-secrets-{}", std::process::id()));
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
    let m = Matcher::new("", Scope::Secrets, false, false).unwrap();
    let hits = search(&st, &m, &Progress::default());
    let text = || {
        hits.iter()
            .map(|h| h.2.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        text().contains("API_KEY")
            && text().contains("HMAC_KEY")
            && text().contains("PARTNER_CODE"),
        "the compound names must catch first-party keys:\n{}",
        text()
    );
    assert!(
        !text().contains("Demo App") && !text().contains("https://api.example.com"),
        "benign constants must stay silent:\n{}",
        text()
    );
    // the entropy tier: the neutral-named blob shows up; nothing the patterns already
    // reported, nothing benign
    let ent = entropy_secrets(&st);
    let etext = ent
        .iter()
        .map(|c| format!("{}#{}", c.class, c.field))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(etext.contains("OBSCURE_BLOB"), "entropy tier:\n{etext}");
    assert!(
        !etext.contains("API_KEY") && !etext.contains("HMAC_KEY"),
        "pattern hits are not repeated in the tier:\n{etext}"
    );
    assert!(
        !etext.contains("APP_NAME") && !etext.contains("BASE_URL"),
        "benign constants stay out of the tier:\n{etext}"
    );
    // the review filters: URL regexes, alphabets (long sequential runs) and constants
    // outside the 24–200 cap stay out of the tier even though their entropy is high
    assert!(
        !etext.contains("LINKIFY_RE")
            && !etext.contains("RANDOM_CHARS")
            && !etext.contains("LONG_BLOB")
            && !etext.contains("SHORT_BLOB"),
        "regex, alphabet and off-cap constants stay out of the tier:\n{etext}"
    );
    // scan reports the same keys in its secrets section
    let scan = crate::re::scan(&st);
    assert!(scan.contains("HMAC_KEY"), "scan secrets section:\n{scan}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `search config` reports what the app ships as changeable, in labelled tiers: the
/// BuildConfig string defaults exactly (and only `*.BuildConfig` classes — other string
/// constants are not config), the remote-config defaults XML entries, and reads of
/// Firebase Remote Config / SharedPreferences grouped by key with their fallbacks
/// (inferred).
#[test]
fn search_config_reports_changeable_surface() {
    use std::io::Write as _;
    // the defaults-XML parser, on a Firebase-style defaults map
    let xml = "<?xml version=\"1.0\"?><defaultsMap>\
               <entry><key>tier</key><value>stable</value></entry>\
               <entry><key>hidden</key><value>false</value></entry></defaultsMap>";
    assert_eq!(
        xml_entries(xml),
        vec![
            ("tier".to_string(), "stable".to_string()),
            ("hidden".to_string(), "false".to_string())
        ]
    );

    let buildconfig = r#"
.class public Lcom/example/app/BuildConfig;
.super Ljava/lang/Object;

.field public static final TIER:Ljava/lang/String; = "stable"

.method public constructor <init>()V
    .locals 0
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method
"#;
    let app = r#"
.class public Lcom/example/app/Prefs;
.super Ljava/lang/Object;

.field public static final NOT_DEFAULT:Ljava/lang/String; = "not a config default"

.method public static load(Landroid/content/SharedPreferences;)Ljava/lang/String;
    .locals 2
    const-string v0, "tier"
    const-string v1, "beta"
    invoke-interface {p0, v0, v1}, Landroid/content/SharedPreferences;->getString(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;
    move-result-object v0
    return-object v0
.end method
"#;
    // a second reader of the same key: the grouped tier collapses both into one row
    let other = r#"
.class public Lcom/example/app/Other;
.super Ljava/lang/Object;

.method public static peek(Landroid/content/SharedPreferences;)I
    .locals 1
    const-string v0, "tier"
    const-string v1, "beta"
    invoke-interface {p0, v0, v1}, Landroid/content/SharedPreferences;->getString(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;
    move-result-object v0
    const-string v0, "hidden"
    invoke-interface {p0, v0}, Landroid/content/SharedPreferences;->getInt(Ljava/lang/String;)I
    move-result v0
    return v0
.end method
"#;
    let classes = [buildconfig, app, other]
        .iter()
        .map(|s| apkscope_core::smali::text::parse(s).unwrap())
        .collect::<Vec<_>>();
    let dex = apkscope_core::smali::write::write(&classes, *b"035").unwrap();
    let dir = std::env::temp_dir().join(format!("apkscope-config-{}", std::process::id()));
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
    let m = Matcher::new("", Scope::Config, false, false).unwrap();
    let hits = search(&st, &m, &Progress::default());
    let text = || {
        hits.iter()
            .map(|h| h.2.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    // exact: the shipped default, from the DEX tables — BuildConfig only
    assert!(
        text().contains("default: com.example.app.BuildConfig#TIER = \"stable\""),
        "{}",
        text()
    );
    // other static string constants are not config: the noise tier is gone
    assert!(
        !text().contains("NOT_DEFAULT") && !text().contains("not a config default"),
        "only *.BuildConfig classes list defaults:\n{}",
        text()
    );
    // inferred and grouped by key: two readers of `tier`, one row, first class + count
    let tier_rows = text()
        .lines()
        .filter(|l| l.starts_with("read: tier"))
        .count();
    assert_eq!(tier_rows, 1, "grouped by key:\n{}", text());
    assert!(
        text().contains(
            "read: tier    fallback: \"beta\"    (inferred)    in com.example.app.Other +1 more"
        ),
        "{}",
        text()
    );
    // a read with no literal fallback says so instead of pretending
    assert!(
        text().contains(
            "read: hidden    (inferred, no literal fallback)    in com.example.app.Other"
        ),
        "{}",
        text()
    );
    // a query narrows the sheet
    let m = Matcher::new("tier", Scope::Config, false, false).unwrap();
    let hits = search(&st, &m, &Progress::default());
    assert!(
        hits.iter().all(|h| h.2.to_lowercase().contains("tier")),
        "{hits:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
