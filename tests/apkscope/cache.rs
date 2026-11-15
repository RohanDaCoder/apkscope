use super::*;

#[test]
fn round_trip_and_incomplete_cache_ignored() {
    let dir = std::env::temp_dir().join(format!("apkscope-cache-test-{}", std::process::id()));
    let w = Writer::create(&dir, 3).unwrap();
    w.put(2, "class C {}");
    w.put(0, "class A { int x; }");
    // no finish yet: not readable
    assert!(Reader::open(&dir, 3).is_none());
    let r = w.finish().unwrap();
    let mut f = r.file().unwrap();
    assert_eq!(r.get(&mut f, 0).as_deref(), Some("class A { int x; }"));
    assert_eq!(r.get(&mut f, 1), None);
    assert_eq!(r.get(&mut f, 2).as_deref(), Some("class C {}"));
    let _ = fs::remove_dir_all(dir);
}
