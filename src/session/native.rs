//! Native libraries of the APK: the lib/ listing, the `native` methods declared in
//! the DEX, and the readable report of one library.

use std::fmt::Write as _;

use crate::elf;
use crate::search::{SECRET_RE, URL_RE};

use super::State;

impl State {
    pub fn native_libs(&self) -> Vec<(String, u64)> {
        let mut apk = self.apk.lock().unwrap();
        let mut v: Vec<(String, u64)> = self
            .entries
            .iter()
            .filter(|e| e.starts_with("lib/") && e.ends_with(".so"))
            .map(|e| (e.clone(), apk.zip.by_name(e).map_or(0, |f| f.size())))
            .collect();
        v.sort();
        v
    }

    pub fn native_methods(&self) -> &[(String, String)] {
        self.natives.get_or_init(|| {
            let mut v = Vec::new();
            for dx in &self.dexes {
                for c in 0..dx.class_count {
                    let Ok(data) = dx.class_data(c) else { continue };
                    for m in data.methods.iter().filter(|m| m.access & 0x100 != 0) {
                        if let (Ok(desc), Ok((_, name, _))) =
                            (dx.class_descriptor(c), dx.method_parts(m.method_idx))
                        {
                            v.push((
                                desc.trim_start_matches('L')
                                    .trim_end_matches(';')
                                    .replace(['/', '$'], "."),
                                name,
                            ));
                        }
                    }
                }
            }
            v.sort();
            v.dedup();
            v
        })
    }

    pub fn native_report(&self, path: &str) -> String {
        let Some(data) = self.read_entry(path) else {
            return format!("// {path}: not found");
        };
        let mut o = String::new();
        let _ = writeln!(
            o,
            "// {path}
"
        );
        let e = match elf::parse(&data) {
            Ok(e) => e,
            Err(err) => return format!("{o}// {err}, {} bytes", data.len()),
        };
        let _ = writeln!(o, "size              {} bytes", data.len());
        let _ = writeln!(o, "architecture      {} ({}-bit)", e.machine, e.bits);
        let _ = writeln!(
            o,
            "JNI_OnLoad        {}",
            if e.exports.iter().any(|x| x == "JNI_OnLoad") {
                "yes: natives may also be registered at runtime (RegisterNatives), which can't be mapped statically"
            } else {
                "no"
            }
        );
        let _ = writeln!(
            o,
            "
// needs ({})",
            e.needed.len()
        );
        for n in &e.needed {
            let _ = writeln!(o, "{n}");
        }
        let natives = self.native_methods();
        let jni: Vec<(&String, (String, String))> = e
            .exports
            .iter()
            .filter_map(|x| elf::jni_target(x).map(|t| (x, t)))
            .collect();
        let _ = writeln!(
            o,
            "
// JNI exports -> Java native methods ({})",
            jni.len()
        );
        for (sym, (cls, m)) in &jni {
            let found = natives.binary_search(&(cls.clone(), m.clone())).is_ok();
            let _ = writeln!(
                o,
                "{cls}.{m}(){}    <- {sym}",
                if found {
                    ""
                } else {
                    "    (no such native method in the DEX)"
                }
            );
        }
        let _ = writeln!(
            o,
            "
// other exports ({})",
            e.exports.len() - jni.len()
        );
        for x in e
            .exports
            .iter()
            .filter(|x| !x.starts_with("Java_"))
            .take(2000)
        {
            let _ = writeln!(o, "{x}");
        }
        let all = elf::strings(&data, 6);
        let url = regex::Regex::new(URL_RE).unwrap();
        let secret = regex::Regex::new(SECRET_RE).unwrap();
        let interesting: Vec<&String> = all
            .iter()
            .filter(|s| {
                url.is_match(&format!("\"{s}\""))
                    || secret.is_match(&format!("\"{s}\""))
                    || secret.is_match(s)
            })
            .collect();
        let _ = writeln!(
            o,
            "
// URL- or key-like strings ({}). Heuristic: encrypted or computed values won't show",
            interesting.len()
        );
        for s in &interesting {
            let _ = writeln!(o, "{s}");
        }
        let _ = writeln!(
            o,
            "
// all strings, 6+ characters ({}{})",
            all.len().min(5000),
            if all.len() > 5000 { ", first 5000" } else { "" }
        );
        for s in all.iter().take(5000) {
            let _ = writeln!(o, "{s}");
        }
        o
    }
}
