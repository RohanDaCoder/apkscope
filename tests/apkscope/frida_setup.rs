use super::*;

#[test]
fn names_the_frida_server_file_for_a_device() {
    assert_eq!(
        server_file("17.2.15", "arm64-v8a"),
        Some((
            "frida-server-17.2.15-android-arm64.xz".to_string(),
            "https://github.com/frida/frida/releases/download/17.2.15/frida-server-17.2.15-android-arm64.xz".to_string()
        ))
    );
    assert_eq!(frida_arch("armeabi-v7a"), Some("arm"));
    assert_eq!(frida_arch("x86_64"), Some("x86_64"));
    assert_eq!(frida_arch("x86"), Some("x86"));
    assert_eq!(server_file("17.2.15", "mips"), None);
}

#[test]
fn http_preset_is_a_raw_script_that_reports_http_events() {
    let js = crate::live::preset("http").unwrap();
    assert!(js.starts_with("Java.perform"));
    assert!(js.contains("__ev(\"http\""));
    // OkHttp 3 and 4 names, and the platform HttpURLConnection
    assert!(js.contains("okhttp3.RealCall"));
    assert!(js.contains("okhttp3.internal.connection.RealCall"));
    assert!(js.contains("HttpURLConnectionImpl"));
    assert!(crate::live::preset("nope").is_none());
    // standalone: the helpers come first
    assert!(crate::live::standalone(js).contains("function __ev"));
}
