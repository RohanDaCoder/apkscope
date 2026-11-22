//! What a class model refers to. A DEX file is five sorted tables (strings, types, prototypes,
//! fields, methods) that everything else points into by index, and a [`Source`] is those tables
//! as the file holds them, read in place from its bytes, with the entries an edit added after
//! them. An id below a table's count in the file is the file's own entry; one from there on is
//! an added one. Nothing of the file is copied, and nothing is looked up by content to write it
//! again: the file's own entries are in order as they are.
//!
//! The tables are checked once, when the file is opened, so reading an entry cannot fail after
//! that. An id is only meaningful for the source it came from.

use crate::dex::uleb128;
use crate::{Error, Result};
use std::cmp::Ordering;
use std::collections::HashMap;

type R<T> = Result<T>;

/// An id into one of a source's tables.
pub trait Id: Copy {
    fn new(i: u32) -> Self;
    fn ix(&self) -> usize;
}

macro_rules! id {
    ($($(#[$doc:meta])* $name:ident),*) => {$(
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
        pub struct $name(pub u32);
        impl Id for $name {
            fn new(i: u32) -> Self {
                Self(i)
            }
            fn ix(&self) -> usize {
                self.0 as usize
            }
        }
    )*};
}
id!(
    /// A string: MUTF-8 bytes, as the DEX stores them.
    StrId,
    /// A type, by its descriptor (`Ljava/lang/String;`, `[I`).
    TypeId,
    /// A method prototype.
    ProtoId,
    /// A field of a class.
    FieldId,
    /// A method of a class.
    MethodId
);

/// A prototype: its shorty, return type and parameters.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Proto {
    pub shorty: StrId,
    pub ret: TypeId,
    pub params: Vec<TypeId>,
}

/// A field: the class it is of, its name and its type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FieldRef {
    pub class: TypeId,
    pub name: StrId,
    pub ty: TypeId,
}

/// A method: the class it is of, its name and its prototype.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MethodRef {
    pub class: TypeId,
    pub name: StrId,
    pub proto: ProtoId,
}

fn u16_at(d: &[u8], o: usize) -> R<u16> {
    d.get(o..o + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| Error::dex(format!("read past end at {o:#x}")))
}
fn u32_at(d: &[u8], o: usize) -> R<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| Error::dex(format!("read past end at {o:#x}")))
}

/// MUTF-8 bytes to UTF-16 units.
fn utf16(b: &[u8]) -> impl Iterator<Item = u16> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        let c = *b.get(i)? as u16;
        let nx = |k: usize| b.get(i + k).map_or(0, |&x| x as u16 & 0x3f);
        let (u, n) = match c {
            0..=0x7f => (c, 1),
            0xc0..=0xdf => ((c & 0x1f) << 6 | nx(1), 2),
            0xe0..=0xef => ((c & 0x0f) << 12 | nx(1) << 6 | nx(2), 3),
            _ => (0xfffd, 1),
        };
        i += n;
        Some(u)
    })
}

/// The order of the string table: by UTF-16 code unit.
pub fn cmp_str(a: &[u8], b: &[u8]) -> Ordering {
    match a.is_ascii() && b.is_ascii() {
        true => a.cmp(b),
        false => utf16(a).cmp(utf16(b)),
    }
}

/// The tables an edit added entries to, each with where an entry is by its content.
#[derive(Default)]
struct Added {
    strs: Vec<Box<[u8]>>,
    types: Vec<StrId>,
    protos: Vec<Proto>,
    fields: Vec<FieldRef>,
    methods: Vec<MethodRef>,
    str_at: HashMap<Box<[u8]>, u32>,
    type_at: HashMap<StrId, u32>,
    proto_at: HashMap<(TypeId, Vec<TypeId>), u32>,
    field_at: HashMap<FieldRef, u32>,
    method_at: HashMap<MethodRef, u32>,
}

#[derive(Default)]
pub struct Source {
    d: Vec<u8>,
    /// (count, offset) of the string, type, proto, field and method id tables in the file
    tables: [(usize, usize); 5],
    added: Added,
}

/// An entry of a file's table, read with every bound checked.
impl Source {
    fn file_str(&self, i: usize) -> R<&[u8]> {
        let mut o = u32_at(&self.d, self.tables[0].1 + i * 4)? as usize;
        // the length in UTF-16 units comes before the bytes
        uleb128(&self.d, &mut o)?;
        let rest = &self.d[o..];
        let end = rest.iter().position(|&b| b == 0);
        Ok(&rest[..end.ok_or_else(|| Error::dex("unterminated string"))?])
    }
    fn file_type(&self, i: usize) -> R<StrId> {
        let s = u32_at(&self.d, self.tables[1].1 + i * 4)?;
        self.in_table(0, s, "string").map(|()| StrId(s))
    }
    fn file_proto(&self, i: usize) -> R<Proto> {
        let o = self.tables[2].1 + i * 12;
        let (shorty, ret) = (u32_at(&self.d, o)?, u32_at(&self.d, o + 4)?);
        self.in_table(0, shorty, "string")?;
        self.in_table(1, ret, "type")?;
        let list = u32_at(&self.d, o + 8)? as usize;
        let mut params = vec![];
        if list != 0 {
            for k in 0..u32_at(&self.d, list)? as usize {
                let t = u16_at(&self.d, list + 4 + k * 2)? as u32;
                self.in_table(1, t, "type")?;
                params.push(TypeId(t));
            }
        }
        Ok(Proto {
            shorty: StrId(shorty),
            ret: TypeId(ret),
            params,
        })
    }
    fn file_field(&self, i: usize) -> R<FieldRef> {
        let o = self.tables[3].1 + i * 8;
        let (class, ty) = (u16_at(&self.d, o)? as u32, u16_at(&self.d, o + 2)? as u32);
        let name = u32_at(&self.d, o + 4)?;
        self.in_table(1, class, "type")?;
        self.in_table(1, ty, "type")?;
        self.in_table(0, name, "string")?;
        Ok(FieldRef {
            class: TypeId(class),
            name: StrId(name),
            ty: TypeId(ty),
        })
    }
    fn file_method(&self, i: usize) -> R<MethodRef> {
        let o = self.tables[4].1 + i * 8;
        let (class, proto) = (u16_at(&self.d, o)? as u32, u16_at(&self.d, o + 2)? as u32);
        let name = u32_at(&self.d, o + 4)?;
        self.in_table(1, class, "type")?;
        self.in_table(2, proto, "proto")?;
        self.in_table(0, name, "string")?;
        Ok(MethodRef {
            class: TypeId(class),
            name: StrId(name),
            proto: ProtoId(proto),
        })
    }
    fn in_table(&self, table: usize, i: u32, what: &str) -> R<()> {
        match (i as usize) < self.tables[table].0 {
            true => Ok(()),
            false => Err(Error::dex(format!("bad {what} id {i}"))),
        }
    }
}

/// An id that is not of this source is the caller's mistake, as an index past a slice's end is.
const CHECKED: &str = "checked when the file was opened";

impl Source {
    /// The tables of the DEX file `d`, every entry of them checked.
    pub fn new(d: Vec<u8>) -> R<Self> {
        if d.len() < 0x70 || &d[..4] != b"dex\n" {
            return Err(Error::dex("not a DEX file"));
        }
        let mut s = Self {
            d,
            ..Self::default()
        };
        for k in 0..5 {
            let o = 0x38 + k * 8;
            s.tables[k] = (u32_at(&s.d, o)? as usize, u32_at(&s.d, o + 4)? as usize);
        }
        let [strs, types, protos, fields, methods] = s.tables.map(|t| t.0);
        (0..strs).try_for_each(|i| s.file_str(i).map(drop))?;
        (0..types).try_for_each(|i| s.file_type(i).map(drop))?;
        (0..protos).try_for_each(|i| s.file_proto(i).map(drop))?;
        (0..fields).try_for_each(|i| s.file_field(i).map(drop))?;
        (0..methods).try_for_each(|i| s.file_method(i).map(drop))?;
        Ok(s)
    }

    /// The file's bytes; empty for a source that is of no file.
    pub fn bytes(&self) -> &[u8] {
        &self.d
    }

    /// How many strings, types, protos, fields and methods the file itself has.
    pub fn file_counts(&self) -> [usize; 5] {
        self.tables.map(|t| t.0)
    }

    /// How many there are with the added ones.
    pub fn counts(&self) -> [usize; 5] {
        let a = &self.added;
        let added = [
            a.strs.len(),
            a.types.len(),
            a.protos.len(),
            a.fields.len(),
            a.methods.len(),
        ];
        std::array::from_fn(|k| self.tables[k].0 + added[k])
    }

    /// Whether any entry was added to the file's own.
    pub fn grown(&self) -> bool {
        self.counts() != self.file_counts()
    }

    /// The bytes of a string, MUTF-8.
    pub fn str(&self, id: StrId) -> &[u8] {
        match id.ix().checked_sub(self.tables[0].0) {
            None => self.file_str(id.ix()).expect(CHECKED),
            Some(at) => &self.added.strs[at],
        }
    }

    /// The descriptor of a type, as a string of this source.
    pub fn type_str(&self, id: TypeId) -> StrId {
        match id.ix().checked_sub(self.tables[1].0) {
            None => self.file_type(id.ix()).expect(CHECKED),
            Some(at) => self.added.types[at],
        }
    }

    /// The descriptor of a type.
    pub fn descriptor(&self, id: TypeId) -> &[u8] {
        self.str(self.type_str(id))
    }

    pub fn proto(&self, id: ProtoId) -> Proto {
        match id.ix().checked_sub(self.tables[2].0) {
            None => self.file_proto(id.ix()).expect(CHECKED),
            Some(at) => self.added.protos[at].clone(),
        }
    }

    pub fn field(&self, id: FieldId) -> FieldRef {
        match id.ix().checked_sub(self.tables[3].0) {
            None => self.file_field(id.ix()).expect(CHECKED),
            Some(at) => self.added.fields[at],
        }
    }

    pub fn method(&self, id: MethodId) -> MethodRef {
        match id.ix().checked_sub(self.tables[4].0) {
            None => self.file_method(id.ix()).expect(CHECKED),
            Some(at) => self.added.methods[at],
        }
    }

    /// Where in the file's table `table`, which is sorted, the entry is that `cmp` finds equal.
    fn search(&self, table: usize, cmp: impl Fn(usize) -> Ordering) -> Option<u32> {
        let (mut lo, mut hi) = (0, self.tables[table].0);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match cmp(mid) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return Some(mid as u32),
            }
        }
        None
    }

    /// The id of the string `s`, if the source has it.
    pub fn find_str(&self, s: &[u8]) -> Option<StrId> {
        let file = self.search(0, |i| cmp_str(self.file_str(i).expect(CHECKED), s));
        let added = || Some(self.tables[0].0 as u32 + self.added.str_at.get(s)?);
        file.or_else(added).map(StrId)
    }

    /// The id of the type whose descriptor is the string `s`, if the source has it. The type
    /// table is in the order of its strings.
    pub fn find_type(&self, s: StrId) -> Option<TypeId> {
        let file = self.search(1, |i| self.file_type(i).expect(CHECKED).cmp(&s));
        let added = || Some(self.tables[1].0 as u32 + self.added.type_at.get(&s)?);
        file.or_else(added).map(TypeId)
    }

    /// The id of the type with the descriptor `d`, if the source has it.
    pub fn find_descriptor(&self, d: &[u8]) -> Option<TypeId> {
        self.find_type(self.find_str(d)?)
    }

    /// The prototype table is in the order of its return type, then its parameters.
    pub fn find_proto(&self, ret: TypeId, params: &[TypeId]) -> Option<ProtoId> {
        let file = self.search(2, |i| {
            let p = self.file_proto(i).expect(CHECKED);
            (p.ret, &p.params[..]).cmp(&(ret, params))
        });
        let added = || {
            let at = self.added.proto_at.get(&(ret, params.to_vec()))?;
            Some(self.tables[2].0 as u32 + at)
        };
        file.or_else(added).map(ProtoId)
    }

    /// The field table is in the order of its class, then its name, then its type.
    pub fn find_field(&self, f: FieldRef) -> Option<FieldId> {
        let key = |f: FieldRef| (f.class, f.name, f.ty);
        let file = self.search(3, |i| key(self.file_field(i).expect(CHECKED)).cmp(&key(f)));
        let added = || Some(self.tables[3].0 as u32 + self.added.field_at.get(&f)?);
        file.or_else(added).map(FieldId)
    }

    /// The method table is in the order of its class, then its name, then its prototype.
    pub fn find_method(&self, m: MethodRef) -> Option<MethodId> {
        let key = |m: MethodRef| (m.class, m.name, m.proto);
        let file = self.search(4, |i| key(self.file_method(i).expect(CHECKED)).cmp(&key(m)));
        let added = || Some(self.tables[4].0 as u32 + self.added.method_at.get(&m)?);
        file.or_else(added).map(MethodId)
    }

    /// The id of the string `s`, added to the source if it is new to it.
    pub fn intern_str(&mut self, s: &[u8]) -> StrId {
        self.find_str(s).unwrap_or_else(|| {
            let at = self.added.strs.len() as u32;
            self.added.strs.push(s.into());
            self.added.str_at.insert(s.into(), at);
            StrId(self.tables[0].0 as u32 + at)
        })
    }

    pub fn intern_type(&mut self, s: StrId) -> TypeId {
        self.find_type(s).unwrap_or_else(|| {
            let at = self.added.types.len() as u32;
            self.added.types.push(s);
            self.added.type_at.insert(s, at);
            TypeId(self.tables[1].0 as u32 + at)
        })
    }

    /// The id of the type with the descriptor `d`.
    pub fn intern_descriptor(&mut self, d: &[u8]) -> TypeId {
        let s = self.intern_str(d);
        self.intern_type(s)
    }

    /// The id of the prototype, with its shorty: a letter for the return type and for each
    /// parameter, `L` for a class or an array.
    pub fn intern_proto(&mut self, ret: TypeId, params: &[TypeId]) -> ProtoId {
        if let Some(id) = self.find_proto(ret, params) {
            return id;
        }
        let letter = |t: &TypeId| match self.descriptor(*t).first() {
            Some(b'[') | None => b'L',
            Some(c) => *c,
        };
        let shorty: Vec<u8> = [ret].iter().chain(params).map(letter).collect();
        let proto = Proto {
            shorty: self.intern_str(&shorty),
            ret,
            params: params.to_vec(),
        };
        let at = self.added.protos.len() as u32;
        self.added.protos.push(proto);
        self.added.proto_at.insert((ret, params.to_vec()), at);
        ProtoId(self.tables[2].0 as u32 + at)
    }

    pub fn intern_field(&mut self, f: FieldRef) -> FieldId {
        self.find_field(f).unwrap_or_else(|| {
            let at = self.added.fields.len() as u32;
            self.added.fields.push(f);
            self.added.field_at.insert(f, at);
            FieldId(self.tables[3].0 as u32 + at)
        })
    }

    pub fn intern_method(&mut self, m: MethodRef) -> MethodId {
        self.find_method(m).unwrap_or_else(|| {
            let at = self.added.methods.len() as u32;
            self.added.methods.push(m);
            self.added.method_at.insert(m, at);
            MethodId(self.tables[4].0 as u32 + at)
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/apkscope-core/smali_source.rs"]
mod tests;
