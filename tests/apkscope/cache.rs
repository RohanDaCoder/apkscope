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

#[test]
fn reads_the_version_a_cache_folder_was_made_by() {
    assert_eq!(version_of("Telegram-4321-65a1b2-d-0.12.0"), Some("0.12.0"));
    assert_eq!(version_of("Telegram-4321-65a1b2-n-0.11.0-u1a2b3c"), Some("0.11.0"));
    // a file name with -d- in it: the last marker is the real one
    assert_eq!(version_of("my-d-app-10-20-d-0.12.0"), Some("0.12.0"));
    assert_eq!(version_of("inputs"), None);
}
