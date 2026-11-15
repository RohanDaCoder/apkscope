use super::*;

/// hand-built DEX: 1 string, 1 type, 1 class_def `a.b.Cls`
pub(crate) fn tiny_dex() -> Vec<u8> {
    let mut d = vec![0u8; 0x70];
    d[..8].copy_from_slice(b"dex\n035\0");
    let put =
        |d: &mut Vec<u8>, off: usize, v: u32| d[off..off + 4].copy_from_slice(&v.to_le_bytes());
    put(&mut d, 0x3C, 0x70); // string_ids
    put(&mut d, 0x44, 0x74); // type_ids
    put(&mut d, 0x60, 1);
    put(&mut d, 0x64, 0x78); // class_defs
    d.resize(0x78 + 32, 0);
    put(&mut d, 0x70, 0x98); // string data offset
    let s = b"\x0aLa/b/Cls;\0";
    d.extend_from_slice(s);
    d
}

#[test]
fn reads_class_name() {
    let d = tiny_dex();
    let dex = Dex::parse(&d).unwrap();
    assert_eq!(dex.class_descriptor(0).unwrap(), "La/b/Cls;");
    assert!(dex.methods(0).unwrap().is_empty());
    assert!(Dex::parse(b"nope").is_err());
}

/// a DEX inflated only through its names: the interface list is past the end, but the
/// superclass (what `refs extends` needs) still reads
#[test]
fn superclass_without_data_section() {
    let mut d = tiny_dex();
    d[0x78 + 8..0x78 + 12].copy_from_slice(&0u32.to_le_bytes()); // extends type 0
    d[0x78 + 12..0x78 + 16].copy_from_slice(&0x10000u32.to_le_bytes()); // interfaces, not inflated
    let dex = Dex::parse(&d).unwrap();
    assert_eq!(dex.superclass(0).unwrap().as_deref(), Some("La/b/Cls;"));
    assert!(dex.class_def(0).is_err());
}

#[test]
fn uleb() {
    let mut o = 0;
    assert_eq!(uleb128(&[0xe5, 0x8e, 0x26], &mut o).unwrap(), 624485);
    assert_eq!(o, 3);
    let mut o = 0;
    assert_eq!(sleb128(&[0x7f], &mut o).unwrap(), -1);
    let mut o = 0;
    assert_eq!(sleb128(&[0x80, 0x7f], &mut o).unwrap(), -128);
}
