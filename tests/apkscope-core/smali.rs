use super::*;
use crate::dex::Dex;
use model::{Class, Id, Ins, Item, Operand};

/// The DEX the classes the Smali `texts` are write.
fn assemble(texts: &[&str], version: [u8; 3]) -> Vec<u8> {
    super::assemble(texts, version).unwrap()
}

/// A DEX file's source and its classes.
fn open(dex: &[u8]) -> (Source, Vec<Class>) {
    let src = Source::new(dex.to_vec()).unwrap();
    let classes = read::read(&src).unwrap();
    (src, classes)
}

/// The classes of a DEX file as Smali: what two files can be compared by, as a model's ids are
/// only those of its own source.
fn smali(dex: &[u8]) -> Vec<String> {
    let (src, classes) = open(dex);
    classes.iter().map(|c| text::print(&src, c)).collect()
}

/// written DEX -> model -> written again must give the same bytes, also through Smali text
fn roundtrip(name: &str, orig: &[u8]) -> (usize, usize) {
    let v = version(orig);
    let (s0, classes) = open(orig);
    let d1 = write::write(&s0, &classes, v).unwrap_or_else(|e| panic!("{name}: write: {e}"));
    let (s1, c1) = open(&d1);
    let again = write::write(&s1, &c1, v).unwrap();
    if d1 != again {
        for (a, b) in classes.iter().zip(&c1) {
            assert_eq!(
                text::print(&s0, a),
                text::print(&s1, b),
                "{name}: writing is not stable"
            );
        }
        let at = 0x20
            + d1[0x20..]
                .iter()
                .zip(&again[0x20..])
                .position(|(x, y)| x != y)
                .unwrap_or(0);
        panic!(
            "{name}: writing is not stable: first diff at {at:#x} of {:#x}/{:#x}",
            d1.len(),
            again.len()
        );
    }
    let mut s2 = Source::default();
    let c2: Vec<Class> = c1
        .iter()
        .map(|c| {
            let t = text::print(&s1, c);
            text::parse(&mut s2, &t).unwrap_or_else(|e| panic!("{name}: {e}\n{t}"))
        })
        .collect();
    let d2 = write::write(&s2, &c2, v).unwrap();
    if d1 != d2 {
        for (a, b) in c1.iter().zip(&c2) {
            assert_eq!(
                text::print(&s1, a),
                text::print(&s2, b),
                "{name}: class differs after Smali"
            );
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
    let d = assemble(&[src], *b"035");
    let x = Dex::parse(&d).unwrap();
    assert_eq!(x.class_descriptor(0).unwrap(), "Lcom/example/Hello;");
    let (back_src, back) = open(&d);
    // the printed form of what was written parses back to the same DEX
    assert_eq!(assemble(&[&text::print(&back_src, &back[0])], *b"035"), d);
    let greeting = back_src.find_str(b"hi \"there\"\n").unwrap();
    assert_eq!(back[0].fields[0].init, Some(model::Value::Str(greeting)));
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
    let mut src = Source::default();
    let class = src.intern_descriptor(b"La/A;");
    let body = r#"
.method public static pick(I)Ljava/lang/String;
    .locals 2
    const v0, 0x5a0
    const-wide/16 v0, 0x5a0
    const/high16 v0, 0x40000000    # 2.0f
    const-string v1, "a \"b\""
    return-object v1
.end method
"#;
    let mut m = text::parse_method(&mut src, class, body).unwrap();
    let sig = "pick(I)Ljava/lang/String;";
    assert_eq!(text::find_method(&src, class, sig).unwrap(), Some(m.id));
    assert_eq!(text::find_method(&src, class, "pick()V").unwrap(), None);
    let count = |m: &model::Method, o: &Operand| {
        let items = &m.code.as_ref().unwrap().items;
        items
            .iter()
            .filter(|i| matches!(i, Item::Ins(i) if i.has(o)))
            .count()
    };
    // a number matches by value, in the narrow and the wide instruction, and nowhere a literal
    // is not held: `return-object` has `lit` 0 without holding a 0
    let n = text::parse_operand(&mut src, "0x5a0").unwrap();
    assert_eq!(n, Operand::Lit(0x5a0));
    assert_eq!(count(&m, &n), 2);
    assert_eq!(count(&m, &Operand::Lit(0)), 0);
    assert_eq!(
        count(&m, &text::parse_operand(&mut src, "2.0f").unwrap()),
        1
    );
    let string = r#""a \"b\"""#;
    let s = text::find_operand(&src, string).unwrap().unwrap();
    assert_eq!(s, Operand::Str(src.find_str(b"a \"b\"").unwrap()));
    assert_eq!(count(&m, &s), 1);

    // what a source lacks is found in none of it, and finding adds nothing to it
    let before = src.counts();
    for absent in ["\"no\"", "Lb/B;", "Lb/B;->f:I", "La/A;->pick()V"] {
        assert_eq!(text::find_operand(&src, absent).unwrap(), None);
    }
    assert_eq!(src.counts(), before);
    // parsing one adds it
    let field = text::parse_operand(&mut src, "Lb/B;->f:I").unwrap();
    assert_eq!(text::find_operand(&src, "Lb/B;->f:I").unwrap(), Some(field));
    assert!(matches!(
        text::parse_operand(&mut src, "La/A;->pick(I)Ljava/lang/String;").unwrap(),
        Operand::Method(id) if id == m.id
    ));
    assert!(matches!(
        text::parse_operand(&mut src, "[Lb/B;").unwrap(),
        Operand::Type(_)
    ));
    assert!(text::parse_operand(&mut src, "nonsense").is_err());

    let to = text::parse_operand(&mut src, "0x4c4").unwrap();
    let ty = text::parse_operand(&mut src, "Lb/B;").unwrap();
    let mut replaced = 0;
    for item in &mut m.code.as_mut().unwrap().items {
        if let Item::Ins(i) = item {
            replaced += usize::from(i.replace(&n, &to));
            // a type is no operand of a const-string, nor a number of anything but a literal
            assert!(!i.replace(&s, &ty));
            assert!(!i.replace(&s, &to));
        }
    }
    assert_eq!((replaced, count(&m, &n), count(&m, &to)), (2, 0, 2));
    assert_eq!(count(&m, &s), 1);
    let two = ".method public a()V\n.end method\n.field x:I";
    assert!(text::parse_method(&mut src, class, two).is_err());

    // an operand of another kind than an opcode takes is refused when the method is written
    if let Item::Ins(i) = &mut m.code.as_mut().unwrap().items[0] {
        i.operand = ty.clone();
    }
    let mut wrong =
        text::parse(&mut src, ".class public La/A;\n.super Ljava/lang/Object;\n").unwrap();
    wrong.methods.push(m);
    let e = write::write(&src, &[wrong], *b"035")
        .unwrap_err()
        .to_string();
    assert!(e.contains("another kind"), "{e}");
}

#[test]
fn local_type_is_a_type_index() {
    let src = ".class LA;\n.super Ljava/lang/Object;\n\n.method static f()V\n    .locals 1\n    const/4 v0, 0x0\n    .local v0, \"s\":Ljava/lang/String;\n    return-void\n.end method\n";
    let d = assemble(&[src], *b"035");
    assert_eq!(smali(&d), [src]);
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
    let parse = |t: &str| {
        text::parse(&mut Source::default(), t)
            .unwrap_err()
            .to_string()
    };
    let e = parse(
        ".class LA;\n.super Ljava/lang/Object;\n.method static f()V\n    .locals 0\n    bogus-op v0\n.end method\n",
    );
    assert!(e.starts_with("line 5:"), "{e}");
    let e =
        parse(".class LA;\n.method static f()V\n    .locals 0\n    goto :nowhere\n.end method\n");
    assert!(e.contains(":nowhere"), "{e}");
}

#[test]
fn edits_an_apk_in_memory() {
    use std::io::{Cursor, Write};
    let src = ".class public Lcom/ex/M;\n.super Ljava/lang/Object;\n\
                   .method public static f()I\n    .registers 1\n    const/16 v0, 0x64\n    return v0\n.end method\n";
    let dex = assemble(&[src], *b"035");
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
    assert_eq!(rebuilt.dex().unwrap().len(), 1);
    assert!(
        export_class_from(&mut rebuilt, "Lcom/ex/M;")
            .unwrap()
            .contains("0x65")
    );
}

#[test]
fn edits_classes_of_open_documents() {
    let class = |name: &str, body: &str| {
        format!(
            ".class public L{name};\n.super Ljava/lang/Object;\n\
             .method public static f()Ljava/lang/String;\n    .registers 1\n{body}\
             \n    return-object v0\n.end method\n"
        )
    };
    let a = class("a/A", "    const-string v0, \"one\"");
    let b = class("a/B", "    const/16 v0, 0x64\n    const-string v0, \"two\"");
    let mut classes = Classes::new(vec![
        ("classes.dex".into(), assemble(&[&a], *b"035")),
        ("classes2.dex".into(), assemble(&[&b], *b"035")),
    ]);
    assert_eq!(classes.find("\"one\"").unwrap(), ["La/A;"]);
    assert_eq!(classes.find("0x64").unwrap(), ["La/B;"]);
    // `return-object` holds no literal, and a string in no table is in no class
    assert!(classes.find("0x0").unwrap().is_empty());
    assert!(classes.find("\"three\"").unwrap().is_empty());

    // reading a class, or setting it as it is, is no edit
    let mut edits = crate::rebuild::Edits::new();
    let (doc, read) = classes.class("a.B").unwrap();
    doc.set(read.clone()).unwrap();
    assert!(classes.write(&mut edits).unwrap().is_empty());

    // an edit that needs a string the file lacks adds it to the file's source; the class is
    // found as edited, and only its file is written
    let (doc, _) = classes.class("a.B").unwrap();
    let two = text::find_operand(doc.source(), "\"two\"")
        .unwrap()
        .unwrap();
    assert_eq!(text::find_operand(doc.source(), "\"one\"").unwrap(), None);
    let one = text::parse_operand(doc.source_mut(), "\"one\"").unwrap();
    let mut edited = read.clone();
    for item in &mut edited.methods[0].code.as_mut().unwrap().items {
        if let Item::Ins(i) = item {
            i.replace(&two, &one);
        }
    }
    doc.set(edited).unwrap();
    assert_eq!(classes.find("\"one\"").unwrap(), ["La/A;", "La/B;"]);
    assert!(classes.find("\"two\"").unwrap().is_empty());
    assert_eq!(classes.write(&mut edits).unwrap(), ["classes2.dex"]);
    let written = edits["classes2.dex"].clone().unwrap();
    // "one" is not in classes2.dex's tables, so the file was written again, not patched: it is
    // the file the edited class writes from text, and "two", no longer used, is gone from it
    let as_text = b.replace("\"two\"", "\"one\"");
    assert_eq!(written, assemble(&[&as_text], *b"035"));
    assert!(Source::new(written).unwrap().find_str(b"two").is_none());

    // setting it back is no edit again; a new class goes to classes.dex
    let (doc, _) = classes.class("a.B").unwrap();
    doc.set(read).unwrap();
    classes
        .set_text(&class("a/C", "    const-string v0, \"new\""))
        .unwrap();
    let mut edits = crate::rebuild::Edits::new();
    assert_eq!(classes.write(&mut edits).unwrap(), ["classes.dex"]);
    let written = edits["classes.dex"].as_ref().unwrap();
    assert_eq!(smali(written).len(), 2);
    let (doc, c) = classes.class("La/C;").unwrap();
    assert_eq!(doc.source().descriptor(c.name), b"La/C;");
    assert!(classes.class("a.Missing").is_err());
}

#[test]
fn writes_a_file_again_from_outlines() {
    // a class with all a method's code can refer to: a string, a method, a caught type, a
    // parameter name, a local with its type
    let kept = r#".class public La/Kept;
.super Ljava/lang/Object;

.method public static f(I)I
    .locals 1
    .param p0, "which"
    .line 10
    :try_start_0
    const-string v0, "kept"
    invoke-static {v0}, Ljava/lang/Integer;->parseInt(Ljava/lang/String;)I
    move-result v0
    .local v0, "n":I
    :try_end_0
    .catch Ljava/lang/NumberFormatException; {:try_start_0 .. :try_end_0} :catch_0
    return v0
    :catch_0
    const/4 v0, 0x0
    return v0
.end method
"#;
    let edited = |s: &str| {
        format!(
            ".class public La/Edited;\n.super Ljava/lang/Object;\n\
             .method public static g()Ljava/lang/String;\n    .registers 1\n\
             \x20   const-string v0, \"{s}\"\n    return-object v0\n.end method\n"
        )
    };
    let mut classes = Classes::new(vec![(
        "classes.dex".into(),
        assemble(&[kept, &edited("old")], *b"035"),
    )]);
    // the edit adds a string, so the file is written again: La/Kept; is read from the file
    // without its code for what it refers to, and with it to be written
    classes.set_text(&edited("new")).unwrap();
    let mut edits = crate::rebuild::Edits::new();
    assert_eq!(classes.write(&mut edits).unwrap(), ["classes.dex"]);
    let written = edits["classes.dex"].clone().unwrap();
    // it is the file both classes write as models, so nothing the unedited one refers to is
    // lost or kept without need
    assert_eq!(written, assemble(&[kept, &edited("new")], *b"035"));
    let back = &smali(&written)[0];
    for part in [
        "const-string v0, \"kept\"",
        "Ljava/lang/Integer;->parseInt(Ljava/lang/String;)I",
        ".catch Ljava/lang/NumberFormatException;",
        ".param p0, \"which\"",
        ".local v0, \"n\":I",
    ] {
        assert!(back.contains(part), "{part} is missing from\n{back}");
    }
}

#[test]
fn patches_instruction_in_place() {
    let text = ".class public Lcom/ex/P;\n.super Ljava/lang/Object;\n\
                   .method public static f()I\n    .registers 1\n    const/16 v0, 0x64\n    return v0\n.end method\n";
    let dex = assemble(&[text], *b"035");
    let mut src = Source::new(dex.clone()).unwrap();
    let reader = read::Reader::new(&src).unwrap();

    // same-width immediate change: patched in place, same size, only the operand (and header
    // checksum/signature) differ, and it decodes to the new value
    let edited = text::parse(&mut src, &text.replace("0x64", "0x65")).unwrap();
    let patched = write::patch(&src, &reader, &[(0, &edited)])
        .unwrap()
        .expect("same-width edit should patch in place");
    assert_eq!(patched.len(), dex.len());
    let (_, back) = open(&patched);
    let lit = back[0].methods[0]
        .code
        .as_ref()
        .unwrap()
        .items
        .iter()
        .find_map(|it| match it {
            Item::Ins(Ins {
                op: 0x13,
                operand: Operand::Lit(v),
                ..
            }) => Some(*v),
            _ => None,
        })
        .unwrap();
    assert_eq!(lit, 0x65);

    // adding an instruction changes the layout, so it must fall back (returns None); so must
    // an edit that refers to what the file's tables lack
    let grew = text::parse(
        &mut src,
        &text.replace("    return v0", "    nop\n    return v0"),
    );
    assert!(
        write::patch(&src, &reader, &[(0, &grew.unwrap())])
            .unwrap()
            .is_none()
    );
    let new = text.replace("    const/16 v0, 0x64", "    const-string v0, \"new\"");
    let new = text::parse(&mut src, &new).unwrap();
    assert!(src.grown());
    assert!(write::patch(&src, &reader, &[(0, &new)]).unwrap().is_none());
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
    let mut src = Source::default();
    let classes = [
        text::parse(&mut src, plain).unwrap(),
        text::parse(&mut src, hidden).unwrap(),
    ];
    assert_eq!(classes[1].fields[0].hidden, Some(0x20));
    assert_eq!(classes[1].fields[1].hidden, Some(2));
    assert_eq!(
        src.str(src.field(classes[1].fields[1].id).name),
        b"whitelist"
    );
    let d = write::write(&src, &classes, *b"039").unwrap();
    let (back_src, back) = open(&d);
    let texts = |src: &Source, c: &[Class]| -> Vec<String> {
        c.iter().map(|c| text::print(src, c)).collect()
    };
    assert_eq!(texts(&back_src, &back), texts(&src, &classes));
    let flags = |c: &Class| c.methods.iter().map(|m| m.hidden).collect::<Vec<_>>();
    assert_eq!(flags(&back[1]), [Some(0x13), Some(7), Some(0x48)]);
    // a class without flags has none after the round trip, not the public list
    assert_eq!(back[0].fields[0].hidden, None);
    let printed = text::print(&back_src, &back[1]);
    assert!(printed.contains(".method public hiddenapi-0x48 unknown()V"));
    // a DEX without flags gets no hiddenapi section
    let d = write::write(&src, &classes[..1], *b"039").unwrap();
    assert_eq!(smali(&d), [plain]);
    let map = u32::from_le_bytes(d[0x34..0x38].try_into().unwrap()) as usize;
    let n = u32::from_le_bytes(d[map..map + 4].try_into().unwrap()) as usize;
    assert!((0..n).all(|k| d[map + 4 + k * 12..][..2] != [0x00, 0xf0]));
    // an id is its place in its table
    assert_eq!(
        classes[0].name.ix(),
        src.find_descriptor(b"Lcom/example/Plain;").unwrap().ix()
    );
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
