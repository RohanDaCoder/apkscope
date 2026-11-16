//! ProGuard/R8 `mapping.txt` parsing: the format obfuscated builds ship next
//! to crash reports. Lines are `original -> obfuscated:` for classes and
//! `    type name -> obfuscated` for members, with method signatures in
//! parentheses and optional `line:first:last` tails when line numbers were
//! kept. Comments (`#`) and blank lines are ignored. The file maps in the
//! *renaming* direction only — it carries no comments or position data worth
//! keeping, so importing yields classes and members, never comments.

use crate::deobf::Notes;
use std::collections::HashMap;

/// One parsed mapping file, kept in file order so merging is deterministic.
pub struct ProGuard {
    /// `Lc/d/E;` (obfuscated, as in the APK) -> `a.b.C` (original, dotted)
    pub classes: HashMap<String, String>,
    /// (`Lc/d/E;`, obfuscated member) -> original name, in file order;
    /// methods carry their signature in the key, overloads stay distinct
    pub members: Vec<((String, String), String)>,
}

/// Parse a `mapping.txt`. Unknown lines are skipped; a member before any
/// class header is ignored (the format guarantees one comes first).
pub fn parse(text: &str) -> ProGuard {
    let mut p = ProGuard {
        classes: HashMap::new(),
        members: Vec::new(),
    };
    let mut cls: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        // class header: `a.b.C -> c.d.E:` — the colon is always present in
        // files R8/proguard actually write, but tolerate its absence
        if !line.starts_with(' ') {
            let Some((from, to)) = split_arrow(line) else {
                continue;
            };
            let to = to.trim_end_matches(':').trim();
            if to.is_empty() || from.is_empty() {
                continue;
            }
            let key = crate::re::desc(to);
            cls = Some(key.clone());
            p.classes.insert(key, from.trim().to_string());
            continue;
        }
        let Some(cls) = &cls else { continue };
        // member: `    java.lang.String f -> a` or `    void run() -> a:12:34`;
        // the type is the first token, the original name the second, and the
        // obfuscated side may carry `:int` / `:12:34` suffixes to strip
        let line = line.trim();
        let Some((from, to)) = split_arrow(line) else {
            continue;
        };
        let to = to.split(':').next().unwrap_or(to).trim();
        let mut parts = from.split_whitespace();
        let Some(_ty) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        // method: the original name ends at '('; the parenthesized signature
        // (if any) belongs to the key so overloads map apart
        let (name, sig) = match name.split_once('(') {
            Some((n, rest)) => (n, Some(format!("({}", rest.trim_start_matches(')').trim()))),
            None => (name, None),
        };
        if name.is_empty() || to.is_empty() {
            continue;
        }
        let key = if sig.is_some() {
            format!("{cls}->{to}()")
        } else {
            format!("{cls}->{to}")
        };
        p.members.push(((cls.clone(), key), name.to_string()));
    }
    p
}

fn split_arrow(line: &str) -> Option<(&str, &str)> {
    // `->` is the separator; a `>` alone (generics in the type token) must
    // not split it, so match the full two-char arrow
    let at = line.find(" -> ").map(|x| (x, 4)).or_else(|| {
        let x = line.find("->")?;
        Some((x, 2))
    })?;
    let (a, b) = line.split_at(at.0);
    Some((a, &b[at.1..]))
}

/// Merge a parsed mapping into the user's notes. ProGuard names the *original*
/// side, apkscope renames the *obfuscated* side, so obfuscated becomes the
/// key and original the value. Returns how many names were taken.
pub fn into_notes(p: ProGuard, n: &mut Notes) -> usize {
    let mut count = 0;
    for (obf, orig) in &p.classes {
        // original is dotted `a.b.C`; keep the package, rejoin as a
        // descriptor so the whole name machinery stays descriptor-based
        let new = crate::re::desc(orig);
        let taken = n
            .classes
            .insert(obf.clone(), new.clone())
            .is_none_or(|old| old != new);
        if !orig.is_empty() && taken {
            count += 1;
        }
    }
    for ((_, key), orig) in &p.members {
        // method keys already end in `()`, field keys do not — the notes
        // format's own distinction (see `apply_note`, `export_mapping`),
        // so methods and fields of one obfuscated name never collide
        let taken = n
            .members
            .insert(key.clone(), orig.clone())
            .is_none_or(|old| old != orig.as_str());
        if !orig.is_empty() && taken {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
#[path = "../tests/apkscope/proguard.rs"]
mod tests;
