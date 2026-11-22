//! Smali round trip: DEX -> class model -> Smali text, and back to a DEX file.
//! Only used by editing commands; the query paths never load it.

pub mod document;
pub mod model;
pub mod read;
pub mod text;
pub mod write;

/// The 3-digit version of a DEX file (`035`, `038`, ...).
pub fn version(dex: &[u8]) -> [u8; 3] {
    dex.get(4..7)
        .and_then(|v| v.try_into().ok())
        .unwrap_or(*b"035")
}

pub use document::{Classes, Document};

use crate::apk::Apk;
use crate::{Error, Result};
use model::Class;
use std::io::{Read, Seek};

/// Round-trippable Smali of one class (dotted or `L..;` name), for editing then re-assembling.
pub fn export_class(path: &str, class: &str) -> Result<String> {
    export_class_from(&mut crate::apk::open(path)?, class)
}

/// [`export_class`] from an APK already open.
pub fn export_class_from<R: Read + Seek>(apk: &mut Apk<R>, class: &str) -> Result<String> {
    let desc = crate::dex::descriptor(class);
    for (name, bytes) in &apk.dex {
        if let Ok(Some(i)) = crate::dex::Dex::parse(bytes).map(|dx| dx.find_class(&desc)) {
            let c = read::Reader::new(bytes)
                .and_then(|r| r.class(i))
                .map_err(|e| e.at(name))?;
            return Ok(text::print(&c));
        }
    }
    Err(Error::ClassNotFound(class.to_owned()))
}

/// Assemble edited Smali `sources` into `edits`: each class replaces the one of the same name in
/// the DEX that defines it (a brand-new class goes to classes.dex). Returns the changed entries.
/// Parses everything first, so a syntax error aborts before any DEX is rewritten.
pub fn inject(
    path: &str,
    sources: &[String],
    edits: &mut crate::rebuild::Edits,
) -> Result<Vec<String>> {
    inject_into(&mut crate::apk::open(path)?, sources, edits)
}

/// [`inject`] into an APK already open.
pub fn inject_into<R: Read + Seek>(
    apk: &mut Apk<R>,
    sources: &[String],
    edits: &mut crate::rebuild::Edits,
) -> Result<Vec<String>> {
    let parsed: Vec<Class> = sources
        .iter()
        .map(|s| text::parse(s))
        .collect::<Result<_, _>>()?;
    let mut classes = Classes::new(apk.dex.clone());
    for class in parsed {
        classes.set(class)?;
    }
    classes.write(edits)
}

#[cfg(test)]
#[path = "../../../tests/apkscope-core/smali.rs"]
mod tests;
