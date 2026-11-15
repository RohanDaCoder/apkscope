use super::*;

fn pool(utf8: bool, strings: &[&str]) -> StringPool {
    StringPool {
        utf8,
        sorted: false,
        strings: strings.iter().map(|s| s.to_string()).collect(),
        styles: vec![],
    }
}

fn simple(flags: u16, key: u32, ty: u8, data: u32) -> Option<Entry> {
    Some(Entry {
        flags,
        key,
        value: EntryValue::Simple(Value { ty, data }),
    })
}

/// A table with every layout the writer knows: both pool encodings, styles, a string
/// outside the BMP, dense, sparse and 16-bit-offset types, compact, weak and complex
/// entries, and a chunk kept as read.
fn sample(utf8: bool) -> Table {
    let mut strings = pool(
        utf8,
        &["plain", "bold text", "b", "emoji 😀", &"x".repeat(200)],
    );
    strings.styles = vec![
        vec![],
        vec![Span {
            name: 2,
            first: 0,
            last: 3,
        }],
    ];
    let mut config = vec![0u8; 64];
    config[0] = 64;
    let mut night = config.clone();
    night[29] = 0x20;
    let ty = |flags, config: &Vec<u8>, entries| {
        Chunk::Type(Type {
            id: 1,
            flags,
            reserved: 0,
            config: config.clone(),
            entries,
        })
    };
    Table {
        strings,
        packages: vec![Package {
            id: 0x7f,
            name: "com.example".into(),
            last_public_type: 0,
            last_public_key: 0,
            type_id_offset: 0,
            types: pool(utf8, &["string", "style"]),
            keys: pool(utf8, &["a", "b", "c", "theme"]),
            chunks: vec![
                Chunk::Spec(Spec {
                    id: 1,
                    res0: 0,
                    res1: 3,
                    flags: vec![0, 0x200, 0],
                }),
                ty(
                    0,
                    &config,
                    vec![simple(0, 0, 3, 0), None, simple(Entry::PUBLIC, 2, 3, 1)],
                ),
                ty(
                    Type::SPARSE,
                    &night,
                    vec![None, simple(Entry::COMPACT, 1, 3, 3)],
                ),
                ty(
                    Type::OFFSET16,
                    &night,
                    vec![simple(Entry::WEAK, 0, 0x10, 7), None, None],
                ),
                Chunk::Spec(Spec {
                    id: 2,
                    res0: 0,
                    res1: 1,
                    flags: vec![0],
                }),
                Chunk::Type(Type {
                    id: 2,
                    flags: 0,
                    reserved: 0,
                    config: config.clone(),
                    entries: vec![Some(Entry {
                        flags: Entry::COMPLEX,
                        key: 3,
                        value: EntryValue::Complex {
                            parent: 0x01030005,
                            map: vec![(
                                0x01010036,
                                Value {
                                    ty: 0x12,
                                    data: 0xffff_ffff,
                                },
                            )],
                        },
                    })],
                }),
                Chunk::Other(vec![0x06, 0x02, 0x0c, 0, 0x0c, 0, 0, 0, 0, 0, 0, 0]),
            ],
        }],
    }
}

#[test]
fn writes_what_it_reads() {
    for utf8 in [true, false] {
        let t = sample(utf8);
        let bytes = write(&t);
        assert_eq!(read(&bytes).unwrap(), t, "utf8: {utf8}");
        assert_eq!(write(&read(&bytes).unwrap()), bytes);
    }
}

/// Every resources.arsc in a directory, and every one in an .apk there, must write back
/// to the same bytes:
/// APKSCOPE_TABLES=<dir> cargo test --release tables_round_trip -- --ignored --nocapture
#[test]
#[ignore]
fn tables_round_trip() {
    use std::io::Read;
    let dir = std::env::var("APKSCOPE_TABLES").expect("set APKSCOPE_TABLES");
    let mut checked = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        let bytes = match path.extension().and_then(|x| x.to_str()) {
            Some("arsc") => std::fs::read(&path).unwrap(),
            Some("apk") => {
                let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
                let Ok(mut e) = z.by_name("resources.arsc") else {
                    continue;
                };
                let mut b = vec![];
                e.read_to_end(&mut b).unwrap();
                b
            }
            _ => continue,
        };
        let t = read(&bytes).unwrap_or_else(|e| panic!("{}: {}", path.display(), e.report()));
        assert!(
            write(&t) == bytes,
            "{} writes back differently",
            path.display()
        );
        checked += 1;
    }
    println!("{checked} tables written back unchanged");
}
