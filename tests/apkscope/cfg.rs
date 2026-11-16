use super::*;

#[test]
fn diamond() {
    // 0 -> 1,2 -> 3
    let g = vec![vec![1, 2], vec![3], vec![3], vec![]];
    assert_eq!(dominators(&g, &[0]), vec![None, Some(0), Some(0), Some(0)]);
    let rev = vec![vec![], vec![0], vec![0], vec![1, 2]];
    assert_eq!(
        dominators(&rev, &[3]),
        vec![Some(3), Some(3), Some(3), None]
    );
}

#[test]
fn if_and_loop_blocks() {
    use crate::bytecode::decode;
    let enc = |u: &[u16]| u.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    // 0: if-eqz v0, +3 ; 2: const/4 v0,1 ; 3: goto -3 ; 4: return-void
    let units = enc(&[0x0038, 0x0004, 0x1012, 0xfd28, 0x000e]);
    let insns = decode(&units).unwrap();
    let cfg = build(&insns, &units, &[]).unwrap();
    assert_eq!(cfg.blocks.len(), 3);
    assert_eq!(cfg.blocks[0].term, Term::If { taken: 2, fall: 1 });
    assert_eq!(cfg.blocks[1].term, Term::Goto(0));
    assert!(cfg.dominates(0, 1));
    assert_eq!(cfg.ipdom[0], Some(2));
}
