use super::*;

#[test]
fn helpers() {
    assert_eq!(jtype("[[Ljava/lang/String;"), "java.lang.String[][]");
    assert_eq!(simple("Lcom/a/Outer$Inner;"), "Inner");
    assert_eq!(simple("[Lcom/a/B;"), "B[]");
    assert_eq!(name_for(Some("Lcom/x/WorldScreen;")), "worldScreen");
    assert_eq!(name_for(Some("Ljava/lang/Boolean;")), "booleanVal");
    assert_eq!(name_for(Some("[I")), "iArr");
    assert_eq!(lit(0, false, Some("Ljava/lang/Object;")), "null");
    assert_eq!(lit(1, false, Some("Z")), "true");
    assert_eq!(lit(0x3f800000, false, Some("F")), "1.0f");
    assert_eq!(java_string("a\"\n\u{e9}"), "\"a\\\"\\n\\u00e9\"");
    let c = Cmp("<", bx(Reg(0)), bx(Reg(1)));
    assert_eq!(
        And(bx(c.clone()), bx(Reg(2))).negate(),
        Or(bx(Cmp(">=", bx(Reg(0)), bx(Reg(1)))), bx(Not(bx(Reg(2)))))
    );
    assert_eq!(bin_op(0xaf - 0x90), "%");
    assert_eq!(bin_type(0xaf - 0x90), 'D');
}
