use super::*;

#[test]
fn write_whole_leaves_no_partial_copy() {
    let dir = std::env::temp_dir().join(format!("apkscope-write-whole-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("copy.apk");
    let part = dir.join("copy.apk.part");

    // a fill that fails halfway leaves neither the copy nor the part behind
    let err = write_whole(&out, |f| {
        use std::io::Write;
        f.write_all(b"half")?;
        Err(std::io::Error::other("killed"))
    });
    assert!(err.is_err());
    assert!(!out.exists() && !part.exists());

    // a whole one is moved into place
    write_whole(&out, |f| {
        use std::io::Write;
        f.write_all(b"whole")
    })
    .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"whole");
    assert!(!part.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
