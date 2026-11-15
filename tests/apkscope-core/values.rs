use super::*;
use crate::table::{self, Chunk, config};

/// The first entries in which two tables of the same shape differ.
fn differences(a: &Table, b: &Table) -> Vec<String> {
    let mut out = vec![];
    if a.strings != b.strings {
        out.push(format!(
            "global pool: {} vs {} strings",
            a.strings.strings.len(),
            b.strings.strings.len()
        ));
    }
    for (p, q) in a.packages.iter().zip(&b.packages) {
        if p.keys != q.keys {
            out.push(format!(
                "keys: {} vs {}",
                p.keys.strings.len(),
                q.keys.strings.len()
            ));
        }
        if p.chunks.len() != q.chunks.len() {
            out.push(format!("chunks: {} vs {}", p.chunks.len(), q.chunks.len()));
        }
        for (c, d) in p.chunks.iter().zip(&q.chunks) {
            match (c, d) {
                (Chunk::Type(x), Chunk::Type(y)) if x != y => {
                    for (i, (e, f)) in x.entries.iter().zip(&y.entries).enumerate() {
                        if e != f && out.len() < 8 {
                            let id = p.id << 24 | (x.id as u32) << 16 | i as u32;
                            out.push(format!(
                                "{:?} in -{}: {e:?} vs {f:?}",
                                p.name(id),
                                config::name(&x.config)
                            ));
                        }
                    }
                }
                (Chunk::Spec(x), Chunk::Spec(y)) if x != y => out.push(format!("spec {}", x.id)),
                _ => {}
            }
        }
    }
    out
}

/// Decoding every table in a directory and applying what it decodes to gives the table back
/// unchanged:
/// APKSCOPE_TABLES=<dir> cargo test --release values_round_trip -- --ignored --nocapture
#[test]
#[ignore]
fn values_round_trip() {
    let dir = std::env::var("APKSCOPE_TABLES").expect("set APKSCOPE_TABLES");
    let (mut same, mut failed) = (0, 0);
    for e in std::fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        let Ok(t) = table::read(&std::fs::read(&path).unwrap()) else {
            continue;
        };
        let names = Names::new(&t);
        let decoded = res::decode_table(&t, &names);
        let mut back = t.clone();
        match apply(&mut back, &decoded.values, 0) {
            Err(e) => {
                failed += 1;
                println!("{}: {}", path.display(), e.report());
            }
            Ok(()) if back == t => same += 1,
            Ok(()) => {
                failed += 1;
                println!(
                    "{}:\n  {}",
                    path.display(),
                    differences(&t, &back).join("\n  ")
                );
            }
        }
    }
    println!("{same} tables unchanged, {failed} not");
    assert_eq!(failed, 0);
}
