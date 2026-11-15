use super::*;

/// Every binary XML file under a directory, and in every .apk there, writes back to the
/// same bytes:
/// APKSCOPE_XML=<dir> cargo test --release xml_round_trip -- --ignored --nocapture
#[test]
#[ignore]
fn xml_round_trip() {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        use std::io::Read;
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "apk") {
                let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
                for i in 0..z.len() {
                    let mut e = z.by_index(i).unwrap();
                    let mut b = vec![];
                    if e.read_to_end(&mut b).is_ok() && b.starts_with(&[3, 0, 8, 0]) {
                        out.push((
                            format!("{}!{}", path.display(), e.name().unwrap_or_default()),
                            b,
                        ));
                    }
                }
            } else {
                let b = std::fs::read(&path).unwrap();
                if b.starts_with(&[3, 0, 8, 0]) {
                    out.push((path.display().to_string(), b));
                }
            }
        }
    }
    let mut files = vec![];
    walk(
        std::env::var("APKSCOPE_XML")
            .expect("set APKSCOPE_XML")
            .as_ref(),
        &mut files,
    );
    let mut differ = 0;
    for (name, bytes) in &files {
        let doc = read(bytes).unwrap_or_else(|e| panic!("{name}: {}", e.report()));
        if write(&doc) != *bytes {
            differ += 1;
            if differ <= 5 {
                let at = write(&doc).iter().zip(bytes).position(|(a, b)| a != b);
                println!(
                    "{name}: differs at {at:?} ({} vs {} bytes)",
                    write(&doc).len(),
                    bytes.len()
                );
            }
        }
    }
    println!("{} files, {differ} write back differently", files.len());
    assert_eq!(differ, 0);
}

/// Decoding every binary XML file of a corpus and applying the text to it gives the same
/// bytes: one directory per APK, its files and its resources.arsc, which names resources:
/// APKSCOPE_XML_APKS=<dir> cargo test --release text_round_trip -- --ignored --nocapture
#[test]
#[ignore]
fn text_round_trip() {
    let dir = std::env::var("APKSCOPE_XML_APKS").expect("set APKSCOPE_XML_APKS");
    let (mut files, mut failed) = (0, 0);
    for apk in std::fs::read_dir(dir).unwrap() {
        let apk = apk.unwrap().path();
        let table = std::fs::read(apk.join("resources.arsc"))
            .ok()
            .and_then(|b| crate::table::read(&b).ok());
        let names = crate::res::Names::new(&table.unwrap_or(crate::table::Table {
            strings: crate::table::StringPool {
                utf8: true,
                sorted: false,
                strings: vec![],
                styles: vec![],
            },
            packages: vec![],
        }));
        for f in std::fs::read_dir(&apk).unwrap() {
            let f = f.unwrap().path();
            let bytes = std::fs::read(&f).unwrap();
            if !bytes.starts_with(&[3, 0, 8, 0]) {
                continue;
            }
            files += 1;
            let doc = read(&bytes).unwrap();
            let text = text::decode(&doc, &names);
            let back = text::apply(&doc, &text, &names).map(|d| write(&d));
            if back.as_ref().ok() != Some(&bytes) {
                failed += 1;
                if failed <= 8 {
                    match back {
                        Err(e) => println!("{}: {}", f.display(), e.report()),
                        Ok(b) => {
                            let at = b.iter().zip(&bytes).position(|(x, y)| x != y);
                            println!(
                                "{}: differs at {at:?} ({} vs {} bytes)",
                                f.display(),
                                b.len(),
                                bytes.len()
                            );
                        }
                    }
                }
            }
        }
    }
    println!("{files} files, {failed} not given back");
    assert_eq!(failed, 0);
}
