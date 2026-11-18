use super::*;

fn sample() -> std::path::PathBuf {
    use std::io::Write;
    let p = std::env::temp_dir().join(format!("apkscope-rebuild-{}.apk", std::process::id()));
    let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    w.start_file("AndroidManifest.xml", deflated).unwrap();
    w.write_all(&[1u8; 300]).unwrap();
    w.start_file("lib/arm64-v8a/libx.so", stored).unwrap();
    w.write_all(b"\x7fELF-not-really").unwrap();
    w.start_file("assets/a.txt", deflated).unwrap();
    w.write_all(b"hello hello hello hello").unwrap();
    w.start_file("classes.dex", stored).unwrap();
    w.write_all(&[0u8; 200]).unwrap();
    w.start_file("META-INF/CERT.RSA", stored).unwrap();
    w.write_all(b"old signature").unwrap();
    w.finish().unwrap();
    p
}

#[test]
fn edits_alignment_and_old_signatures() {
    let src = sample();
    let mut edits = Edits::new();
    edits.insert("assets/a.txt".into(), Some(b"replaced".to_vec()));
    edits.insert("assets/new.bin".into(), Some(vec![7; 10]));
    edits.insert("classes.dex".into(), Some(vec![1; 300]));
    edits.insert("AndroidManifest.xml".into(), None);
    let (bytes, cd) = rebuild(src.to_str().unwrap(), &edits).unwrap();
    let out = std::env::temp_dir().join(format!("apkscope-rebuilt-{}.apk", std::process::id()));
    std::fs::write(&out, &bytes).unwrap();
    let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let names: Vec<String> = z
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .collect();
    assert_eq!(
        names,
        vec![
            "lib/arm64-v8a/libx.so",
            "assets/a.txt",
            "classes.dex",
            "assets/new.bin"
        ]
    );
    // a replaced file is stored when the one it replaces was, deflated otherwise
    let method = |z: &mut zip::ZipArchive<std::fs::File>, n| z.by_name(n).unwrap().compression();
    assert_eq!(
        method(&mut z, "classes.dex"),
        zip::CompressionMethod::Stored
    );
    assert_eq!(
        z.by_name("classes.dex").unwrap().data_start().unwrap() % 4,
        0
    );
    assert_eq!(
        method(&mut z, "assets/new.bin"),
        zip::CompressionMethod::Deflated
    );
    let mut s = String::new();
    z.by_name("assets/a.txt")
        .unwrap()
        .read_to_string(&mut s)
        .unwrap();
    assert_eq!(s, "replaced");
    // the .so is stored and page-aligned, so Android can map it in place
    let so = z.by_name("lib/arm64-v8a/libx.so").unwrap();
    assert_eq!(so.data_start().unwrap() % 4096, 0);
    assert_eq!(
        u32::from_le_bytes(bytes[bytes.len() - 6..bytes.len() - 2].try_into().unwrap()) as usize,
        cd
    );
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn plain_jar_keeps_its_manifest() {
    use std::io::Write;
    let p = std::env::temp_dir().join(format!("apkscope-rebuild-{}.jar", std::process::id()));
    let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    w.start_file("META-INF/MANIFEST.MF", opts).unwrap();
    w.write_all(b"Manifest-Version: 1.0\n").unwrap();
    w.start_file("classes.dex", opts).unwrap();
    w.write_all(&[0u8; 100]).unwrap();
    w.finish().unwrap();
    let (bytes, _) = rebuild(p.to_str().unwrap(), &Edits::new()).unwrap();
    let z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let names: Vec<_> = z.file_names().flatten().collect();
    assert_eq!(names, ["META-INF/MANIFEST.MF", "classes.dex"]);
    let _ = std::fs::remove_file(p);
}

#[test]
fn v1_signature_names() {
    assert!(
        is_v1_signature("META-INF/CERT.RSA")
            && is_v1_signature("META-INF/MANIFEST.MF")
            && is_v1_signature("META-INF/x.SF")
    );
    assert!(
        !is_v1_signature("META-INF/services/foo")
            && !is_v1_signature("META-INF/kotlin.kotlin_module")
    );
}
