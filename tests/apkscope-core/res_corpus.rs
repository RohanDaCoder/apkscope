use super::*;
use crate::table::{self, Chunk, EntryValue, Value};

/// Every dimension, fraction and float of every table in a directory prints as text that
/// encodes back to the same bits:
/// APKSCOPE_TABLES=<dir> cargo test --release numbers_round_trip -- --ignored --nocapture
#[test]
#[ignore]
fn numbers_round_trip() {
    let dir = std::env::var("APKSCOPE_TABLES").expect("set APKSCOPE_TABLES");
    let names = Names {
        map: HashMap::new(),
        app: 0x7f,
    };
    let (mut checked, mut wrong) = (0, 0);
    for e in std::fs::read_dir(dir).unwrap() {
        let Ok(t) = table::read(&std::fs::read(e.unwrap().path()).unwrap()) else {
            continue;
        };
        let mut values = vec![];
        for p in &t.packages {
            for c in &p.chunks {
                let Chunk::Type(ty) = c else { continue };
                for e in ty.entries.iter().flatten() {
                    match &e.value {
                        EntryValue::Simple(v) => values.push(*v),
                        EntryValue::Complex { map, .. } => values.extend(map.iter().map(|x| x.1)),
                    }
                }
            }
        }
        for v in values {
            let text = format_value(v, &[], &names);
            let back = match v.ty {
                Value::FLOAT => text.parse::<f32>().map(f32::to_bits).ok(),
                Value::DIMENSION | Value::FRACTION => {
                    let (units, scale): (&[&str], f32) = if v.ty == Value::DIMENSION {
                        (&DIMENSION_UNITS, 1.0)
                    } else {
                        (&FRACTION_UNITS, 0.01)
                    };
                    let unit = (v.data & 0xf) as usize;
                    let num = text.trim_end_matches(units.get(unit).copied().unwrap_or(""));
                    num.parse::<f32>()
                        .ok()
                        .map(|f| float_to_complex(f * scale, unit as u32))
                }
                _ => continue,
            };
            checked += 1;
            if back != Some(v.data) {
                wrong += 1;
                if wrong <= 10 {
                    println!("{:#x} {:#010x} -> {text} -> {back:x?}", v.ty, v.data);
                }
            }
        }
    }
    println!("{checked} numbers, {wrong} don't encode back");
    assert_eq!(wrong, 0);
}
