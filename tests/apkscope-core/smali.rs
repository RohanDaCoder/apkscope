use super::*;
use crate::dex::Dex;

/// written DEX -> model -> written again must give the same bytes, also through Smali text
fn roundtrip(name: &str, orig: &[u8]) -> (usize, usize) {
    let classes = read::read(orig).unwrap_or_else(|e| panic!("{name}: read: {e}"));
    let d1 = write::write(&classes, version(orig)).unwrap_or_else(|e| panic!("{name}: write: {e}"));
    let c1 = read::read(&d1).unwrap_or_else(|e| panic!("{name}: re-read: {e}"));
    let again = write::write(&c1, version(orig)).unwrap();
    if d1 != again {
        let by: std::collections::HashMap<&[u8], &model::Class> =
            c1.iter().map(|c| (&*c.name, c)).collect();
        for a in &classes {
            let b = by[&*a.name];
            if a != b {
                for (ma, mb) in a.methods.iter().zip(&b.methods) {
                    if ma != mb {
                        let (Some(ca), Some(cb)) = (ma.code.as_ref(), mb.code.as_ref()) else {
                            panic!(
                                "{name}: {:?}
{:?}",
                                ma, mb
                            )
                        };
                        let one = |m: &model::Method| {
                            text::print(&model::Class {
                                methods: vec![m.clone()],
                                fields: vec![],
                                ..a.clone()
                            })
                        };
                        std::fs::write("rt_a.smali", one(ma)).unwrap();
                        std::fs::write("rt_b.smali", one(mb)).unwrap();
                        for (x, y) in ca.items.iter().zip(&cb.items) {
                            assert_eq!(
                                x,
                                y,
                                "{name}: {} {}",
                                String::from_utf8_lossy(&a.name),
                                String::from_utf8_lossy(&ma.name)
                            );
                        }
                        assert_eq!(ca, cb);
                    }
                }
                assert_eq!(a, b, "{name}: writing is not stable");
            }
        }
        let at = 0x20
            + d1[0x20..]
                .iter()
                .zip(&again[0x20..])
                .position(|(x, y)| x != y)
                .unwrap_or(0);
        let map = u32::from_le_bytes(d1[0x34..0x38].try_into().unwrap()) as usize;
        let n = u32::from_le_bytes(d1[map..map + 4].try_into().unwrap()) as usize;
        let secs: Vec<String> = (0..n)
            .map(|k| {
                let o = map + 4 + k * 12;
                format!(
                    "{:#x}@{:#x}",
                    u16::from_le_bytes(d1[o..o + 2].try_into().unwrap()),
                    u32::from_le_bytes(d1[o + 8..o + 12].try_into().unwrap())
                )
            })
            .collect();
        panic!(
            "{name}: writing is not stable: first diff at {at:#x} of {:#x}/{:#x}; sections {secs:?}",
            d1.len(),
            again.len()
        );
    }
    let c2: Vec<_> = c1
        .iter()
        .map(|c| {
            let t = text::print(c);
            text::parse(&t).unwrap_or_else(|e| {
                panic!("{name}: {}: {e}\n{t}", String::from_utf8_lossy(&c.name))
            })
        })
        .collect();
    let d2 = write::write(&c2, version(orig)).unwrap();
    if d1 != d2 {
        for (a, b) in c1.iter().zip(&c2) {
            if a != b {
                for (ma, mb) in a.methods.iter().zip(&b.methods) {
                    assert_eq!(
                        ma,
                        mb,
                        "{name}: {} differs after Smali\n{}",
                        String::from_utf8_lossy(&a.name),
                        text::print(a)
                    );
                }
                assert_eq!(a, b, "{name}: class differs after Smali");
            }
        }
        panic!("{name}: Smali round trip changed the DEX");
    }
    same_meaning(name, orig, &d1);
    (classes.len(), d1.len())
}

/// the rewritten DEX lists the same classes, members and instructions as the original
fn same_meaning(name: &str, a: &[u8], b: &[u8]) {
    let (x, y) = (Dex::parse(a).unwrap(), Dex::parse(b).unwrap());
    assert_eq!(x.class_count, y.class_count, "{name}: class count");
    let desc = |d: &Dex, i| d.class_descriptor(i).unwrap();
    let body = |d: &Dex, i: usize| -> Vec<String> {
        let cd = d.class_def(i).unwrap();
        let mut v = vec![format!(
            "{} {:x} {:?} {:?} {:?}",
            cd.class, cd.access, cd.superclass, cd.interfaces, cd.source_file
        )];
        let data = d.class_data(i).unwrap();
        for (f, a) in data.fields {
            v.push(format!("F {} {a:x}", d.field_ref(f).unwrap()));
        }
        for m in data.methods {
            v.push(format!(
                "M {} {:x}",
                d.method_ref(m.method_idx).unwrap(),
                m.access
            ));
            if let Some(c) = m.code {
                v.push(format!(
                    "  regs {} ins {} tries {}",
                    c.registers,
                    c.ins,
                    c.tries.len()
                ));
                for i in crate::bytecode::decode(c.insns).unwrap() {
                    v.push(crate::dex::render(d, &i).unwrap_or_else(|e| e.to_string()));
                }
            }
        }
        v.push(
            format!("{:?}", d.static_values(i).unwrap())
                .chars()
                .filter(|c| !c.is_ascii_digit())
                .collect(),
        );
        v
    };
    let mut by: std::collections::HashMap<String, usize> =
        (0..y.class_count).map(|i| (desc(&y, i), i)).collect();
    for i in 0..x.class_count {
        let j = by
            .remove(&desc(&x, i))
            .unwrap_or_else(|| panic!("{name}: {} missing", desc(&x, i)));
        let (p, q) = (body(&x, i), body(&y, j));
        for (l1, l2) in p.iter().zip(&q) {
            // branch offsets print as numbers and stay equal unless code grew
            assert_eq!(l1, l2, "{name}: {} differs", desc(&x, i));
        }
        assert_eq!(
            p.len(),
            q.len(),
            "{name}: {} differs in length",
            desc(&x, i)
        );
    }
}

#[test]
fn assembles_hand_written_smali() {
    let src = r#"
.class public Lcom/example/Hello;
.super Ljava/lang/Object;
.source "Hello.java"

.field private static final GREETING:Ljava/lang/String; = "hi \"there\"\n"
.field public count:I

.method public constructor <init>()V
    .locals 0
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method

.method public static pick(I)I
    .locals 1
    .param p0, "which"
    .line 10
    packed-switch p0, :pswitch_data_0
    const/4 v0, -0x1
    :goto_0
    return v0
    :pswitch_0
    const/16 v0, 0x64
    goto :goto_0
    :pswitch_1
    :try_start_0
    const-string v0, "x"
    invoke-static {v0}, Ljava/lang/Integer;->parseInt(Ljava/lang/String;)I
    move-result v0
    :try_end_0
    .catch Ljava/lang/NumberFormatException; {:try_start_0 .. :try_end_0} :catch_0
    goto :goto_0
    :catch_0
    const/4 v0, 0x0
    goto :goto_0
    :pswitch_data_0
    .packed-switch 0x1
        :pswitch_0
        :pswitch_1
    .end packed-switch
.end method
"#;
    let c = text::parse(src).unwrap();
    let d = write::write(&[c.clone()], *b"035").unwrap();
    let x = Dex::parse(&d).unwrap();
    assert_eq!(x.class_descriptor(0).unwrap(), "Lcom/example/Hello;");
    let back = read::read(&d).unwrap();
    // the printed form of what was written parses back to the same DEX
    let again = write::write(&[text::parse(&text::print(&back[0])).unwrap()], *b"035").unwrap();
    assert_eq!(d, again);
    assert_eq!(
        back[0].fields[0].init,
        Some(model::Value::Str(model::s(b"hi \"there\"\n")))
    );
    let m = x.methods(0).unwrap();
    assert_eq!(m.len(), 2);
    assert_eq!(m[1].code.as_ref().unwrap().tries.len(), 1);
    // DEX header checksum verifies
    let mut sealed = d.clone();
    write::seal(&mut sealed);
    assert_eq!(sealed, d);
}

#[test]
fn parses_a_method_and_an_operand_alone() {
    use model::{Item, Operand, Ref};
    let mut m = text::parse_method(
        r#"
.method public static pick(I)Ljava/lang/String;
    .locals 2
    const v0, 0x5a0
    const-wide/16 v0, 0x5a0
    const/high16 v0, 0x40000000    # 2.0f
    const-string v1, "a \"b\""
    return-object v1
.end method
"#,
    )
    .unwrap();
    assert_eq!(
        text::parse_signature("pick(I)Ljava/lang/String;").unwrap(),
        (m.name.clone(), m.proto.clone())
    );
    let count = |m: &model::Method, o: &Operand| {
        let items = &m.code.as_ref().unwrap().items;
        items
            .iter()
            .filter(|i| matches!(i, Item::Ins(i) if i.has(o)))
            .count()
    };
    // a number matches by value, in the narrow and the wide instruction, and nowhere a literal
    // is not held: `return-object` has `lit` 0 without holding a 0
    let n = text::parse_operand("0x5a0").unwrap();
    assert_eq!(n, Operand::Lit(0x5a0));
    assert_eq!(count(&m, &n), 2);
    assert_eq!(count(&m, &text::parse_operand("0x0").unwrap()), 0);
    assert_eq!(count(&m, &text::parse_operand("2.0f").unwrap()), 1);
    let s = text::parse_operand(r#""a \"b\"""#).unwrap();
    assert_eq!(s, Operand::Ref(Ref::Str(model::s(b"a \"b\""))));
    assert_eq!(count(&m, &s), 1);
    assert_eq!(
        text::parse_operand("Lcom/example/A;->f:I").unwrap(),
        Operand::Ref(Ref::Field(model::FieldRef {
            class: model::s(b"Lcom/example/A;"),
            name: model::s(b"f"),
            ty: model::s(b"I"),
        }))
    );
    assert!(matches!(
        text::parse_operand("Lcom/example/A;->f(I)V").unwrap(),
        Operand::Ref(Ref::Method(_))
    ));
    assert_eq!(
        text::parse_operand("[Lcom/example/A;").unwrap(),
        Operand::Ref(Ref::Type(model::s(b"[Lcom/example/A;")))
    );
    assert!(text::parse_operand("nonsense").is_err());

    let to = text::parse_operand("0x4c4").unwrap();
    for item in &mut m.code.as_mut().unwrap().items {
        if let Item::Ins(i) = item {
            i.replace(&n, &to);
        }
    }
    assert_eq!((count(&m, &n), count(&m, &to)), (0, 2));
    assert!(text::parse_method(".method public a()V\n.end method\n.field x:I").is_err());
}

#[test]
fn local_type_is_a_type_index() {
    let src = ".class LA;\n.super Ljava/lang/Object;\n.method static f()V\n    .locals 1\n    const/4 v0, 0x0\n    .local v0, \"s\":Ljava/lang/String;\n    return-void\n.end method\n";
    let c = text::parse(src).unwrap();
    let d = write::write(std::slice::from_ref(&c), *b"035").unwrap();
    assert_eq!(read::read(&d).unwrap(), [c]);
    let at = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
    // the one code item's debug info: line start, no parameters, advance pc, start local
    let map = at(0x34);
    let code = (0..at(map))
        .map(|k| map + 4 + k * 12)
        .find(|&o| d[o..o + 2] == [0x01, 0x20])
        .map(|o| at(o + 8))
        .unwrap();
    let debug = at(code + 8);
    assert_eq!(d[debug + 1..debug + 6], [0, 1, 1, 3, 0]);
    // DBG_START_LOCAL names its type by type index (plus one), not by string index
    let ty = d[debug + 7] as usize - 1;
    let string = at(at(0x3c) + at(at(0x44) + ty * 4) * 4);
    assert_eq!(&d[string + 1..string + 19], b"Ljava/lang/String;");
}

#[test]
fn reports_errors_with_line() {
    let e = text::parse(".class LA;\n.super Ljava/lang/Object;\n.method static f()V\n    .locals 0\n    bogus-op v0\n.end method\n").unwrap_err().to_string();
    assert!(e.starts_with("line 5:"), "{e}");
    let e = text::parse(
        ".class LA;\n.method static f()V\n    .locals 0\n    goto :nowhere\n.end method\n",
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains(":nowhere"), "{e}");
}

#[test]
fn edits_an_apk_in_memory() {
    use std::io::{Cursor, Write};
    let src = ".class public Lcom/ex/M;\n.super Ljava/lang/Object;\n\
                   .method public static f()I\n    .registers 1\n    const/16 v0, 0x64\n    return v0\n.end method\n";
    let dex = write::write(&[text::parse(src).unwrap()], *b"035").unwrap();
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("classes.dex", stored).unwrap();
    w.write_all(&dex).unwrap();
    let apk = w.finish().unwrap().into_inner();

    let mut open = Apk::new(Cursor::new(&apk)).unwrap();
    let class = export_class_from(&mut open, "com.ex.M").unwrap();
    let edited = class.replace("0x64", "0x65");
    let mut edits = crate::rebuild::Edits::new();
    let changed = inject_into(&mut open, &[edited], &mut edits).unwrap();
    assert_eq!(changed, ["classes.dex"]);
    let (out, _) = crate::rebuild::rebuild_from(Cursor::new(&apk), &edits).unwrap();

    let mut rebuilt = Apk::new(Cursor::new(out)).unwrap();
    assert_eq!(rebuilt.dex.len(), 1);
    assert!(
        export_class_from(&mut rebuilt, "Lcom/ex/M;")
            .unwrap()
            .contains("0x65")
    );
}

#[test]
fn edits_classes_of_open_documents() {
    use model::{Item, Operand};
    let class = |name: &str, body: &str| {
        text::parse(&format!(
            ".class public L{name};\n.super Ljava/lang/Object;\n\
             .method public static f()Ljava/lang/String;\n    .registers 1\n{body}\
             \n    return-object v0\n.end method\n"
        ))
        .unwrap()
    };
    let a = class("a/A", "    const-string v0, \"one\"");
    let b = class("a/B", "    const/16 v0, 0x64\n    const-string v0, \"two\"");
    let dex = |c: &[model::Class]| write::write(c, *b"035").unwrap();
    let mut classes = Classes::new(vec![
        ("classes.dex".into(), dex(&[a.clone()])),
        ("classes2.dex".into(), dex(&[b.clone()])),
    ]);
    let (one, two) = (
        text::parse_operand("\"one\"").unwrap(),
        text::parse_operand("\"two\"").unwrap(),
    );
    let n = Operand::Lit(0x64);
    assert_eq!(classes.find(&one).unwrap(), ["La/A;"]);
    assert_eq!(classes.find(&n).unwrap(), ["La/B;"]);
    // `return-object` holds no literal, and a string in no table is in no class
    assert!(classes.find(&Operand::Lit(0)).unwrap().is_empty());
    assert!(
        classes
            .find(&text::parse_operand("\"three\"").unwrap())
            .unwrap()
            .is_empty()
    );

    // reading a class, or setting it as it is, is no edit
    let mut edits = crate::rebuild::Edits::new();
    let read = classes.class("a.B").unwrap();
    assert_eq!(read, b);
    classes.set(read.clone()).unwrap();
    assert!(classes.write(&mut edits).unwrap().is_empty());

    // an edit is found as edited, and only its file is written
    let mut edited = read;
    for item in &mut edited.methods[0].code.as_mut().unwrap().items {
        if let Item::Ins(i) = item {
            i.replace(&two, &one);
        }
    }
    classes.set(edited.clone()).unwrap();
    assert_eq!(classes.find(&one).unwrap(), ["La/A;", "La/B;"]);
    assert!(classes.find(&two).unwrap().is_empty());
    assert_eq!(classes.write(&mut edits).unwrap(), ["classes2.dex"]);
    let written = edits["classes2.dex"].clone().unwrap();
    // "one" is not in classes2.dex's tables, so the file was written again, not patched
    assert_eq!(read::read(&written).unwrap(), [edited.clone()]);
    // a file written again one class at a time is the file its classes write together
    assert_eq!(written, dex(&[edited]));

    // setting it back is no edit again; a new class goes to classes.dex
    classes.set(b).unwrap();
    classes
        .set(class("a/C", "    const-string v0, \"new\""))
        .unwrap();
    let mut edits = crate::rebuild::Edits::new();
    assert_eq!(classes.write(&mut edits).unwrap(), ["classes.dex"]);
    let written = read::read(edits["classes.dex"].as_ref().unwrap()).unwrap();
    assert_eq!(written.len(), 2);
    assert_eq!(classes.class("La/C;").unwrap().name, model::s(b"La/C;"));
    assert!(classes.class("a.Missing").is_err());
}

#[test]
fn patches_instruction_in_place() {
    let src = ".class public Lcom/ex/P;\n.super Ljava/lang/Object;\n\
                   .method public static f()I\n    .registers 1\n    const/16 v0, 0x64\n    return v0\n.end method\n";
    let dex = write::write(&[text::parse(src).unwrap()], *b"035").unwrap();
    let ci = Dex::parse(&dex).unwrap().find_class("Lcom/ex/P;").unwrap();

    // same-width immediate change: patched in place, same size, only the operand (and header
    // checksum/signature) differ, and it decodes to the new value
    let edited = text::parse(&src.replace("0x64", "0x65")).unwrap();
    let patched = write::patch(&dex, &[(ci, &edited)])
        .unwrap()
        .expect("same-width edit should patch in place");
    assert_eq!(patched.len(), dex.len());
    let back = read::read(&patched).unwrap();
    let lit = back[0].methods[0]
        .code
        .as_ref()
        .unwrap()
        .items
        .iter()
        .find_map(|it| match it {
            model::Item::Ins(i) if i.op == 0x13 => Some(i.lit),
            _ => None,
        })
        .unwrap();
    assert_eq!(lit, 0x65);

    // adding an instruction changes the layout, so it must fall back (returns None)
    let grew = text::parse(&src.replace("    return v0", "    nop\n    return v0")).unwrap();
    assert!(write::patch(&dex, &[(ci, &grew)]).unwrap().is_none());
}

#[test]
fn keeps_hiddenapi_flags() {
    let hidden = r#"
.class public Lcom/example/Hidden;
.super Ljava/lang/Object;

.field public static whitelist test-api OPEN:I
.field private blacklist whitelist:I

.method public constructor greylist-max-o core-platform-api <init>()V
    .locals 0
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method

.method public greylist-max-s later()V
    .locals 0
    return-void
.end method

.method public hiddenapi-0x48 unknown()V
    .locals 0
    return-void
.end method
"#;
    let plain =
        ".class public Lcom/example/Plain;\n.super Ljava/lang/Object;\n\n.field public a:I\n";
    let classes = [text::parse(plain).unwrap(), text::parse(hidden).unwrap()];
    assert_eq!(classes[1].fields[0].hidden, Some(0x20));
    assert_eq!(classes[1].fields[1].hidden, Some(2));
    assert_eq!(classes[1].fields[1].name, model::s(b"whitelist"));
    let d = write::write(&classes, *b"039").unwrap();
    let back = read::read(&d).unwrap();
    assert_eq!(back, classes);
    let flags = |c: &model::Class| c.methods.iter().map(|m| m.hidden).collect::<Vec<_>>();
    assert_eq!(flags(&back[1]), [Some(0x13), Some(7), Some(0x48)]);
    // a class without flags has none after the round trip, not the public list
    assert_eq!(back[0].fields[0].hidden, None);
    let printed = text::print(&back[1]);
    assert!(printed.contains(".method public hiddenapi-0x48 unknown()V"));
    assert_eq!(text::parse(&printed).unwrap(), back[1]);
    // a DEX without flags gets no hiddenapi section
    let d = write::write(&classes[..1], *b"039").unwrap();
    assert_eq!(read::Reader::new(&d).unwrap().class(0).unwrap(), classes[0]);
    let map = u32::from_le_bytes(d[0x34..0x38].try_into().unwrap()) as usize;
    let n = u32::from_le_bytes(d[map..map + 4].try_into().unwrap()) as usize;
    assert!((0..n).all(|k| d[map + 4 + k * 12..][..2] != [0x00, 0xf0]));
}

/// APKSCOPE_ROUNDTRIP=<dir of .apk> cargo test --release roundtrip_apks -- --ignored --nocapture
#[test]
#[ignore]
fn roundtrip_apks() {
    let dir = std::env::var("APKSCOPE_ROUNDTRIP").expect("set APKSCOPE_ROUNDTRIP");
    for e in std::fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        if path.extension().is_none_or(|x| x != "apk") {
            continue;
        }
        let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let names: Vec<String> = z
            .file_names()
            .filter_map(|n| n.ok().map(|n| n.into_owned()))
            .filter(|n| n.starts_with("classes") && n.ends_with(".dex"))
            .collect();
        let t = std::time::Instant::now();
        let (mut classes, mut bytes, mut orig) = (0, 0, 0);
        for n in names {
            let mut b = vec![];
            std::io::Read::read_to_end(&mut z.by_name(&n).unwrap(), &mut b).unwrap();
            let (c, size) = roundtrip(&format!("{}!{n}", path.display()), &b);
            classes += c;
            bytes += size;
            orig += b.len();
        }
        println!(
            "{}: {classes} classes, {orig} -> {bytes} bytes, {:?}",
            path.file_name().unwrap().to_string_lossy(),
            t.elapsed()
        );
    }
}
