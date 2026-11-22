//! DEX files open for editing. A [`Document`] is one file: its bytes and id tables, read once,
//! with the classes an edit changed kept beside them. A class is read from the bytes when it is
//! asked for, and writing puts back only what changed. [`Classes`] is the DEX files of an APK.

use super::model::{Class, Item, Operand, Ref, S};
use super::{read, version, write};
use crate::bytecode;
use crate::{Error, Result};
use std::collections::{BTreeMap, HashMap};

/// One DEX file open for editing.
pub struct Document {
    reader: read::Reader<Vec<u8>>,
    /// class_def index by descriptor
    index: HashMap<S, usize>,
    /// The classes that differ from the file's, as edited, by descriptor: changed ones and
    /// new ones.
    edited: BTreeMap<S, Class>,
}

impl Document {
    pub fn new(dex: Vec<u8>) -> Result<Self> {
        let reader = read::Reader::owned(dex)?;
        let index = (0..reader.class_count())
            .map(|i| Ok((reader.class_name(i)?, i)))
            .collect::<Result<_>>()?;
        Ok(Self {
            reader,
            index,
            edited: BTreeMap::new(),
        })
    }

    /// Whether the file, as edited, defines the class `name`, a descriptor.
    pub fn defines(&self, name: &[u8]) -> bool {
        self.index.contains_key(name) || self.edited.contains_key(name)
    }

    /// The class `name`, a descriptor, as edited.
    pub fn class(&self, name: &[u8]) -> Result<Option<Class>> {
        match (self.edited.get(name), self.index.get(name)) {
            (Some(c), _) => Ok(Some(c.clone())),
            (None, Some(&i)) => self.reader.class(i).map(Some),
            (None, None) => Ok(None),
        }
    }

    /// Give the file `class`, in place of the one of its name or as a new one. A class equal to
    /// the file's own is no edit.
    pub fn set(&mut self, class: Class) -> Result<()> {
        let same = match self.index.get(&class.name) {
            Some(&i) => self.reader.class(i)? == class,
            None => false,
        };
        match same {
            true => self.edited.remove(&class.name),
            false => self.edited.insert(class.name.clone(), class),
        };
        Ok(())
    }

    /// Whether an edit changed the file.
    pub fn changed(&self) -> bool {
        !self.edited.is_empty()
    }

    /// The classes, as edited, with an instruction that has the operand `o`. The file's own
    /// classes are not modelled for it: a reference is looked up in the id tables, where one
    /// that is absent is in no class, and the bytecode is scanned for its index or the literal.
    pub fn find(&self, o: &Operand) -> Result<Vec<S>> {
        let (strings, types, protos, fields, methods, ..) = self.reader.pool_tables();
        let at = |i: Option<usize>, kind| i.map(|i| (kind, i as u32));
        let idx = match o {
            Operand::Lit(_) => None,
            Operand::Ref(Ref::Str(x)) => {
                at(strings.iter().position(|y| y == x), bytecode::Ref::String)
            }
            Operand::Ref(Ref::Type(x)) => {
                at(types.iter().position(|y| y == x), bytecode::Ref::Type)
            }
            Operand::Ref(Ref::Field(x)) => {
                at(fields.iter().position(|y| y == x), bytecode::Ref::Field)
            }
            Operand::Ref(Ref::Method(x)) => {
                at(methods.iter().position(|y| y == x), bytecode::Ref::Method)
            }
            Operand::Ref(Ref::Proto(x)) => {
                at(protos.iter().position(|y| y == x), bytecode::Ref::Proto)
            }
            Operand::Ref(_) => None,
        };
        let mut found = Vec::new();
        if idx.is_some() || matches!(o, Operand::Lit(_)) {
            let dex = crate::dex::Dex::parse(self.reader.bytes())?;
            for i in 0..self.reader.class_count() {
                let name = self.reader.class_name(i)?;
                if self.edited.contains_key(&name) {
                    continue;
                }
                let mut has = false;
                for m in dex.class_data(i)?.methods {
                    let Some(code) = m.code else { continue };
                    has |= bytecode::decode(code.insns)?.iter().any(|ins| {
                        let Some((_, f, kind)) = bytecode::info(ins.op) else {
                            return false;
                        };
                        match (o, idx) {
                            (Operand::Lit(v), _) => f.has_literal() && ins.lit == *v,
                            (_, Some(want)) => (kind, ins.idx) == want,
                            _ => false,
                        }
                    });
                    if has {
                        break;
                    }
                }
                if has {
                    found.push(name);
                }
            }
        }
        for (name, class) in &self.edited {
            let code = class.methods.iter().filter_map(|m| m.code.as_ref());
            if code
                .flat_map(|code| &code.items)
                .any(|item| matches!(item, Item::Ins(ins) if ins.has(o)))
            {
                found.push(name.clone());
            }
        }
        Ok(found)
    }

    /// The file as edited. Edits that only change the instructions of existing methods are
    /// patched into its bytes; anything else writes the file again.
    pub fn write(&self) -> Result<Vec<u8>> {
        if self.edited.is_empty() {
            return Ok(self.reader.bytes().to_vec());
        }
        let in_place: Option<Vec<(usize, &Class)>> = self
            .edited
            .values()
            .map(|c| self.index.get(&c.name).map(|&i| (i, c)))
            .collect();
        if let Some(edits) = in_place
            && let Some(dex) = write::patch_read(&self.reader, &edits)?
        {
            return Ok(dex);
        }
        let mut all = Vec::with_capacity(self.reader.class_count());
        for i in 0..self.reader.class_count() {
            all.push(match self.edited.get(&self.reader.class_name(i)?) {
                Some(c) => c.clone(),
                None => self.reader.class(i)?,
            });
        }
        let new = self.edited.values();
        all.extend(new.filter(|c| !self.index.contains_key(&c.name)).cloned());
        write::write(&all, version(self.reader.bytes()))
    }
}

/// The DEX files of an APK open for editing, in load order. A file is opened when a class of it
/// is first asked for.
pub struct Classes {
    dex: Vec<(String, Slot)>,
}

enum Slot {
    Raw(Vec<u8>),
    Open(Box<Document>),
}

impl Classes {
    /// The files `dex`, by entry name, in load order (classes.dex, classes2.dex, ...), as
    /// [`crate::apk::Apk::dex`] has them.
    pub fn new(dex: Vec<(String, Vec<u8>)>) -> Self {
        Self {
            dex: dex.into_iter().map(|(n, d)| (n, Slot::Raw(d))).collect(),
        }
    }

    fn open(&mut self, k: usize) -> Result<&mut Document> {
        let (name, slot) = &mut self.dex[k];
        if let Slot::Raw(bytes) = slot {
            let doc = Document::new(std::mem::take(bytes)).map_err(|e| e.at(name))?;
            *slot = Slot::Open(Box::new(doc));
        }
        match slot {
            Slot::Open(doc) => Ok(doc),
            Slot::Raw(_) => unreachable!("opened above"),
        }
    }

    /// The file that defines the class `desc`: the first in load order, as Android resolves it.
    fn defining(&mut self, desc: &str) -> Result<Option<&mut Document>> {
        for k in 0..self.dex.len() {
            let defines = match &self.dex[k].1 {
                Slot::Raw(bytes) => crate::dex::Dex::parse(bytes)
                    .map_err(|e| e.at(&self.dex[k].0))?
                    .find_class(desc)
                    .is_some(),
                Slot::Open(doc) => doc.defines(desc.as_bytes()),
            };
            if defines {
                return self.open(k).map(Some);
            }
        }
        Ok(None)
    }

    /// The class `name` (dotted or `L..;`), as edited.
    pub fn class(&mut self, name: &str) -> Result<Class> {
        let desc = crate::dex::descriptor(name);
        let class = match self.defining(&desc)? {
            Some(doc) => doc.class(desc.as_bytes())?,
            None => None,
        };
        class.ok_or_else(|| Error::ClassNotFound(name.to_owned()))
    }

    /// Give the APK `class`: in the file that defines a class of its name, in place of it, or
    /// as a new class in classes.dex.
    pub fn set(&mut self, class: Class) -> Result<()> {
        let desc = String::from_utf8_lossy(&class.name).into_owned();
        if let Some(doc) = self.defining(&desc)? {
            return doc.set(class);
        }
        if self.dex.is_empty() {
            return Err(Error::EntryNotFound("classes.dex".into()));
        }
        self.open(0)?.set(class)
    }

    /// The classes, as edited, with an instruction that has the operand `o`, by descriptor.
    pub fn find(&mut self, o: &Operand) -> Result<Vec<String>> {
        let mut found = Vec::new();
        for k in 0..self.dex.len() {
            let names = self.open(k)?.find(o)?;
            found.extend(
                names
                    .iter()
                    .map(|n| String::from_utf8_lossy(n).into_owned()),
            );
        }
        Ok(found)
    }

    /// Put the files the edits changed, as edited, into `edits`; their entry names.
    pub fn write(&self, edits: &mut crate::rebuild::Edits) -> Result<Vec<String>> {
        let mut changed = Vec::new();
        for (name, slot) in &self.dex {
            if let Slot::Open(doc) = slot
                && doc.changed()
            {
                edits.insert(name.clone(), Some(doc.write().map_err(|e| e.at(name))?));
                changed.push(name.clone());
            }
        }
        Ok(changed)
    }
}
