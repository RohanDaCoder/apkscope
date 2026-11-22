//! DEX files open for editing. A [`Document`] is one file: its [`Source`], with the classes an
//! edit changed kept beside it as models. A class is read from the file when it is asked for,
//! and writing puts back only what changed. [`Classes`] is the DEX files of an APK.

use super::model::{Class, Item, Operand, Ref, Source, TypeId};
use super::{read, text, version, write};
use crate::bytecode;
use crate::{Error, Result};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

/// One DEX file open for editing. Its classes refer to its source: a class of one document
/// means nothing in another.
pub struct Document {
    src: Source,
    reader: read::Reader,
    /// class_def index by name
    index: HashMap<TypeId, usize>,
    /// The classes that differ from the file's, as edited, by name: changed ones and new ones.
    edited: BTreeMap<TypeId, Class>,
}

impl Document {
    pub fn new(dex: Vec<u8>) -> Result<Self> {
        let src = Source::new(dex)?;
        let reader = read::Reader::new(&src)?;
        let index = (0..reader.class_count())
            .map(|i| Ok((reader.class_name(&src, i)?, i)))
            .collect::<Result<_>>()?;
        Ok(Self {
            src,
            reader,
            index,
            edited: BTreeMap::new(),
        })
    }

    /// What the document's classes refer to.
    pub fn source(&self) -> &Source {
        &self.src
    }

    /// The source, to add what an edit refers to that the file lacks.
    pub fn source_mut(&mut self) -> &mut Source {
        &mut self.src
    }

    /// Whether the file, as edited, defines the class `name`.
    pub fn defines(&self, name: TypeId) -> bool {
        self.index.contains_key(&name) || self.edited.contains_key(&name)
    }

    /// The class `name`, as edited.
    pub fn class(&self, name: TypeId) -> Result<Option<Class>> {
        match (self.edited.get(&name), self.index.get(&name)) {
            (Some(c), _) => Ok(Some(c.clone())),
            (None, Some(&i)) => self.reader.class(&self.src, i).map(Some),
            (None, None) => Ok(None),
        }
    }

    /// Give the file `class`, in place of the one of its name or as a new one. A class equal to
    /// the file's own is no edit.
    pub fn set(&mut self, class: Class) -> Result<()> {
        let same = match self.index.get(&class.name) {
            Some(&i) => self.reader.class(&self.src, i)? == class,
            None => false,
        };
        match same {
            true => self.edited.remove(&class.name),
            false => self.edited.insert(class.name, class),
        };
        Ok(())
    }

    /// Whether an edit changed the file.
    pub fn changed(&self) -> bool {
        !self.edited.is_empty()
    }

    /// The classes, as edited, with an instruction that has the operand `o`. The file's own
    /// classes are not modelled for it: the bytecode is scanned for the literal, or for the id
    /// of what the operand refers to, which is the file's own index of it. A reference the
    /// file's tables lack is in none of the file's classes.
    pub fn find(&self, o: &Operand) -> Result<Vec<TypeId>> {
        let file = self.src.file_counts();
        let own = |table: usize, id: u32, kind| ((id as usize) < file[table]).then_some((kind, id));
        let idx = match o {
            Operand::Lit(_) => None,
            Operand::Ref(Ref::Str(x)) => own(0, x.0, bytecode::Ref::String),
            Operand::Ref(Ref::Type(x)) => own(1, x.0, bytecode::Ref::Type),
            Operand::Ref(Ref::Proto(x)) => own(2, x.0, bytecode::Ref::Proto),
            Operand::Ref(Ref::Field(x)) => own(3, x.0, bytecode::Ref::Field),
            Operand::Ref(Ref::Method(x)) => own(4, x.0, bytecode::Ref::Method),
            Operand::Ref(_) => None,
        };
        let mut found = Vec::new();
        if idx.is_some() || matches!(o, Operand::Lit(_)) {
            let dex = crate::dex::Dex::parse(self.src.bytes())?;
            for i in 0..self.reader.class_count() {
                let name = self.reader.class_name(&self.src, i)?;
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
                found.push(*name);
            }
        }
        Ok(found)
    }

    /// The file as edited. Edits that only change the instructions of existing methods are
    /// patched into its bytes; anything else writes the file again, with no more than one of
    /// its unedited classes modelled at a time.
    pub fn write(&self) -> Result<Vec<u8>> {
        if self.edited.is_empty() {
            return Ok(self.src.bytes().to_vec());
        }
        let in_place: Option<Vec<(usize, &Class)>> = self
            .edited
            .values()
            .map(|c| self.index.get(&c.name).map(|&i| (i, c)))
            .collect();
        if let Some(edits) = in_place
            && let Some(dex) = write::patch(&self.src, &self.reader, &edits)?
        {
            return Ok(dex);
        }
        // the file's classes in their order, each read as it is written, then the new ones
        let count = self.reader.class_count();
        let new: Vec<&Class> = (self.edited.values())
            .filter(|c| !self.index.contains_key(&c.name))
            .collect();
        let class = |i: usize| -> Result<Cow<'_, Class>> {
            if i >= count {
                return Ok(Cow::Borrowed(new[i - count]));
            }
            Ok(
                match self.edited.get(&self.reader.class_name(&self.src, i)?) {
                    Some(c) => Cow::Borrowed(c),
                    None => Cow::Owned(self.reader.class(&self.src, i)?),
                },
            )
        };
        let version = match self.src.bytes().is_empty() {
            true => *b"035",
            false => version(self.src.bytes()),
        };
        write::write_each(&self.src, count + new.len(), &class, version)
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

    /// The file that defines the class `name` (dotted or `L..;`), with the class's name in it:
    /// the first in load order, as Android resolves it.
    pub fn defining(&mut self, name: &str) -> Result<Option<(&mut Document, TypeId)>> {
        let desc = crate::dex::descriptor(name);
        for k in 0..self.dex.len() {
            let id = |doc: &Document| {
                let id = doc.src.find_descriptor(desc.as_bytes())?;
                doc.defines(id).then_some(id)
            };
            let defines = match &self.dex[k].1 {
                Slot::Raw(bytes) => crate::dex::Dex::parse(bytes)
                    .map_err(|e| e.at(&self.dex[k].0))?
                    .find_class(&desc)
                    .is_some(),
                Slot::Open(doc) => id(doc).is_some(),
            };
            if defines {
                let doc = self.open(k)?;
                let id = id(doc).ok_or_else(|| Error::ClassNotFound(name.to_owned()))?;
                return Ok(Some((doc, id)));
            }
        }
        Ok(None)
    }

    /// The class `name` (dotted or `L..;`), as edited, with the file it is of.
    pub fn class(&mut self, name: &str) -> Result<(&mut Document, Class)> {
        let missing = || Error::ClassNotFound(name.to_owned());
        let (doc, id) = self.defining(name)?.ok_or_else(missing)?;
        let class = doc.class(id)?.ok_or_else(missing)?;
        Ok((doc, class))
    }

    /// The file a class goes to that no file defines yet: classes.dex.
    pub fn first(&mut self) -> Result<&mut Document> {
        match self.dex.is_empty() {
            true => Err(Error::EntryNotFound("classes.dex".into())),
            false => self.open(0),
        }
    }

    /// Give the APK the class the Smali `source` is, in place of the one of its name, or as a
    /// new class in classes.dex.
    pub fn set_text(&mut self, source: &str) -> Result<()> {
        let name = text::class_name(source)?;
        let doc = match self.defining(&name)? {
            Some((doc, _)) => doc,
            None => self.first()?,
        };
        let class = text::parse(doc.source_mut(), source)?;
        doc.set(class)
    }

    /// The classes, as edited, with an instruction that has the operand `operand` writes, by
    /// descriptor.
    pub fn find(&mut self, operand: &str) -> Result<Vec<String>> {
        let mut found = Vec::new();
        for k in 0..self.dex.len() {
            let doc = self.open(k)?;
            let Some(o) = text::find_operand(&doc.src, operand)? else {
                continue;
            };
            for name in doc.find(&o)? {
                found.push(String::from_utf8_lossy(doc.src.descriptor(name)).into_owned());
            }
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
