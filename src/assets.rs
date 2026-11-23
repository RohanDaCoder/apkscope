//! What `assets/` and `res/raw/` hold for the search scopes: text files read as lines, and the
//! string table of a Hermes bytecode bundle (React Native's compiled JavaScript), where the
//! endpoints, keys and API paths of a React Native app live instead of the DEX.

use crate::search::{Hit, Matcher, Progress, Scope};
use crate::session::State;
use std::io::Read;
use std::sync::atomic::Ordering;

/// First 8 bytes of a Hermes bytecode file (`HermesMagic`, little-endian).
const HERMES_MAGIC: [u8; 8] = [0xC6, 0x1F, 0xBC, 0x03, 0xC1, 0x03, 0x19, 0x1F];

/// Extensions that are text: read whole, matched line by line.
const TEXT: [&str; 20] = [
    "json",
    "js",
    "mjs",
    "cjs",
    "html",
    "htm",
    "xml",
    "txt",
    "properties",
    "cfg",
    "conf",
    "ini",
    "yaml",
    "yml",
    "csv",
    "plist",
    "toml",
    "env",
    "graphql",
    "map",
];

/// Extensions that are never text or Hermes: not opened at all.
const BINARY: [&str; 25] = [
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "ico", "svg", "svgz", "ttf", "otf", "woff",
    "woff2", "mp3", "ogg", "wav", "m4a", "aac", "mp4", "webm", "so", "dex", "zip", "tflite", "pb",
];

pub fn is_asset(path: &str) -> bool {
    path.starts_with("assets/") || path.starts_with("res/raw/")
}

fn ext(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

pub fn is_hermes(data: &[u8]) -> bool {
    data.starts_with(&HERMES_MAGIC)
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// The string table of a Hermes bytecode file, in table order. The header fields read here
/// (counts up to the string storage size) and the order of the sections after the 128-byte
/// header are the same in every bytecode version React Native has shipped; the result is
/// checked against the file's length, and `None` means the layout was not the one expected.
pub fn hermes_strings(d: &[u8]) -> Option<Vec<String>> {
    if !is_hermes(d) {
        return None;
    }
    let file_len = u32_at(d, 32)? as usize;
    let functions = u32_at(d, 40)? as usize;
    let kinds = u32_at(d, 44)? as usize;
    let identifiers = u32_at(d, 48)? as usize;
    let strings = u32_at(d, 52)? as usize;
    let overflow = u32_at(d, 56)? as usize;
    let storage_size = u32_at(d, 60)? as usize;
    if file_len > d.len() {
        return None;
    }
    let align = |o: usize| (o + 3) & !3;
    // function headers (16 bytes each), string kinds, identifier hashes, then the tables
    let mut o = 128usize;
    o = align(o.checked_add(functions.checked_mul(16)?)?);
    o = align(o.checked_add(kinds.checked_mul(4)?)?);
    o = align(o.checked_add(identifiers.checked_mul(4)?)?);
    let small = o;
    o = align(o.checked_add(strings.checked_mul(4)?)?);
    let big = o;
    o = align(o.checked_add(overflow.checked_mul(8)?)?);
    let storage = d.get(o..o.checked_add(storage_size)?)?;
    if o + storage_size > file_len {
        return None;
    }
    let mut out = Vec::with_capacity(strings);
    for i in 0..strings {
        let e = u32_at(d, small + i * 4)?;
        let utf16 = e & 1 == 1;
        let (mut off, mut len) = (((e >> 1) & 0x7f_ffff) as usize, (e >> 24) as usize);
        if len == 0xff {
            // overflow entry: offset and length in the overflow table
            off = u32_at(d, big + off * 8)? as usize;
            len = u32_at(d, big + (((e >> 1) & 0x7f_ffff) as usize) * 8 + 4)? as usize;
        }
        let s = if utf16 {
            let b = storage.get(off..off.checked_add(len.checked_mul(2)?)?)?;
            let units: Vec<u16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            // Hermes stores ASCII strings one byte per character (Latin-1)
            storage
                .get(off..off.checked_add(len)?)?
                .iter()
                .map(|&b| b as char)
                .collect()
        };
        out.push(s);
    }
    Some(out)
}

/// What one asset is, from its name and first bytes.
#[derive(Debug, PartialEq)]
pub enum Kind {
    Text,
    Hermes,
    Other,
}

/// The kind of every asset, opening only those whose name does not tell: first bytes only.
pub fn kinds(st: &State) -> Vec<(String, Kind)> {
    let mut out = Vec::new();
    let Ok(mut apk) = st.apk.lock() else {
        return out;
    };
    for name in st
        .entries
        .iter()
        .filter(|e| is_asset(e) && !e.ends_with('/'))
    {
        let e = ext(name);
        let kind = if BINARY.contains(&e.as_str()) {
            Kind::Other
        } else {
            let mut head = [0u8; 512];
            let n = apk
                .zip
                .by_name(name)
                .ok()
                .and_then(|mut f| f.read(&mut head).ok())
                .unwrap_or(0);
            let head = &head[..n];
            if is_hermes(head) {
                Kind::Hermes
            } else if TEXT.contains(&e.as_str())
                || (!head.is_empty()
                    && !head.contains(&0)
                    && std::str::from_utf8(head)
                        .is_ok_and(|s| s.chars().all(|c| !c.is_control() || c.is_whitespace())))
            {
                Kind::Text
            } else {
                Kind::Other
            }
        };
        out.push((name.clone(), kind));
    }
    out
}

/// Hits of the URL, secrets and string scopes in assets: text lines (with line numbers) and
/// Hermes strings (line 0: a bundle has no source lines).
pub fn hits(st: &State, m: &Matcher, prog: &Progress) -> Vec<Hit> {
    let mut out = Vec::new();
    for (name, kind) in kinds(st) {
        if prog.cancel.load(Ordering::Relaxed) {
            break;
        }
        if kind == Kind::Other {
            continue;
        }
        let Some(data) = st.read_entry(&name) else {
            continue;
        };
        if kind == Kind::Hermes {
            if let Some(strings) = hermes_strings(&data) {
                for s in strings {
                    let hit = match m.scope {
                        Scope::String => m.query_hits(&s),
                        _ => m.line(&format!("\"{s}\"")),
                    };
                    if hit {
                        out.push((name.clone(), 0, s));
                    }
                }
            }
            continue;
        }
        let text = String::from_utf8_lossy(&data);
        for (ln, l) in text.lines().enumerate() {
            let hit = match m.scope {
                Scope::String => m.query_hits(l),
                _ => m.line(l),
            };
            if hit {
                // minified JS puts a whole bundle on one line: keep the part around the match
                out.push((name.clone(), ln + 1, clip(l.trim(), m)));
            }
        }
    }
    out
}

/// A long line cut to 300 characters around its first match.
fn clip(l: &str, m: &Matcher) -> String {
    const MAX: usize = 300;
    if l.len() <= MAX {
        return l.to_string();
    }
    let at = m.first_match(l).unwrap_or(0);
    let mut start = at.saturating_sub(MAX / 3);
    while !l.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + MAX).min(l.len());
    while !l.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        &l[start..end],
        if end < l.len() { "…" } else { "" }
    )
}

/// `info` line: how many assets, their size, the biggest one, and any Hermes bundle.
pub fn summary(st: &State) -> Option<String> {
    let Ok(mut apk) = st.apk.lock() else {
        return None;
    };
    let mut count = 0usize;
    let mut total = 0u64;
    let mut biggest = (0u64, String::new());
    let mut hermes = Vec::new();
    for name in st
        .entries
        .iter()
        .filter(|e| is_asset(e) && !e.ends_with('/'))
    {
        let Ok(mut f) = apk.zip.by_name(name) else {
            continue;
        };
        count += 1;
        total += f.size();
        if f.size() > biggest.0 {
            biggest = (f.size(), name.clone());
        }
        if !BINARY.contains(&ext(name).as_str()) {
            let mut head = [0u8; 8];
            if f.read_exact(&mut head).is_ok() && is_hermes(&head) {
                hermes.push(name.clone());
            }
        }
    }
    if count == 0 {
        return None;
    }
    let mb = |b: u64| b as f64 / 1_048_576.0;
    let mut s = format!(
        "assets: {count} files, {:.1} MB, largest {} ({:.1} MB)",
        mb(total),
        biggest.1,
        mb(biggest.0)
    );
    if !hermes.is_empty() {
        s += &format!(
            "; Hermes bundle: {} (search urls/secrets/string read its strings)",
            hermes.join(", ")
        );
    }
    Some(s)
}

#[cfg(test)]
#[path = "../tests/apkscope/assets.rs"]
mod tests;
