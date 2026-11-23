//! An APK (or a JAR of DEX files) as Android loads it, from a file or from anything that reads
//! and seeks, such as a `Cursor` over bytes already in memory. Opening it reads only the list of
//! its entries: an entry, or its DEX files, is read when something asks for it.

use crate::{Error, Result};
use std::fs;
use std::io::{Read, Seek};

pub struct Apk<R = fs::File> {
    pub zip: zip::ZipArchive<R>,
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
        })
    }

    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let mut e = self.zip.by_name(name).ok()?;
        let mut buf = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut buf).ok()?;
        Some(buf)
    }

    /// The root-level classes*.dex by entry name, in load order: classes.dex, classes2.dex, ...
    /// (the first definition of a class wins at runtime).
    pub fn dex(&mut self) -> Result<Vec<(String, Vec<u8>)>> {
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
            .into_iter()
            .map(|name| match self.read(&name) {
                Some(data) => Ok((name, data)),
                None => Err(Error::EntryNotFound(name)),
            })
            .collect()
    }
}
