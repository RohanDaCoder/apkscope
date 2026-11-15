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
