use super::*;

#[test]
fn adrp() {
    // adrp x0 at 0x12719f0 in a real game, +0x1c5a pages
    assert_eq!(adrp_page(0x12719f0, 0xd000e2c0), Some(0x2ecb000));
    assert_eq!(adrp_page(0x1000, 0x91182000), None); // add, not adrp
}

#[test]
fn literal_slots() {
    assert_eq!(literal_index(0xa000_8233), Some(16665));
    assert_eq!(literal_index(0x8000_0003), None); // kind 4: a method, not a string
    assert_eq!(literal_index(0x2d72c90), None); // a plain pointer
}
