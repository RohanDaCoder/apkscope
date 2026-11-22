use super::*;
use crate::smali::{text, write};

fn dex() -> Vec<u8> {
    let class = text::parse(
        ".class public La/A;\n.super Ljava/lang/Object;\n.field public n:I\n\
         .method public static f(IJ)Ljava/lang/String;\n    .registers 4\n\
         \x20   const-string v0, \"caf\u{e9}\"\n    const-string v0, \"zebra\"\n\
         \x20   return-object v0\n.end method\n",
    )
    .unwrap();
    write::write(&[class], *b"035").unwrap()
}

#[test]
fn reads_the_files_tables_in_place_and_finds_each_entry() {
    let s = Source::new(dex()).unwrap();
    let [strs, types, protos, fields, methods] = s.file_counts();
    assert!(!s.grown());
    for i in 0..strs as u32 {
        assert_eq!(s.find_str(s.str(StrId(i))), Some(StrId(i)));
    }
    for i in 0..types as u32 {
        assert_eq!(s.find_type(s.type_str(TypeId(i))), Some(TypeId(i)));
    }
    for i in 0..protos as u32 {
        let p = s.proto(ProtoId(i));
        assert_eq!(s.find_proto(p.ret, &p.params), Some(ProtoId(i)));
    }
    for i in 0..fields as u32 {
        assert_eq!(s.find_field(s.field(FieldId(i))), Some(FieldId(i)));
    }
    for i in 0..methods as u32 {
        assert_eq!(s.find_method(s.method(MethodId(i))), Some(MethodId(i)));
    }
    let f = s.method(MethodId(0));
    assert_eq!(s.descriptor(f.class), b"La/A;");
    assert_eq!(s.str(f.name), b"f");
    let p = s.proto(f.proto);
    assert_eq!(s.str(p.shorty), b"LIJ");
    assert_eq!(s.descriptor(p.ret), b"Ljava/lang/String;");
    assert!(s.find_str(b"absent").is_none());
    assert!(s.find_descriptor(b"Lb/B;").is_none());
}

#[test]
fn adds_what_is_new_after_the_files_own() {
    let mut s = Source::new(dex()).unwrap();
    let before = s.counts();
    // what the file has keeps its id, and adds nothing
    let own = s.find_str(b"zebra").unwrap();
    assert_eq!(s.intern_str(b"zebra"), own);
    let string = s.find_descriptor(b"Ljava/lang/String;").unwrap();
    assert_eq!(s.intern_descriptor(b"Ljava/lang/String;"), string);
    assert_eq!(s.counts(), before);

    // a new entry gets the next id of its table, once
    let new = s.intern_str(b"new");
    assert_eq!(new.ix(), before[0]);
    assert_eq!(s.intern_str(b"new"), new);
    assert_eq!(s.str(new), b"new");
    let b = s.intern_descriptor(b"Lb/B;");
    let int = s.find_descriptor(b"I").unwrap();
    let proto = s.intern_proto(b, &[int, string]);
    assert_eq!(s.str(s.proto(proto).shorty), b"LIL");
    assert_eq!(s.intern_proto(b, &[int, string]), proto);
    let method = MethodRef {
        class: b,
        name: new,
        proto,
    };
    let id = s.intern_method(method);
    assert_eq!(s.find_method(method), Some(id));
    assert_eq!(s.method(id), method);
    let field = FieldRef {
        class: b,
        name: new,
        ty: int,
    };
    assert_eq!(s.intern_field(field), s.intern_field(field));
    assert!(s.grown());

    assert!(Source::new(b"dex\n035".to_vec()).is_err());
    // a source of no file starts empty
    let mut empty = Source::default();
    assert_eq!(empty.intern_descriptor(b"I"), TypeId(0));
}
