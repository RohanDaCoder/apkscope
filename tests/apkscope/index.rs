use super::*;

#[test]
fn rows_group_and_dedup() {
    let r = Rows::build(4, vec![(2, 7), (0, 1), (2, 3), (2, 7), (9, 9)]);
    assert_eq!(r.get(0), &[1]);
    assert_eq!(r.get(1), &[] as &[u32]);
    assert_eq!(r.get(2), &[3, 7]);
    // out-of-range ids are dropped, lookups past the end are empty
    assert_eq!(r.get(9), &[] as &[u32]);
}
