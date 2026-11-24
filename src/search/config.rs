//! What the app ships as changeable (the `config` scope): BuildConfig string
//! defaults and remote-config defaults XML (exact), and reads of Firebase Remote
//! Config / SharedPreferences grouped by key with their fallback values
//! (inferred from the decompiled call sites).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::index;
use crate::session::State;

use super::secrets::string_constants;
use super::{Hit, LIMIT, Matcher, Origin, threads};

/// `<entry><key>..</key><value>..</value></entry>` pairs of a remote-config defaults XML.
pub(crate) fn xml_entries(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in text.split("<entry>").skip(1) {
        let body = entry.split("</entry>").next().unwrap_or("");
        let grab = |tag: &str| {
            body.split(&format!("<{tag}>"))
                .nth(1)
                .and_then(|r| r.split(&format!("</{tag}>")).next())
                .map(str::to_string)
        };
        if let (Some(k), Some(v)) = (grab("key"), grab("value")) {
            out.push((k, v));
        }
    }
    out
}

/// (class, key, fallback) of one config read in code
type ConfigRead = (String, String, String);

/// Reads of Firebase Remote Config / SharedPreferences with their fallback values, from the
/// decompiled call sites. Heuristic — labelled `inferred` in the output: the first argument
/// is the key, the second (when a literal) the fallback. The caller classes are collected
/// from the reference index (cheap) and then decompiled in parallel through the same
/// machinery `scan` uses — all cores, stopping once `LIMIT` reads are in.
pub(crate) fn config_reads(st: &State) -> Vec<(String, String, String)> {
    let call = regex::Regex::new(
        r#"\.get(?:String|Boolean|Int|Long|Float|Double)\("([^"]*)"(?:,\s*"([^"]*)")?"#,
    )
    .unwrap();
    // candidates: app classes that call a config API
    let mut list = Vec::new();
    if let Some(idx) = st.index.get() {
        for (d, dx) in st.dexes.iter().enumerate() {
            for id in 0..dx.counts()[3] as u32 {
                let Ok((owner, name, _)) = dx.method_parts(id) else {
                    continue;
                };
                let config_api = (owner
                    == "Lcom/google/firebase/remoteconfig/FirebaseRemoteConfig;"
                    && name.starts_with("get"))
                    || (owner == "Landroid/content/SharedPreferences;" && name.starts_with("get"));
                if !config_api {
                    continue;
                }
                for &k in idx.users(d, index::METHOD, id as usize) {
                    if st.origin[k as usize] != Origin::Library {
                        list.push(k as usize);
                    }
                }
            }
        }
    }
    list.sort_unstable();
    list.dedup();
    // parallel decompile of the candidates, the scan way
    let reader = st.cache.read().unwrap().clone();
    let next = AtomicUsize::new(0);
    // tagged with the position in `list`, cut by a shared count: the same reads every run (see
    // `scan` in indexed.rs)
    let found: Mutex<Vec<(usize, ConfigRead)>> = Mutex::new(Vec::new());
    let count = AtomicUsize::new(0);
    let full = AtomicBool::new(false);
    std::thread::scope(|s| {
        for _ in 0..threads() {
            let _ = std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn_scoped(s, || {
                    let mut f = reader.as_ref().and_then(|r| r.file());
                    let mut mine = Vec::new();
                    loop {
                        if full.load(Ordering::Relaxed) {
                            break;
                        }
                        let n = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&k) = list.get(n) else { break };
                        let text = match (&reader, f.as_mut()) {
                            (Some(r), Some(f)) => st.java_of(k, Some((r, f))),
                            _ => st.java_of(k, None),
                        };
                        if let Some(text) = text {
                            let class = &st.classes[k].0;
                            let before = mine.len();
                            for cap in call.captures_iter(&text) {
                                mine.push((
                                    n,
                                    (
                                        class.clone(),
                                        cap[1].to_string(),
                                        cap.get(2)
                                            .map(|x| x.as_str().to_string())
                                            .unwrap_or_default(),
                                    ),
                                ));
                            }
                            let added = mine.len() - before;
                            if count.fetch_add(added, Ordering::Relaxed) + added >= LIMIT {
                                full.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                    found.lock().unwrap().extend(mine);
                });
        }
    });
    let mut tagged = found.into_inner().unwrap();
    if full.load(Ordering::Relaxed) {
        tagged.sort_by_key(|r| r.0);
        tagged.truncate(LIMIT);
    }
    let mut out: Vec<(String, String, String)> = tagged.into_iter().map(|r| r.1).collect();
    out.sort();
    out.dedup();
    out
}

/// The `search config` sheet: what the app ships as changeable, in three labelled tiers —
/// BuildConfig string defaults (the `*.BuildConfig` classes, and only those) and
/// remote-config defaults are exact (read from the DEX tables and the decoded XML); code
/// reads of Firebase Remote Config / SharedPreferences are inferred from the decompiled
/// call sites, grouped by key, and say so.
pub fn config_sheet(st: &State, m: &Matcher) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    for c in string_constants(&st.dexes) {
        let build_config = c.class == "BuildConfig" || c.class.ends_with(".BuildConfig");
        if !build_config || st.origin_of(&c.class) == Origin::Library {
            continue;
        }
        let line = format!("default: {}#{} = \"{}\"", c.class, c.field, c.value);
        if m.line(&line) {
            hits.push((c.class.clone(), 0, line));
        }
    }
    for (p, t) in st.texts() {
        if !p.starts_with("res/xml")
            || !(p.contains("remote_config") || p.contains("firebase_config"))
        {
            continue;
        }
        for (k, v) in xml_entries(t) {
            let line = format!("remote default: {k} = \"{v}\"    ({p})");
            if m.line(&line) {
                hits.push((p.clone(), 0, line));
            }
        }
    }
    // reads, grouped by key: the same key read in several classes collapses to one row,
    // the fallback shows when any call site ships one, and rows with a fallback come first
    let mut reads: Vec<(String, String, Vec<String>)> = Vec::new();
    for (class, key, fb) in config_reads(st) {
        match reads.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, f, cs)) => {
                if f.is_empty() {
                    *f = fb;
                }
                cs.push(class);
            }
            None => reads.push((key, fb, vec![class])),
        }
    }
    reads.sort_by(|a, b| {
        a.1.is_empty()
            .cmp(&b.1.is_empty())
            .then_with(|| a.0.cmp(&b.0))
    });
    for (key, fb, mut cs) in reads {
        cs.sort();
        cs.dedup();
        let at = if cs.len() == 1 {
            format!("in {}", cs[0])
        } else {
            format!("in {} +{} more", cs[0], cs.len() - 1)
        };
        let line = if fb.is_empty() {
            format!("read: {key}    (inferred, no literal fallback)    {at}")
        } else {
            format!("read: {key}    fallback: \"{fb}\"    (inferred)    {at}")
        };
        if m.line(&line) {
            hits.push((cs.swap_remove(0), 0, line));
        }
    }
    hits.truncate(LIMIT);
    hits
}
