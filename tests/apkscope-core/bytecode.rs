use super::*;

fn enc(units: &[u16]) -> Vec<u8> {
    units.iter().flat_map(|u| u.to_le_bytes()).collect()
}

#[test]
fn decodes_common_forms() {
    // const/4 v1, -1 ; invoke-virtual {v1, v2}, meth@7 ; if-eqz v0, -2 ; packed-switch payload ; return-void
    let code = enc(&[
        0xf112, 0x206e, 0x0007, 0x0021, 0x0038, 0xfffe, 0x0100, 0x0001, 0, 0, 0, 0, 0x000e,
    ]);
    let v = decode(&code).unwrap();
    assert_eq!(v.len(), 4);
    assert_eq!((v[0].op, v[0].regs[0], v[0].lit), (0x12, 1, -1));
    assert_eq!(
        (v[1].nregs, &v[1].regs[..2], v[1].idx),
        (2, &[1u16, 2][..], 7)
    );
    assert_eq!((v[2].off, v[2].lit), (4, -2));
    assert_eq!((v[3].op, v[3].off), (0x0e, 12));
}

#[test]
fn every_used_opcode_has_name() {
    let used = (0..=255u8).filter(|&o| info(o).is_some()).count();
    assert_eq!(used, 256 - 6 - 1 - 2 - 23); // 3e-43, 73, 79-7a, e3-f9 unused
    assert_eq!(info(0xb0).unwrap().0, "add-int/2addr");
    assert_eq!(info(0xcf).unwrap().0, "rem-double/2addr");
    assert_eq!(info(0xaf).unwrap().0, "rem-double");
    assert_eq!(info(0x5a).unwrap().0, "iput-wide");
}
