//! Smali round trip: DEX -> class model -> Smali text, and back to a DEX file.
//! Only used by editing commands; the query paths never load it.

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

use crate::apk::Apk;
use crate::{Error, Result};
use model::Class;
use std::io::{Read, Seek};

/// `classesN.dex` entries, in load order (classes.dex, classes2.dex, ...).
fn dex_entries<R: Read + Seek>(apk: &Apk<R>) -> Vec<String> {
    let mut v: Vec<String> = apk
        .zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .filter(|n| n.starts_with("classes") && n.ends_with(".dex"))
        .collect();
    v.sort_by_key(|n| (n.len(), n.clone())); // classes.dex before classes2.dex ... classes10.dex
    v
}

/// Round-trippable Smali of one class (dotted or `L..;` name), for editing then re-assembling.
pub fn export_class(path: &str, class: &str) -> Result<String> {
    export_class_from(&mut crate::apk::open(path)?, class)
}

/// [`export_class`] from an APK already open.
pub fn export_class_from<R: Read + Seek>(apk: &mut Apk<R>, class: &str) -> Result<String> {
    let desc = crate::dex::descriptor(class);
    for name in dex_entries(apk) {
        let Some(bytes) = apk.read(&name) else {
            continue;
        };
        if let Ok(Some(i)) = crate::dex::Dex::parse(&bytes).map(|dx| dx.find_class(&desc)) {
            let c = read::Reader::new(&bytes)
                .and_then(|r| r.class(i))
                .map_err(|e| e.at(&name))?;
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
    let classes: Vec<Class> = sources
        .iter()
        .map(|s| text::parse(s))
        .collect::<Result<_, _>>()?;
    let entries = dex_entries(apk);
    // group each edited class under the dex entry it belongs to
    let mut by_dex: std::collections::BTreeMap<String, Vec<Class>> = Default::default();
    'next: for c in classes {
        let desc = String::from_utf8_lossy(&c.name).into_owned();
        for name in &entries {
            if let Some(bytes) = apk.read(name)
                && crate::dex::Dex::parse(&bytes)
                    .ok()
                    .and_then(|dx| dx.find_class(&desc))
                    .is_some()
            {
                by_dex.entry(name.clone()).or_default().push(c);
                continue 'next;
            }
        }
        by_dex.entry("classes.dex".into()).or_default().push(c);
    }
    let mut changed = Vec::new();
    for (name, edited) in by_dex {
        let bytes = apk
            .read(&name)
            .ok_or_else(|| Error::EntryNotFound(name.clone()))?;
        // fast path: patch instruction bytes in place when the edits only change existing methods'
        // code, so a one-class edit doesn't model and rewrite the whole DEX
        let patched = {
            let dx = crate::dex::Dex::parse(&bytes).map_err(|e| e.at(&name))?;
            let indexed: Option<Vec<(usize, &Class)>> = edited
                .iter()
                .map(|c| {
                    dx.find_class(&String::from_utf8_lossy(&c.name))
                        .map(|k| (k, c))
                })
                .collect();
            match indexed {
                Some(ix) => write::patch(&bytes, &ix).map_err(|e| e.at(&name))?,
                None => None, // a brand-new class: no in-place slot
            }
        };
        let dex = match patched {
            Some(d) => d,
            None => {
                let mut all = read::read(&bytes).map_err(|e| e.at(&name))?;
                for c in edited {
                    match all.iter().position(|x| x.name == c.name) {
                        Some(k) => all[k] = c,
                        None => all.push(c),
                    }
                }
                write::write(&all, version(&bytes)).map_err(|e| e.at(&name))?
            }
        };
        edits.insert(name.clone(), Some(dex));
        changed.push(name);
    }
    Ok(changed)
}

#[cfg(test)]
#[path = "../../../tests/apkscope-core/smali.rs"]
mod tests;
