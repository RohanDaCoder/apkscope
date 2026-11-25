//! An APK (or a JAR of DEX files) open, from a file or from anything that reads and seeks, such
//! as a `Cursor` over bytes already in memory. Opening it reads only the list of its entries:
//! an entry, its DEX files and its resource table are read when something asks for them, and
//! the APK keeps only what an edit changed, which [`Apk::rebuild`] writes with everything else
//! as it was.
//!
//! Its files are edited as files ([`Apk::put`], [`Apk::remove`]), its classes through
//! [`Apk::classes`], a value of its resource table by [`Apk::set_value`].

use crate::resources::Resources;
use crate::smali::Classes;
use crate::{Error, Result, rebuild};
use std::fs;
use std::io::{Read, Seek, Write};

pub struct Apk<R = fs::File> {
    pub zip: zip::ZipArchive<R>,
    /// The entries the edits changed, by name: the new content of one, or `None` for one that
    /// is removed.
    pub(crate) entries: rebuild::Edits,
    /// The DEX files, once an edit reaches a class.
    classes: Option<Classes>,
    /// The resource table and what goes with it, once an edit reaches a resource.
    pub(crate) resources: Option<Resources>,
}

pub fn open(path: &str) -> Result<Apk> {
    let file = fs::File::open(path).map_err(Error::io(path))?;
    Apk::new(file).map_err(|e| e.at(path))
}

/// Whether Android loads an entry as code: only root-level classes*.dex.
pub(crate) fn is_dex(name: &str) -> bool {
    name.starts_with("classes") && name.ends_with(".dex") && !name.contains('/')
}

impl<R: Read + Seek> Apk<R> {
    pub fn new(reader: R) -> Result<Self> {
        Ok(Apk {
            zip: zip::ZipArchive::new(reader)?,
            entries: rebuild::Edits::new(),
            classes: None,
            resources: None,
        })
    }

    /// The entry `name`, as the edits left it.
    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        if let Some(edited) = self.entries.get(name) {
            return edited.clone();
        }
        let mut e = self.zip.by_name(name).ok()?;
        let mut buf = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut buf).ok()?;
        Some(buf)
    }

    /// Whether the APK has the entry `name`, as the edits left it.
    pub(crate) fn has(&self, name: &str) -> bool {
        match self.entries.get(name) {
            Some(edited) => edited.is_some(),
            None => self.zip.index_for_name(name).is_some(),
        }
    }

    /// The root-level classes*.dex by entry name, in load order: classes.dex, classes2.dex, ...
    /// (the first definition of a class wins at runtime). The names without the reads, so a
    /// loader that streams the files through other handles reads them in the same order.
    pub fn dex_names(&mut self) -> Vec<String> {
        let mut names: Vec<String> = self
            .zip
            .file_names()
            .flatten()
            .filter(|n| is_dex(n))
            .map(|n| n.into_owned())
            .collect();
        names.sort_by_key(|n| {
            n.trim_start_matches("classes")
                .trim_end_matches(".dex")
                .parse::<u32>()
                .unwrap_or(1)
        });
        names
    }

    /// Every root-level classes*.dex, read in load order.
    pub fn dex(&mut self) -> Result<Vec<(String, Vec<u8>)>> {
        self.dex_names()
            .into_iter()
            .map(|name| match self.read(&name) {
                Some(data) => Ok((name, data)),
                None => Err(Error::EntryNotFound(name)),
            })
            .collect()
    }

    /// The APK's DEX files, open for edits.
    pub fn classes(&mut self) -> Result<&mut Classes> {
        if self.classes.is_none() {
            self.classes = Some(Classes::new(self.dex()?));
        }
        Ok(self.classes.as_mut().expect("opened above"))
    }

    /// How the APK's entries change for the edits so far: the files they changed, added or
    /// removed, the DEX files of the classes they changed, binary XML compiled from the text
    /// they gave, and the resource table if they changed it.
    pub fn edits(&mut self) -> Result<&rebuild::Edits> {
        self.write_resources()?;
        if let Some(classes) = &self.classes {
            classes.write(&mut self.entries)?;
        }
        Ok(&self.entries)
    }

    /// Write the APK as the edits left it into `out`, unsigned: [`rebuild::archive`] with
    /// [`Apk::edits`]. How many bytes that wrote.
    pub fn rebuild<W: Write>(&mut self, out: W) -> Result<u64> {
        self.edits()?;
        rebuild::archive(&mut self.zip, &self.entries, out)
    }
}
