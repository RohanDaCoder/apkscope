//! State accessors: class lookup and decompiled text, resources, the reference index
//! and the code cache.

use crate::cache;
use crate::dex;
use crate::index;
use crate::java;
use crate::res;
use crate::search::{self, Origin, origin, threads};

use super::State;

impl State {
    pub fn build_index(&self) {
        let list: Vec<(usize, usize)> = self.classes.iter().map(|c| (c.1, c.2)).collect();
        let idx = index::build(&self.dexes, &list, threads(), &self.indexed);
        let _ = self.index.set(idx);
    }

    pub fn cached(&self) -> bool {
        self.cache.read().unwrap().is_some()
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        let by = |n: &str| self.classes.binary_search_by(|c| c.0.as_str().cmp(n)).ok();
        by(name).or_else(|| {
            let desc = if name.starts_with('L') && name.ends_with(';') {
                name.to_string()
            } else {
                format!("L{};", name.replace('.', "/"))
            };
            by(&dex::java_name(&desc))
        })
    }

    pub fn origin_of(&self, name: &str) -> Origin {
        match self.find(name) {
            Some(k) => self.origin[k],
            None => origin(name, &self.package),
        }
    }

    pub(crate) fn java_of(
        &self,
        k: usize,
        reader: Option<(&cache::Reader, &mut std::fs::File)>,
    ) -> Option<String> {
        if let Some((r, f)) = reader
            && let Some(t) = r.get(f, k)
        {
            return Some(t);
        }
        let (_, d, c) = &self.classes[k];
        java::class(&self.dexes[*d], *c).ok()
    }

    pub fn class_text(&self, name: &str, smali: bool) -> Option<String> {
        let k = self.find(name)?;
        let (_, d, c) = &self.classes[k];
        let dx = &self.dexes[*d];
        if !smali {
            let reader = self.cache.read().unwrap().clone();
            let mut f = reader.as_ref().and_then(|r| r.file());
            let text = match (&reader, f.as_mut()) {
                (Some(r), Some(f)) => self.java_of(k, Some((r, f))),
                _ => None,
            };
            return Some(text.unwrap_or_else(|| {
                java::class(dx, *c).unwrap_or_else(|e| format!("/* failed: {e} */"))
            }));
        }
        Some(search::smali(dx, *c))
    }

    pub fn resource(&self, path: &str) -> Option<String> {
        if let Some(t) = self.texts.get(path) {
            return Some(t.clone());
        }
        // only paths from the table can be read, so nothing can reach other zip entries
        let src = self.files.get(path)?;
        let data = self.apk.lock().ok()?.read(src)?;
        match res::decode_xml(&data, self.names.as_ref()) {
            Ok(t) => Some(t),
            Err(_) => Some(format!("// binary file, {} bytes ({src})", data.len())),
        }
    }

    pub fn is_res(&self, t: &str) -> bool {
        self.texts.contains_key(t) || self.files.contains_key(t)
    }

    pub fn is_native(&self, t: &str) -> bool {
        t.ends_with(".so") && self.entries.iter().any(|e| e == t)
    }

    pub fn target_text(&self, t: &str) -> Option<String> {
        if self.is_native(t) {
            Some(self.native_report(t))
        } else if self.is_res(t) {
            self.resource(t)
        } else {
            self.class_text(t, false)
        }
    }

    pub fn read_entry(&self, name: &str) -> Option<Vec<u8>> {
        if !self.entries.iter().any(|e| e == name) {
            return None;
        }
        self.apk.lock().ok()?.read(name)
    }

    pub fn drop_cache(&self) {
        *self.cache.write().unwrap() = None;
    }
}
