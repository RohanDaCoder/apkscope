use super::*;
use std::io::Write;

/// A bundle holding `base.apk` (whose bytes are `inner`), stored or deflated.
fn bundle(dir: &Path, name: &str, inner: &[u8], stored: bool) -> String {
    let p = dir.join(name);
    let mut w = zip::ZipWriter::new(File::create(&p).unwrap());
    let m = if stored {
        zip::CompressionMethod::Stored
    } else {
        zip::CompressionMethod::Deflated
    };
    let o = zip::write::SimpleFileOptions::default().compression_method(m);
    w.start_file("config.en.apk", o).unwrap();
    w.write_all(b"not the base").unwrap();
    w.start_file("base.apk", o).unwrap();
    w.write_all(inner).unwrap();
    w.finish().unwrap();
    p.display().to_string()
}

#[test]
fn reads_a_stored_base_split_in_place() {
    let dir = std::env::temp_dir().join(format!("apkscope-input-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let inner: Vec<u8> = (0..5000u32).flat_map(|i| i.to_le_bytes()).collect();
    let p = bundle(&dir, "app.xapk", &inner, true);
    let mut i = Input::open(&p).unwrap();
    assert_eq!(i.len(), inner.len() as u64);
    let mut all = Vec::new();
    i.read_to_end(&mut all).unwrap();
    assert_eq!(all, inner);
    // seeks are relative to the split and clamped to it
    assert_eq!(i.seek(SeekFrom::End(-4)).unwrap(), inner.len() as u64 - 4);
    let mut tail = [0u8; 8];
    assert_eq!(i.read(&mut tail).unwrap(), 4);
    assert_eq!(&tail[..4], &inner[inner.len() - 4..]);
    // another handle reads the same bytes
    let mut j = i.reopen().unwrap();
    let mut again = Vec::new();
    j.read_to_end(&mut again).unwrap();
    assert_eq!(again, inner);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_plain_file_is_read_whole() {
    let dir = std::env::temp_dir().join(format!("apkscope-input-plain-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("app.apk");
    std::fs::write(&p, b"PK whole file").unwrap();
    let mut all = Vec::new();
    Input::open(&p.display().to_string())
        .unwrap()
        .read_to_end(&mut all)
        .unwrap();
    assert_eq!(all, b"PK whole file");
    std::fs::remove_dir_all(&dir).unwrap();
}
