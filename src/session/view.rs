//! Viewer-only state surface: resident memory, manifest re-encoding, min SDK, the
//! file list, single-file views (text or hex), whole-APK extraction and the strings
//! browser. One module-level `gui` gate instead of an attribute on every item.

use std::fmt::Write as _;

use crate::index;
use crate::res;
use crate::search::attr;

use super::State;

/// Resident memory of this process, in bytes.
pub fn memory_used() -> Option<u64> {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct Counters {
            cb: u32,
            faults: u32,
            peak_ws: usize,
            ws: usize,
            a: usize,
            b: usize,
            c: usize,
            d: usize,
            pagefile: usize,
            peak_pagefile: usize,
        }
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(p: isize, c: *mut Counters, cb: u32) -> i32;
        }
        let mut c = Counters {
            cb: std::mem::size_of::<Counters>() as u32,
            ..Default::default()
        };
        // safety: c is a correctly sized PROCESS_MEMORY_COUNTERS that outlives the call
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        (ok != 0).then_some(c.ws as u64)
    }
    #[cfg(not(windows))]
    {
        let s = std::fs::read_to_string("/proc/self/statm").ok()?;
        s.split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|p| p * 4096)
    }
}

impl State {
    pub fn encode_manifest(&self, text: &str) -> Result<Vec<u8>, String> {
        let names = self
            .names
            .as_ref()
            .ok_or("this APK has no resources.arsc to resolve @references")?;
        Ok(crate::axml::encode(
            text,
            self.read_entry("AndroidManifest.xml").as_deref(),
            names,
        )?)
    }

    pub fn min_sdk(&self) -> Option<u32> {
        let m = self.texts.get("AndroidManifest.xml")?;
        let at = m.find("<uses-sdk")?;
        attr(&m[at..], "android:minSdkVersion")?.parse().ok()
    }

    pub fn files_list(&self) -> Vec<(String, u64, u64)> {
        let mut apk = self.apk.lock().unwrap();
        let mut v: Vec<(String, u64, u64)> = self
            .entries
            .iter()
            .filter(|e| !e.ends_with('/'))
            .map(|e| {
                let (s, c) = apk
                    .zip
                    .by_name(e)
                    .map_or((0, 0), |f| (f.size(), f.compressed_size()));
                (e.clone(), s, c)
            })
            .collect();
        v.sort();
        v
    }

    pub fn file_view(&self, name: &str) -> String {
        let Some(d) = self.read_entry(name) else {
            return format!("// {name}: not found");
        };
        let mut o = format!(
            "// {name}
// {} bytes

",
            d.len()
        );
        if let Ok(t) = res::decode_xml(&d, self.names.as_ref()) {
            return o + &t;
        }
        match std::str::from_utf8(&d) {
            Ok(t) if d.len() < 4 << 20 && !t.contains('\0') => o + t,
            _ => {
                for (k, row) in d.chunks(16).take(256).enumerate() {
                    let hexs: String = row.iter().map(|b| format!("{b:02x} ")).collect();
                    let text: String = row
                        .iter()
                        .map(|&b| {
                            if (0x20..0x7f).contains(&b) {
                                b as char
                            } else {
                                '.'
                            }
                        })
                        .collect();
                    let _ = writeln!(o, "{:08x}  {hexs:<48} {text}", k * 16);
                }
                if d.len() > 4096 {
                    let _ = writeln!(
                        o,
                        "
// first 4096 bytes shown; extract the file for the rest"
                    );
                }
                o
            }
        }
    }

    pub fn extract_all(&self, dir: &std::path::Path) -> Result<usize, String> {
        let mut n = 0;
        for (name, _, _) in self.files_list() {
            let rel = std::path::Path::new(&name);
            if name.contains("..") || rel.is_absolute() || name.contains(':') {
                continue;
            }
            let out = dir.join(rel);
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            if let Some(d) = self.read_entry(&name) {
                std::fs::write(&out, d).map_err(|e| format!("{}: {e}", out.display()))?;
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn strings(&self, needle: &str, limit: usize) -> Vec<(String, usize)> {
        let low = needle.to_lowercase();
        let idx = self.index.get();
        let mut out = Vec::new();
        for (d, dx) in self.dexes.iter().enumerate() {
            for id in 0..dx.counts()[0] {
                let Ok(s) = dx.string(id as u32) else {
                    continue;
                };
                if !s.trim().is_empty() && s.to_lowercase().contains(&low) {
                    let users = idx.map_or(0, |i| i.users(d, index::STRING, id).len());
                    out.push((s, users));
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
        out
    }
}
