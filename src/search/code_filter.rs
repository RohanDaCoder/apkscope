//! Which classes `search code` has to decompile. A line of decompiled Java is made of names and
//! literals the DEX holds (strings, types, fields, methods and their prototypes, under their
//! deobfuscated names), plus words the decompiler writes itself (keywords, `obj`, `Arr`, line
//! comments). A literal query is matched case-insensitively as a substring, so its longest word
//! is a substring of one word of every matching line. When that word cannot come from the
//! decompiler itself, only the classes that refer to a DEX entry containing it can match, and
//! the reference index lists them. Any doubt gives `None`: read every class, as before.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Every word of the decompiler's own source: a superset of what it can print by itself.
/// (`re.rs` only adds decrypted plaintext to the Java, and `decryptors` covers that.)
/// A query word inside one of these could be made up by the decompiler, so it can't narrow.
fn generated_words() -> &'static [String] {
    static WORDS: OnceLock<Vec<String>> = OnceLock::new();
    WORDS.get_or_init(|| {
        const SOURCES: [&str; 6] = [
            include_str!("../java.rs"),
            include_str!("../ir.rs"),
            include_str!("../opt.rs"),
            include_str!("../cfg.rs"),
            include_str!("../deobf.rs"),
            include_str!("../imports.rs"),
        ];
        let mut w: Vec<String> = SOURCES
            .iter()
            .flat_map(|s| s.split(|c: char| !c.is_ascii_alphanumeric() && c != '_'))
            .filter(|w| !w.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        w.sort_unstable();
        w.dedup();
        w
    })
}

/// The part of a literal query every matching line must hold in a word that comes from the DEX,
/// lowercased; `None` when the query can't narrow the search.
pub(crate) fn core(query: &str) -> Option<String> {
    let word = query
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '$')
        .max_by_key(|w| w.len())?;
    let mut t = word.to_ascii_lowercase();
    // local names add a suffix to a type name (`intentArr2`, `doVal`) and lead with `v` before a
    // digit; cutting what may be such an affix only shortens the word, which stays sound
    loop {
        let before = t.len();
        t = t.trim_end_matches(|c: char| c.is_ascii_digit()).into();
        for s in ["arr", "val"] {
            if let Some(x) = t.strip_suffix(s) {
                t = x.into();
            }
        }
        if t.len() == before {
            break;
        }
    }
    for s in ["ar", "va", "a", "v"] {
        if let Some(x) = t.strip_suffix(s) {
            t = x.into();
            break;
        }
    }
    let t = t
        .trim_start_matches('v')
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .to_string();
    // numbers print as words of their own (`0x7f0a0012`, `100L`, `1.5f`, `é`)
    if t.len() < 3 || t.chars().all(|c| "0123456789abcdefxlu_".contains(c)) {
        return None;
    }
    if generated_words().iter().any(|w| w.contains(&t)) {
        return None;
    }
    Some(t)
}

/// Top-level class ids whose own declaration (name, superclass, interfaces) contains `core`.
pub(crate) fn declared(st: &crate::session::State, core: &str) -> Vec<usize> {
    let has = |desc: &str| {
        desc.to_ascii_lowercase().contains(core)
            || crate::dex::java_name(desc)
                .to_ascii_lowercase()
                .contains(core)
    };
    let top: HashMap<(usize, usize), usize> = st
        .classes
        .iter()
        .enumerate()
        .map(|(k, c)| ((c.1, c.2), k))
        .collect();
    let mut out = Vec::new();
    for (d, dx) in st.dexes.iter().enumerate() {
        let parent = &dx.nesting().parent;
        for c in 0..dx.class_count {
            let Ok(def) = dx.class_def(c) else { continue };
            if !(has(&def.class)
                || def.superclass.as_deref().is_some_and(has)
                || def.interfaces.iter().any(|i| has(i)))
            {
                continue;
            }
            let mut root = c;
            while let Some(p) = parent.get(root).copied().flatten() {
                root = p;
            }
            if let Some(&k) = top.get(&(d, root)) {
                out.push(k);
            }
        }
    }
    out
}

/// String decryptors the APK defines, as (owner, name, prototype): a call to one prints its
/// plaintext beside it (`/* = "..." */`), text no DEX table holds, so its callers must be read.
/// Only a static method with code can be run, so only those are kept.
pub(crate) fn decryptors(st: &crate::session::State) -> HashSet<(String, String, String)> {
    let mut out = HashSet::new();
    for dx in st.dexes.iter() {
        for c in 0..dx.class_count {
            let Ok(data) = dx.class_data(c) else { continue };
            for m in &data.methods {
                if m.access & 0x8 == 0
                    || m.code.is_none()
                    || !crate::re::decryptor_shape(dx, m.method_idx)
                {
                    continue;
                }
                if let Ok((owner, name, p)) = dx.method_parts(m.method_idx)
                    && let Ok(proto) = dx.proto(p)
                {
                    out.insert((owner, name, proto));
                }
            }
        }
    }
    out
}

/// The printer writes a lambda's body where the lambda is created (`new L(..)`, or the read
/// of a non-capturing lambda's singleton field), so a lambda class that can match makes the
/// classes creating it match too. Repeated until nothing is added: a lambda can create one.
pub(crate) fn add_lambda_hosts(st: &crate::session::State, hit: &mut [bool]) {
    let Some(idx) = st.index.get() else { return };
    let mut todo: Vec<usize> = (0..hit.len()).filter(|&k| hit[k]).collect();
    while !todo.is_empty() {
        // lambda-shaped classes among the new ones: synthetic + final, one interface
        let mut lambdas: HashSet<String> = HashSet::new();
        for k in todo.drain(..) {
            let (_, d, c) = &st.classes[k];
            if let Ok(def) = st.dexes[*d].class_def(*c)
                && def.access & 0x1010 == 0x1010
                && def.interfaces.len() == 1
            {
                lambdas.insert(def.class);
            }
        }
        if lambdas.is_empty() {
            break;
        }
        for (d, dx) in st.dexes.iter().enumerate() {
            let [_, types, fields, _] = dx.counts();
            let hosts = (0..types)
                .filter(|&t| dx.type_name(t as u32).is_ok_and(|n| lambdas.contains(&n)))
                .flat_map(|t| idx.users(d, crate::index::TYPE, t))
                .chain(
                    (0..fields)
                        .filter(|&f| {
                            dx.field_parts(f as u32)
                                .is_ok_and(|(owner, _, _)| lambdas.contains(&owner))
                        })
                        .flat_map(|f| idx.users(d, crate::index::FIELD, f)),
                );
            for &k in hosts {
                if let Some(h) = hit.get_mut(k as usize)
                    && !*h
                {
                    *h = true;
                    todo.push(k as usize);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/apkscope/code_filter.rs"]
mod tests;
