//! DEX bytes -> class model: a class is read from its source's file with every reference kept
//! as the id the file gives it. Self-contained so the query paths stay untouched.

use super::model::*;
use crate::bytecode::{self, Fmt};
use crate::dex::{sleb128, uleb128};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

type R<T> = Result<T>;

fn u32_at(d: &[u8], o: usize) -> R<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| Error::dex(format!("read past end at {o:#x}")))
}
fn u16_at(d: &[u8], o: usize) -> R<u16> {
    d.get(o..o + 2)
        .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| Error::dex(format!("read past end at {o:#x}")))
}

/// What reading a class needs of its file beyond the source's tables, read once: where the
/// class_defs are, and the method handles and call sites, which are few.
pub struct Reader {
    handles: Vec<Handle>,
    call_sites: Vec<Arc<Vec<Value>>>,
    class_defs: usize,
    class_count: usize,
    /// hiddenapi_class_data section, 0 when the file has none
    hidden: usize,
}

/// Every class of a source's file, in class_def order.
pub fn read(s: &Source) -> R<Vec<Class>> {
    let r = Reader::new(s)?;
    (0..r.class_count).map(|i| r.class(s, i)).collect()
}

impl Reader {
    pub fn new(s: &Source) -> R<Self> {
        if s.bytes().is_empty() {
            return Ok(Reader {
                handles: vec![],
                call_sites: vec![],
                class_defs: 0,
                class_count: 0,
                hidden: 0,
            });
        }
        View {
            s,
            t: &Reader {
                handles: vec![],
                call_sites: vec![],
                class_defs: 0,
                class_count: 0,
                hidden: 0,
            },
        }
        .tables()
    }

    pub fn class_count(&self) -> usize {
        self.class_count
    }

    /// The name of class `i`, in class_def order.
    pub fn class_name(&self, s: &Source, i: usize) -> R<TypeId> {
        View { s, t: self }.class_name(i)
    }

    /// Class `i` of the file, in class_def order.
    pub fn class(&self, s: &Source, i: usize) -> R<Class> {
        View { s, t: self }.class(i)
    }

    /// The file's method handles and call sites, in the order of their ids.
    pub(super) fn handles(&self) -> (&[Handle], &[Arc<Vec<Value>>]) {
        (&self.handles, &self.call_sites)
    }
}

/// A reader with the source it reads.
struct View<'a> {
    s: &'a Source,
    t: &'a Reader,
}

impl View<'_> {
    /// The reader of this file: its class_defs, method handles and call sites.
    fn tables(&self) -> R<Reader> {
        let s = self.s;
        let d = s.bytes();
        let mut r = Reader {
            handles: vec![],
            call_sites: vec![],
            class_count: u32_at(d, 0x60)? as usize,
            class_defs: u32_at(d, 0x64)? as usize,
            hidden: 0,
        };
        // call sites and method handles are only listed in the map
        let map = u32_at(d, 0x34)? as usize;
        let mut sections = (0, 0, 0, 0);
        let entries = if map == 0 {
            0
        } else {
            u32_at(d, map)? as usize
        };
        for k in 0..entries {
            let o = map + 4 + k * 12;
            match u16_at(d, o)? {
                7 => {
                    (sections.0, sections.1) =
                        (u32_at(d, o + 4)? as usize, u32_at(d, o + 8)? as usize)
                }
                8 => {
                    (sections.2, sections.3) =
                        (u32_at(d, o + 4)? as usize, u32_at(d, o + 8)? as usize)
                }
                0xf000 => r.hidden = u32_at(d, o + 8)? as usize,
                _ => {}
            }
        }
        for i in 0..sections.2 {
            let o = sections.3 + i * 8;
            let kind = u16_at(d, o)?;
            let id = u16_at(d, o + 4)? as usize;
            let member = if kind <= 3 {
                Member::Field(View { s, t: &r }.fl(id as u32)?)
            } else {
                Member::Method(View { s, t: &r }.me(id as u32)?)
            };
            r.handles.push(Handle { kind, member });
        }
        for i in 0..sections.0 {
            let mut o = u32_at(d, sections.1 + i * 4)? as usize;
            let v = View { s, t: &r }.array(&mut o)?;
            r.call_sites.push(Arc::new(v));
        }
        Ok(r)
    }

    fn class_name(&self, i: usize) -> R<TypeId> {
        self.ty(u32_at(self.s.bytes(), self.t.class_defs + i * 32)?)
    }

    /// An id read from the file, if the file's table has it.
    fn id<I: Id>(&self, table: usize, i: u32, what: &str) -> R<I> {
        match (i as usize) < self.s.file_counts()[table] {
            true => Ok(I::new(i)),
            false => Err(Error::dex(format!("bad {what} id {i}"))),
        }
    }
    fn st(&self, i: u32) -> R<StrId> {
        self.id(0, i, "string")
    }
    fn ty(&self, i: u32) -> R<TypeId> {
        self.id(1, i, "type")
    }
    fn pr(&self, i: u32) -> R<ProtoId> {
        self.id(2, i, "proto")
    }
    fn fl(&self, i: u32) -> R<FieldId> {
        self.id(3, i, "field")
    }
    fn me(&self, i: u32) -> R<MethodId> {
        self.id(4, i, "method")
    }
    fn opt_st(&self, i: u32) -> R<Option<StrId>> {
        if i == u32::MAX {
            Ok(None)
        } else {
            self.st(i).map(Some)
        }
    }
    /// uleb128p1 string index
    fn p1(&self, o: &mut usize) -> R<Option<StrId>> {
        match uleb128(self.s.bytes(), o)? {
            0 => Ok(None),
            v => self.st(v - 1).map(Some),
        }
    }

    fn array(&self, o: &mut usize) -> R<Vec<Value>> {
        let n = uleb128(self.s.bytes(), o)?;
        (0..n).map(|_| self.value(o, 0)).collect()
    }

    fn value(&self, o: &mut usize, depth: u32) -> R<Value> {
        if depth > 64 {
            return Err(Error::dex("encoded value nesting too deep"));
        }
        let d = self.s.bytes();
        let h = *d.get(*o).ok_or_else(|| Error::dex("value past end"))?;
        *o += 1;
        let (t, arg) = (h & 0x1f, (h >> 5) as usize);
        let mut raw = |n: usize| -> R<u64> {
            let b = d
                .get(*o..*o + n)
                .ok_or_else(|| Error::dex("value past end"))?;
            *o += n;
            Ok(b.iter().rev().fold(0u64, |v, &x| v << 8 | x as u64))
        };
        let sx = |v: u64, n: usize| ((v << (64 - 8 * n)) as i64) >> (64 - 8 * n);
        let idx = |v: u64| v as u32;
        Ok(match t {
            0x00 => Value::Byte(sx(raw(1)?, 1) as i8),
            0x02 => Value::Short(sx(raw(arg + 1)?, arg + 1) as i16),
            0x03 => Value::Char(raw(arg + 1)? as u16),
            0x04 => Value::Int(sx(raw(arg + 1)?, arg + 1) as i32),
            0x06 => Value::Long(sx(raw(arg + 1)?, arg + 1)),
            0x10 => Value::Float((raw(arg + 1)? << (8 * (3 - arg.min(3)))) as u32),
            0x11 => Value::Double(raw(arg + 1)? << (8 * (7 - arg.min(7)))),
            0x15 => Value::MethodType(self.pr(idx(raw(arg + 1)?))?),
            0x16 => Value::MethodHandle(
                self.t
                    .handles
                    .get(idx(raw(arg + 1)?) as usize)
                    .copied()
                    .ok_or_else(|| Error::dex("bad method handle"))?,
            ),
            0x17 => Value::Str(self.st(idx(raw(arg + 1)?))?),
            0x18 => Value::Type(self.ty(idx(raw(arg + 1)?))?),
            0x19 => Value::Field(self.fl(idx(raw(arg + 1)?))?),
            0x1a => Value::Method(self.me(idx(raw(arg + 1)?))?),
            0x1b => Value::Enum(self.fl(idx(raw(arg + 1)?))?),
            0x1c => {
                let n = uleb128(d, o)?;
                Value::Array((0..n).map(|_| self.value(o, depth + 1)).collect::<R<_>>()?)
            }
            0x1d => Value::Anno(self.anno(o, 0, depth + 1)?),
            0x1e => Value::Null,
            0x1f => Value::Bool(arg != 0),
            t => return Err(Error::dex(format!("bad encoded value type {t:#x}"))),
        })
    }

    fn anno(&self, o: &mut usize, vis: u8, depth: u32) -> R<Anno> {
        let ty = self.ty(uleb128(self.s.bytes(), o)?)?;
        let n = uleb128(self.s.bytes(), o)?;
        let mut elems = Vec::with_capacity(n.min(64) as usize);
        for _ in 0..n {
            let name = self.st(uleb128(self.s.bytes(), o)?)?;
            elems.push((name, self.value(o, depth)?));
        }
        Ok(Anno { vis, ty, elems })
    }

    fn anno_set(&self, off: usize) -> R<Vec<Anno>> {
        if off == 0 {
            return Ok(vec![]);
        }
        (0..u32_at(self.s.bytes(), off)? as usize)
            .map(|k| {
                let mut o = u32_at(self.s.bytes(), off + 4 + k * 4)? as usize;
                let vis = *self
                    .s
                    .bytes()
                    .get(o)
                    .ok_or_else(|| Error::dex("annotation past end"))?;
                o += 1;
                self.anno(&mut o, vis, 0)
            })
            .collect()
    }

    pub fn class(&self, i: usize) -> R<Class> {
        let d = self.s.bytes();
        let o = self.t.class_defs + i * 32;
        let name = self.ty(u32_at(d, o)?)?;
        let sup = match u32_at(d, o + 8)? {
            u32::MAX => None,
            t => Some(self.ty(t)?),
        };
        let il = u32_at(d, o + 12)? as usize;
        let ifaces = if il == 0 {
            vec![]
        } else {
            (0..u32_at(d, il)? as usize)
                .map(|k| self.ty(u16_at(d, il + 4 + k * 2)? as u32))
                .collect::<R<_>>()?
        };
        let mut c = Class {
            name,
            access: u32_at(d, o + 4)?,
            sup,
            ifaces,
            source: self.opt_st(u32_at(d, o + 16)?)?,
            annos: vec![],
            fields: vec![],
            methods: vec![],
        };

        // annotations by field / method index
        let dir = u32_at(d, o + 20)? as usize;
        let (mut fa, mut ma, mut pa) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        if dir != 0 {
            c.annos = self.anno_set(u32_at(d, dir)? as usize)?;
            let (nf, nm, np) = (
                u32_at(d, dir + 4)? as usize,
                u32_at(d, dir + 8)? as usize,
                u32_at(d, dir + 12)? as usize,
            );
            let mut p = dir + 16;
            for _ in 0..nf {
                fa.insert(u32_at(d, p)?, self.anno_set(u32_at(d, p + 4)? as usize)?);
                p += 8;
            }
            for _ in 0..nm {
                ma.insert(u32_at(d, p)?, self.anno_set(u32_at(d, p + 4)? as usize)?);
                p += 8;
            }
            for _ in 0..np {
                let list = u32_at(d, p + 4)? as usize;
                let sets = if list == 0 {
                    vec![]
                } else {
                    (0..u32_at(d, list)? as usize)
                        .map(|k| self.anno_set(u32_at(d, list + 4 + k * 4)? as usize))
                        .collect::<R<Vec<_>>>()?
                };
                pa.insert(u32_at(d, p)?, sets);
                p += 8;
            }
        }

        let mut statics = {
            let mut sv = u32_at(d, o + 28)? as usize;
            if sv == 0 {
                vec![]
            } else {
                self.array(&mut sv)?
            }
        }
        .into_iter();

        // hiddenapi flags: one uleb128 per member, in class_data order
        let mut hid = match self.t.hidden {
            0 => 0,
            h => match u32_at(d, h + 4 + i * 4)? as usize {
                0 => 0,
                rel => h + rel,
            },
        };
        let mut hidden = || -> R<Option<u32>> {
            if hid == 0 {
                Ok(None)
            } else {
                uleb128(d, &mut hid).map(Some)
            }
        };

        let mut off = u32_at(d, o + 24)? as usize;
        if off != 0 {
            let counts = [
                uleb128(d, &mut off)?,
                uleb128(d, &mut off)?,
                uleb128(d, &mut off)?,
                uleb128(d, &mut off)?,
            ];
            for (k, &count) in counts[..2].iter().enumerate() {
                let mut idx = 0u32;
                for _ in 0..count {
                    idx += uleb128(d, &mut off)?;
                    let access = uleb128(d, &mut off)?;
                    let init = if k == 0 { statics.next() } else { None };
                    c.fields.push(Field {
                        id: self.fl(idx)?,
                        access,
                        hidden: hidden()?,
                        init,
                        annos: fa.remove(&idx).unwrap_or_default(),
                    });
                }
            }
            for &count in &counts[2..] {
                let mut idx = 0u32;
                for _ in 0..count {
                    idx += uleb128(d, &mut off)?;
                    let access = uleb128(d, &mut off)?;
                    let code_off = uleb128(d, &mut off)? as usize;
                    let id = self.me(idx)?;
                    let nparams = self.s.proto(self.s.method(id).proto).params.len();
                    let mut method = Method {
                        id,
                        access,
                        hidden: hidden()?,
                        code: None,
                        annos: ma.remove(&idx).unwrap_or_default(),
                        param_annos: None,
                    };
                    if let Some(sets) = pa.remove(&idx) {
                        let mut sets = sets;
                        sets.resize(nparams.max(sets.len()), vec![]);
                        if sets.iter().any(|s| !s.is_empty()) {
                            method.param_annos = Some(sets);
                        }
                    }
                    if code_off != 0 {
                        method.code = Some(
                            self.code(code_off, nparams)
                                .map_err(|e| e.at(format!("method {idx}")))?,
                        );
                    }
                    c.methods.push(method);
                }
            }
        }
        Ok(c)
    }

    fn code(&self, co: usize, nparams: usize) -> R<Code> {
        let d = self.s.bytes();
        let registers = u16_at(d, co)?;
        let ntries = u16_at(d, co + 6)? as usize;
        let debug_off = u32_at(d, co + 8)? as usize;
        let units = u32_at(d, co + 12)? as usize;
        let insns = d
            .get(co + 16..co + 16 + units * 2)
            .ok_or_else(|| Error::dex("code past end"))?;
        let unit = |pc: usize| -> R<u32> {
            insns
                .get(pc * 2..pc * 2 + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]) as u32)
                .ok_or_else(|| Error::dex(format!("payload past end at {pc:#x}")))
        };
        let decoded = bytecode::decode(insns)?;

        let mut labels = BTreeSet::new();
        // offset -> item, merged in address order (payloads are skipped by decode)
        let mut at: BTreeMap<u32, Vec<Item>> = BTreeMap::new();
        let mut payload_users: BTreeMap<u32, u32> = BTreeMap::new();
        for i in &decoded {
            let (_, f, kind) = bytecode::info(i.op).unwrap();
            let mut ins = Ins {
                op: i.op,
                regs: i.regs,
                n: i.nregs,
                lit: i.lit,
                target: 0,
                r: Ref::None,
                proto: None,
            };
            match f {
                Fmt::F10t | Fmt::F20t | Fmt::F30t | Fmt::F21t | Fmt::F22t | Fmt::F31t => {
                    let t = (i.off as i64 + i.lit) as u32;
                    ins.target = t;
                    ins.lit = 0;
                    labels.insert(t);
                    if f == Fmt::F31t {
                        payload_users.insert(t, i.off);
                    }
                }
                _ => {}
            }
            ins.r = match kind {
                bytecode::Ref::None => Ref::None,
                bytecode::Ref::String => Ref::Str(self.st(i.idx)?),
                bytecode::Ref::Type => Ref::Type(self.ty(i.idx)?),
                bytecode::Ref::Field => Ref::Field(self.fl(i.idx)?),
                bytecode::Ref::Method => Ref::Method(self.me(i.idx)?),
                bytecode::Ref::Proto => Ref::Proto(self.pr(i.idx)?),
                bytecode::Ref::CallSite => Ref::CallSite(
                    self.t
                        .call_sites
                        .get(i.idx as usize)
                        .cloned()
                        .ok_or_else(|| Error::dex("bad call site id"))?,
                ),
                bytecode::Ref::MethodHandle => Ref::Handle(
                    self.t
                        .handles
                        .get(i.idx as usize)
                        .cloned()
                        .ok_or_else(|| Error::dex("bad method handle id"))?,
                ),
            };
            if matches!(f, Fmt::F45cc | Fmt::F4rcc) {
                ins.proto = Some(self.pr(i.idx2)?);
            }
            at.entry(i.off).or_default().push(Item::Ins(ins));
        }
        for (&p, &user) in &payload_users {
            let p = p as usize;
            let item = match unit(p)? {
                0x0100 => {
                    let n = unit(p + 1)? as usize;
                    let first = (unit(p + 2)? | unit(p + 3)? << 16) as i32;
                    let targets = (0..n)
                        .map(|k| {
                            Ok((user as i64
                                + (unit(p + 4 + k * 2)? | unit(p + 5 + k * 2)? << 16) as i32 as i64)
                                as u32)
                        })
                        .collect::<R<Vec<_>>>()?;
                    labels.extend(targets.iter().copied());
                    Item::Packed { first, targets }
                }
                0x0200 => {
                    let n = unit(p + 1)? as usize;
                    let key = |k: usize| -> R<i32> {
                        Ok((unit(p + 2 + k * 2)? | unit(p + 3 + k * 2)? << 16) as i32)
                    };
                    let tgt = |k: usize| -> R<u32> {
                        Ok((user as i64
                            + (unit(p + 2 + n * 2 + k * 2)? | unit(p + 3 + n * 2 + k * 2)? << 16)
                                as i32 as i64) as u32)
                    };
                    let v = (0..n)
                        .map(|k| Ok((key(k)?, tgt(k)?)))
                        .collect::<R<Vec<_>>>()?;
                    labels.extend(v.iter().map(|x| x.1));
                    Item::Sparse(v)
                }
                0x0300 => {
                    let width = unit(p + 1)? as u16;
                    let n = (unit(p + 2)? | unit(p + 3)? << 16) as usize;
                    let start = (p + 4) * 2;
                    let data = insns
                        .get(start..start + n * width as usize)
                        .ok_or_else(|| Error::dex("array data past end"))?
                        .to_vec();
                    Item::Array { width, data }
                }
                w => return Err(Error::dex(format!("bad payload {w:#x} at {p:#x}"))),
            };
            at.entry(p as u32).or_default().push(item);
            // the nop just before an aligned payload is padding; the writer re-adds it
            if p.is_multiple_of(2)
                && p > 0
                && let Some(v) = at.get_mut(&(p as u32 - 1))
                && matches!(v.as_slice(), [Item::Ins(i)] if i.op == 0)
            {
                at.remove(&(p as u32 - 1));
            }
        }

        let mut tries = Vec::with_capacity(ntries);
        for t in crate::dex::parse_tries(d, co, units)? {
            let end = t.start + t.count;
            labels.insert(t.start);
            labels.insert(end);
            let handlers = t
                .handlers
                .into_iter()
                .map(|(ty, a)| {
                    labels.insert(a);
                    Ok((ty.map(|x| self.ty(x)).transpose()?, a))
                })
                .collect::<R<Vec<_>>>()?;
            tries.push(Try {
                start: t.start,
                end,
                handlers,
            });
        }

        // debug events by address
        let mut events: BTreeMap<u32, Vec<Item>> = BTreeMap::new();
        let mut param_names = vec![];
        if debug_off != 0 {
            let mut o = debug_off;
            let mut line = uleb128(d, &mut o)?;
            let n = uleb128(d, &mut o)? as usize;
            for _ in 0..n {
                param_names.push(self.p1(&mut o)?);
            }
            let mut addr = 0u32;
            loop {
                let op = *d.get(o).ok_or_else(|| Error::dex("debug info past end"))?;
                o += 1;
                let ev = match op {
                    0 => break,
                    1 => {
                        addr += uleb128(d, &mut o)?;
                        continue;
                    }
                    2 => {
                        line = line.wrapping_add(sleb128(d, &mut o)? as u32);
                        continue;
                    }
                    3 | 4 => {
                        let reg = uleb128(d, &mut o)? as u16;
                        let name = self.p1(&mut o)?;
                        // the type is an index into type_ids, not string_ids
                        let ty = match uleb128(d, &mut o)? {
                            0 => None,
                            v => Some(self.ty(v - 1)?),
                        };
                        let sig = if op == 4 { self.p1(&mut o)? } else { None };
                        Item::Local { reg, name, ty, sig }
                    }
                    5 => Item::EndLocal(uleb128(d, &mut o)? as u16),
                    6 => Item::Restart(uleb128(d, &mut o)? as u16),
                    7 => Item::Prologue,
                    8 => Item::Epilogue,
                    9 => Item::SetFile(self.p1(&mut o)?),
                    _ => {
                        let adj = (op - 0x0a) as i32;
                        line = line.wrapping_add((adj % 15 - 4) as u32);
                        addr += (adj / 15) as u32;
                        Item::Line(line)
                    }
                };
                events.entry(addr).or_default().push(ev);
            }
        }
        // debug info with no events and no names says nothing; the text form cannot carry it
        param_names.resize(nparams, None);
        let debug =
            debug_off != 0 && (!events.is_empty() || param_names.iter().any(|n| n.is_some()));

        // shrinkers leave line entries inside an instruction; Smali can only place them between
        let starts: BTreeSet<u32> = at.keys().copied().chain([units as u32]).collect();
        if events.keys().any(|a| !starts.contains(a)) {
            let mut snapped: BTreeMap<u32, Vec<Item>> = BTreeMap::new();
            for (a, ev) in std::mem::take(&mut events) {
                let to = starts.range(a..).next().copied().unwrap_or(a);
                snapped.entry(to).or_default().extend(ev);
            }
            events = snapped;
        }

        let mut items = Vec::with_capacity(decoded.len() + labels.len());
        let mut keys: BTreeSet<u32> = at.keys().copied().collect();
        keys.extend(labels.iter().copied());
        keys.extend(events.keys().copied());
        for k in keys {
            if labels.contains(&k) {
                items.push(Item::Label(k));
            }
            if let Some(ev) = events.remove(&k) {
                items.extend(ev);
            }
            if let Some(v) = at.remove(&k) {
                items.extend(v);
            }
        }
        Ok(Code {
            registers,
            items,
            tries,
            debug,
            param_names: if debug {
                param_names
            } else {
                vec![None; nparams]
            },
        })
    }
}
