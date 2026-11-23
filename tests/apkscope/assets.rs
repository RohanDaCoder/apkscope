use super::*;

/// A Hermes file with no functions and the given strings: ASCII, UTF-16, and one long enough
/// (>= 255) to live in the overflow table.
fn hermes(strings: &[(&str, bool)]) -> Vec<u8> {
    let mut storage = Vec::new();
    let mut small = Vec::new();
    let mut overflow = Vec::new();
    for (s, utf16) in strings {
        let off = storage.len() as u32;
        let len = if *utf16 {
            for u in s.encode_utf16() {
                storage.extend_from_slice(&u.to_le_bytes());
            }
            s.encode_utf16().count() as u32
        } else {
            storage.extend_from_slice(s.as_bytes());
            s.len() as u32
        };
        let entry = if len >= 255 {
            overflow.push((off, len));
            (*utf16 as u32) | (((overflow.len() as u32 - 1) & 0x7f_ffff) << 1) | (0xff << 24)
        } else {
            (*utf16 as u32) | ((off & 0x7f_ffff) << 1) | (len << 24)
        };
        small.push(entry);
    }
    let mut d = vec![0u8; 128];
    d[..8].copy_from_slice(&[0xC6, 0x1F, 0xBC, 0x03, 0xC1, 0x03, 0x19, 0x1F]);
    d[8..12].copy_from_slice(&96u32.to_le_bytes());
    d[52..56].copy_from_slice(&(small.len() as u32).to_le_bytes());
    d[56..60].copy_from_slice(&(overflow.len() as u32).to_le_bytes());
    d[60..64].copy_from_slice(&(storage.len() as u32).to_le_bytes());
    for e in &small {
        d.extend_from_slice(&e.to_le_bytes());
    }
    for (o, l) in &overflow {
        d.extend_from_slice(&o.to_le_bytes());
        d.extend_from_slice(&l.to_le_bytes());
    }
    d.extend_from_slice(&storage);
    let n = d.len() as u32;
    d[32..36].copy_from_slice(&n.to_le_bytes());
    d
}

#[test]
fn reads_the_string_table_of_a_hermes_bundle() {
    let long = format!("https://api.example.com/{}", "a".repeat(300));
    let d = hermes(&[
        ("https://api.example.com/v1/login", false),
        ("Grüße", true),
        (&long, false),
        ("x-api-key", false),
    ]);
    assert!(is_hermes(&d));
    let s = hermes_strings(&d).unwrap();
    assert_eq!(
        s,
        vec![
            "https://api.example.com/v1/login".to_string(),
            "Grüße".into(),
            long,
            "x-api-key".into()
        ]
    );
}

#[test]
fn rejects_what_is_not_a_hermes_layout() {
    assert!(hermes_strings(b"var a = 1;").is_none());
    // right magic, but a string table that runs past the file
    let mut d = hermes(&[("abc", false)]);
    d[52..56].copy_from_slice(&1000u32.to_le_bytes());
    assert!(hermes_strings(&d).is_none());
}
