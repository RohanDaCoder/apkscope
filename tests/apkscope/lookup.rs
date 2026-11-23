use super::*;

/// Classes assembled from smali: constants (one set by <clinit>, so not a stored constant),
/// methods that return a constant, and an interface with direct and nested implementors.
fn fixture() -> Vec<u8> {
    let codes = r#"
.class public Lcom/example/net/Codes;
.super Ljava/lang/Object;

.field public static final ERROR:I = 0x195
.field public static final TL_ID:I = -0x472f5021
.field public static final NAME:Ljava/lang/String; = "room"
.field public static final COMPUTED:I = 0x0

.method static constructor <clinit>()V
    .locals 1
    const/16 v0, 0x40
    sput v0, Lcom/example/net/Codes;->COMPUTED:I
    return-void
.end method
"#;
    let proto = r#"
.class public interface abstract Lcom/example/net/IProtocol;
.super Ljava/lang/Object;

.method public abstract uri()I
.end method
"#;
    let join = r#"
.class public Lcom/example/net/JoinReq;
.super Ljava/lang/Object;
.implements Lcom/example/net/IProtocol;

.method public uri()I
    .locals 1
    const v0, 0xb3b83
    return v0
.end method

.method public big()Z
    .locals 1
    const/4 v0, 0x1
    return v0
.end method
"#;
    // implements through a nested subclass of an implementor
    let leave = r#"
.class public Lcom/example/net/Room$Leave;
.super Lcom/example/net/JoinReq;

.method public uri()I
    .locals 1
    const/16 v0, 0x2a
    return v0
.end method
"#;
    let other = r#"
.class public Lcom/other/Util;
.super Ljava/lang/Object;

.method public uri()I
    .locals 1
    const/4 v0, 0x7
    return v0
.end method
"#;
    apkscope_core::smali::assemble(&[codes, proto, join, leave, other], *b"035").unwrap()
}

#[test]
fn consts_values_and_filters() {
    let bytes = fixture();
    let dexes = [Dex::parse(&bytes).unwrap()];
    let all = NameFilter::new(&[]).unwrap();
    let rows = consts(&dexes, &all, &ValueFilter::new(None), None);
    let lines: Vec<String> = rows.iter().map(Row::line).collect();
    assert!(
        lines.contains(&"com.example.net.Codes#ERROR = 405    (int)".to_string()),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("NAME = \"room\"")),
        "{lines:?}"
    );
    // set by <clinit>: the stored 0 is only the default, not the value the app runs with
    assert!(!lines.iter().any(|l| l.contains("COMPUTED")), "{lines:?}");
    // a number, and the hex of a negative int (0xB8D0AFDF = -1194283041)
    let by = |v: &str| consts(&dexes, &all, &ValueFilter::new(Some(v)), None).len();
    assert_eq!(by("405"), 1);
    assert_eq!(by("0x195"), 1);
    assert_eq!(by("0xB8D0AFDF"), 1);
    assert_eq!(by("room"), 1);
    assert_eq!(by("406"), 0);
    // packages and nested classes
    let pkg = NameFilter::new(&["com.example.net.*".into()]).unwrap();
    assert!(pkg.matches("com.example.net.Room$Leave"));
    assert!(!pkg.matches("com.other.Util"));
    let exact = NameFilter::new(&["com.example.net.Room".into()]).unwrap();
    assert!(exact.matches("com.example.net.Room$Leave") && !exact.matches("com.example.net.RoomX"));
}

#[test]
fn returns_with_impl() {
    let bytes = fixture();
    let dexes = [Dex::parse(&bytes).unwrap()];
    let impls = implementors(&dexes, "com.example.net.IProtocol");
    assert!(impls.contains("Lcom/example/net/JoinReq;"));
    assert!(
        impls.contains("Lcom/example/net/Room$Leave;"),
        "through a subclass"
    );
    assert!(!impls.contains("Lcom/other/Util;"));
    let rows = returns(
        &dexes,
        Some("uri"),
        |d, _| impls.contains(d),
        &ValueFilter::new(None),
    );
    let lines: Vec<String> = rows.iter().map(Row::line).collect();
    assert_eq!(
        lines,
        [
            "com.example.net.JoinReq#uri() = 736131    (int)",
            "com.example.net.Room$Leave#uri() = 42    (int)"
        ]
    );
    // by value, and a boolean return
    let by_value = returns(
        &dexes,
        Some("uri"),
        |_, _| true,
        &ValueFilter::new(Some("0xB3B83")),
    );
    assert_eq!(by_value.len(), 1);
    let any = returns(&dexes, None, |_, _| true, &ValueFilter::new(None));
    assert!(
        any.iter()
            .any(|r| r.member == "big()" && r.value == "true" && r.kind == "boolean"),
        "{any:?}"
    );
}
