use super::*;
use crate::ir::Expr::*;

#[test]
fn substitute_respects_order() {
    let call = |m| Invoke(crate::ir::Call::Static, m, vec![]);
    // v0 = f(); use(g(), v0) must not become use(g(), f())
    let mut target = Invoke(crate::ir::Call::Static, 9, vec![call(1), Reg(0)]);
    assert!(!substitute(vec![&mut target], 0, &call(2)));
    let mut target = Invoke(crate::ir::Call::Static, 9, vec![Reg(0), call(1)]);
    assert!(substitute(vec![&mut target], 0, &call(2)));
    assert_eq!(
        target,
        Invoke(crate::ir::Call::Static, 9, vec![call(2), call(1)])
    );
}
