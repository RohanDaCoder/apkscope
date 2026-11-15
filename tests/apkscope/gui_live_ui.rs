use super::*;

#[test]
fn stream_depth_and_tiles() {
    let mut ui = LiveUi::default();
    let ev: Vec<Value> = [
        r#"{"k":"call","m":"a.B.f","v":[],"tid":1,"seq":0}"#,
        r#"{"k":"call","m":"a.B.g","v":["1"],"tid":1,"seq":1}"#,
        r#"{"k":"ret","m":"a.B.g","v":"2","tid":1,"seq":2}"#,
        r#"{"k":"throw","m":"a.B.f","v":"E","tid":1,"seq":3}"#,
    ]
    .iter()
    .map(|l| serde_json::from_str(l).unwrap())
    .collect();
    ui.ingest(ev, Instant::now());
    assert_eq!(
        ui.rows.iter().map(|r| r.0).collect::<Vec<_>>(),
        [0, 1, 1, 0]
    );
    assert_eq!(ui.seen, 4);
    assert!(ui.tiles["a.B.f"].threw && ui.tiles["a.B.g"].calls == 1);
    assert!(matches!(open_method("a.b.C$D.run"), Some(Act::OpenClass(c, _)) if c == "a.b.C"));
}
