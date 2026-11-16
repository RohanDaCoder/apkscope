use super::*;

#[test]
fn detects_obfuscated_names() {
    for n in ["a", "b0", "zza", "zzbx", "if", "A", "1x"] {
        assert!(obfuscated(n), "{n}");
    }
    for n in ["id", "x", "onCreate", "LoginActivity", "<init>", "abc"] {
        assert!(!obfuscated(n), "{n}");
    }
    assert_eq!(
        package("a/b0/okhttp3/ui/h"),
        "pkg_a/pkg_b0/okhttp3/ui/pkg_h"
    );
    assert_eq!(cap("a0"), "A0");
    assert_ne!(ident("a"), ident("A"));
}
