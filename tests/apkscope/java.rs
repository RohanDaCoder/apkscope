use super::*;

#[test]
fn hoists_declarations_used_after_their_block() {
    let mut l: Vec<(usize, String)> = vec![
        (2, "try {".into()),
        (3, "\u{1}a/Foo\u{2} foo = make();".into()),
        (3, "int i = 1;".into()),
        (2, "} finally {".into()),
        (3, "done();".into()),
        (2, "}".into()),
        (2, "use(foo);".into()),
    ];
    hoist_declarations(&mut l);
    assert_eq!(l[0], (2, "\u{1}a/Foo\u{2} foo;".into()));
    assert_eq!(l[2], (3, "foo = make();".into()));
    // i is not used after the block: left alone
    assert_eq!(l[3], (3, "int i = 1;".into()));
}

#[test]
fn enum_rewrites() {
    assert_eq!(
        split_args("a, f(b, c), \"x, y\""),
        vec!["a", "f(b, c)", "\"x, y\""]
    );
    let ctor = "    private Color(String str, int i, int i2) {\n        super(str, i);\n        this.rgb = i2;\n    }\n";
    assert_eq!(
        enum_ctor(ctor),
        "    Color(int i2) {\n        this.rgb = i2;\n    }\n"
    );
    assert_eq!(
        replace_word("a[i] = c; cc = c2;", "c", "RED"),
        "a[i] = RED; cc = c2;"
    );
    assert_eq!(
        constant("RED", &["\"RED\"".into(), "0".into(), "255".into()]),
        Some((0, "RED(255)".into()))
    );
}

#[test]
fn inner_names() {
    assert_eq!(inner_name("La/B$C;", "La/B;"), "C");
    assert_eq!(inner_name("La/B$1;", "La/B;"), "AnonymousClass1");
    assert_eq!(inner_name("La/B$$Lambda0;", "La/B;"), "$Lambda0");
}

#[test]
fn loop_keeps_its_body() {
    let d = crate::dex::tests::tiny_dex();
    let dex = Dex::parse(&d).unwrap();
    // 0: if-eqz v0, 5 ; 2: add-int/lit8 v1, v1, 1 ; 4: goto 0 ; 5: return-void
    let units: Vec<u8> = [0x0038u16, 0x0005, 0x01d8, 0x0101, 0xfc28, 0x000e]
        .iter()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let insns = bytecode::decode(&units).unwrap();
    let mut cfg = cfg::build(&insns, &units, &[]).unwrap();
    let mut bir: Bir = cfg
        .blocks
        .iter()
        .map(|b| ir::lower(&dex, &insns, &units, b.first, b.last, &b.term).unwrap())
        .collect();
    opt::inline(&cfg, &mut bir, 2);
    if opt::merge_conditions(&mut cfg, &mut bir) {
        cfg.recompute();
    }
    let ctx = Ctx {
        dex: &dex,
        class: "La/b/Cls;",
        ret: "V".into(),
        vars: None,
    };
    let mut e = Emitter::new(&ctx, &insns, &cfg, &bir);
    e.seq(0, None, 2);
    let out = e.finish();
    assert!(
        out.contains(
            "while (v0 != 0) {
            v1 = v1 + 1;
        }"
        ),
        "{out}"
    );
}

#[test]
fn loop_exits_meet_after_the_loop() {
    let d = crate::dex::tests::tiny_dex();
    let dex = Dex::parse(&d).unwrap();
    // 0: if-eqz v0, 7 ; 2: if-nez v1, 9 ; 4: add-int/lit8 v2, v2, 1 ; 6: goto 0
    // 7: const/4 v3, 0 ; 8: goto 10 ; 9: const/4 v3, 1 ; 10: return-void
    let units: Vec<u8> = [
        0x0038u16, 0x0007, 0x0139, 0x0007, 0x02d8, 0x0102, 0xfa28, 0x0312, 0x0228, 0x1312, 0x000e,
    ]
    .iter()
    .flat_map(|u| u.to_le_bytes())
    .collect();
    let insns = bytecode::decode(&units).unwrap();
    let cfg = cfg::build(&insns, &units, &[]).unwrap();
    let bir: Bir = cfg
        .blocks
        .iter()
        .map(|b| ir::lower(&dex, &insns, &units, b.first, b.last, &b.term).unwrap())
        .collect();
    let ctx = Ctx {
        dex: &dex,
        class: "La/b/Cls;",
        ret: "V".into(),
        vars: None,
    };
    let mut e = Emitter::new(&ctx, &insns, &cfg, &bir);
    e.seq(0, None, 2);
    let out = e.finish();
    assert!(
        !out.contains("goto") && out.contains("v3 = 0;") && out.contains("v3 = 1;"),
        "{out}"
    );
}

#[test]
fn try_catch_block() {
    let d = crate::dex::tests::tiny_dex();
    let dex = Dex::parse(&d).unwrap();
    // 0: throw v1  [try 0..1, catch-all -> 1] ; 1: move-exception v0 ; 2: return-void
    let units: Vec<u8> = [0x0127u16, 0x000d, 0x000e]
        .iter()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let tries = [crate::dex::Try {
        start: 0,
        count: 1,
        handlers: vec![(None, 1)],
    }];
    let insns = bytecode::decode(&units).unwrap();
    let cfg = cfg::build(&insns, &units, &tries).unwrap();
    let bir: Bir = cfg
        .blocks
        .iter()
        .map(|b| ir::lower(&dex, &insns, &units, b.first, b.last, &b.term).unwrap())
        .collect();
    let ctx = Ctx {
        dex: &dex,
        class: "La/b/Cls;",
        ret: "V".into(),
        vars: None,
    };
    let mut e = Emitter::new(&ctx, &insns, &cfg, &bir);
    e.seq(0, None, 2);
    let out = e.finish();
    assert!(
        out.contains(
            "try {
            throw v1;
        } catch (Throwable th) {"
        ),
        "{out}"
    );
}
