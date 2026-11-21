//! The indexed search driver: the candidate classes the reference index can narrow a
//! search to, the parallel decompile-and-match scan (which fills the code cache on the
//! way), and the per-scope result summaries. Runs on a `session::State`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::cache;
use crate::dex;
use crate::elf;
use crate::index;
use crate::session::State;

use super::api_sheet;
use super::{CACHE_WRITE, Hit, LIMIT, Matcher, Origin, Progress, Scope, threads};

/// prefilter for HTTP: types and member names a matching line would mention
const HTTP_TYPES_RE: &str = r"retrofit2/|okhttp3/(Request|HttpUrl|WebSocket|Call|OkHttpClient)|Https?URLConnection|WebSocket";

const HTTP_MEMBERS: [&str; 7] = [
    "openConnection",
    "addHeader",
    "header",
    "newCall",
    "baseUrl",
    "setRequestMethod",
    "url",
];

const SECRET_NAME_RE: &str = r"(?i)api_?key|secret|token|password|passwd|client_?id|private_?key|access_?key|auth_?header|sign_?key|signing_?salt|hmac_?key|hash_?key|encryption_?key|license_?key|merchant_?id|partner_?code";

/// Classes that can contain a match, from the index; None = every class must be read.
fn candidates(st: &State, m: &Matcher) -> Option<Vec<usize>> {
    let idx = st.index.get()?;
    let wants = |kind: usize| match m.scope {
        Scope::String | Scope::Urls => kind == index::STRING,
        Scope::Usages | Scope::Secrets => true,
        Scope::Http => kind != index::STRING,
        _ => false,
    };
    if !(0..4).any(wants) {
        return None;
    }
    let http_types = regex::Regex::new(HTTP_TYPES_RE).unwrap();
    let secret_names = regex::Regex::new(SECRET_NAME_RE).unwrap();
    let mut hit = vec![false; st.classes.len()];
    let tables: Vec<(usize, usize)> = st
        .dexes
        .iter()
        .enumerate()
        .flat_map(|(d, _)| (0..4).filter(|&k| wants(k)).map(move |k| (d, k)))
        .collect();
    // one table per task: strings of a big DEX are the slow part
    let found = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads().min(tables.len()).max(1) {
            s.spawn(|| {
                let mut mine = Vec::new();
                loop {
                    let t = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(d, kind)) = tables.get(t) else {
                        break;
                    };
                    let dx = &st.dexes[d];
                    for id in 0..dx.counts()[kind] {
                        let ok = match kind {
                            index::STRING => dx.string(id as u32).is_ok_and(|s| match m.scope {
                                Scope::String | Scope::Usages => m.query_hits(&s),
                                _ => m.line(&format!("\"{s}\"")),
                            }),
                            index::TYPE => dx.type_name(id as u32).is_ok_and(|t| match m.scope {
                                Scope::Http => http_types.is_match(&t),
                                Scope::Usages => {
                                    m.query_hits(&dex::java_name(&t))
                                        || crate::deobf::class(&t).is_some_and(|n| m.query_hits(n))
                                }
                                _ => false,
                            }),
                            _ => {
                                let (owner, name, ty) = if kind == index::FIELD {
                                    dx.field_parts(id as u32)
                                        .map(|(c, t, n)| (c, n, t))
                                        .unwrap_or_default()
                                } else {
                                    dx.method_parts(id as u32)
                                        .map(|(c, n, _)| (c, n, String::new()))
                                        .unwrap_or_default()
                                };
                                let shown = if kind == index::FIELD {
                                    crate::deobf::field(&owner, &name, &ty)
                                } else {
                                    crate::deobf::method(&owner, &name)
                                };
                                match m.scope {
                                    Scope::Http => {
                                        kind == index::METHOD
                                            && HTTP_MEMBERS.contains(&name.as_str())
                                    }
                                    Scope::Secrets => secret_names.is_match(&name),
                                    Scope::Usages => {
                                        m.query_hits(&name)
                                            || shown.is_some_and(|n| m.query_hits(&n))
                                    }
                                    _ => false,
                                }
                            }
                        };
                        if ok {
                            mine.extend_from_slice(idx.users(d, kind, id));
                        }
                    }
                }
                found.lock().unwrap().extend(mine);
            });
        }
    });
    for k in found.into_inner().unwrap() {
        if let Some(h) = hit.get_mut(k as usize) {
            *h = true;
        }
    }
    Some(
        hit.iter()
            .enumerate()
            .filter(|x| *x.1)
            .map(|x| x.0)
            .collect(),
    )
}

pub fn search(st: &State, m: &Matcher, prog: &Progress) -> Vec<Hit> {
    search_stats(st, m, prog).0
}

/// `search` plus what the app-only filter hid: for `class`, the hidden name matches
/// themselves; for the decompiling scopes, the library classes the index said could match
/// but that were never decompiled. Callers print it so nothing disappears silently now
/// that the API scopes default to app-only.
pub fn search_stats(st: &State, m: &Matcher, prog: &Progress) -> (Vec<Hit>, usize) {
    let mut hits = Vec::new();
    let mut hidden = 0usize;
    if m.scope == Scope::Class {
        for (k, c) in st.classes.iter().enumerate() {
            if !m.line(&c.0) {
                continue;
            }
            if m.app_only && st.origin[k] == Origin::Library {
                hidden += 1;
                continue;
            }
            if hits.len() < LIMIT {
                hits.push((c.0.clone(), 0, String::new()));
            }
        }
        return (hits, hidden);
    }
    if m.scope == Scope::Api {
        let mut api = api_sheet(st, m);
        if m.app_only {
            api.retain(|h| {
                st.find(&h.0)
                    .is_none_or(|k| st.origin[k] != Origin::Library)
            });
        }
        return (api, 0);
    }
    if matches!(
        m.scope,
        Scope::Res | Scope::Urls | Scope::Secrets | Scope::Usages
    ) {
        for (p, t) in &st.texts {
            // translations repeat the default strings; API scopes only need res/values
            if m.scope != Scope::Res && p.starts_with("res/values-") {
                continue;
            }
            for (ln, l) in t.lines().enumerate() {
                if m.line(l) {
                    hits.push((p.clone(), ln + 1, l.trim().to_string()));
                }
            }
        }
        if m.scope == Scope::Res {
            hits.extend(
                st.files
                    .keys()
                    .filter(|p| m.line(p))
                    .map(|p| (p.clone(), 0, String::new())),
            );
        }
    }
    // endpoints and keys are often kept in native code; one ABI is enough, the others repeat it
    if matches!(m.scope, Scope::Urls | Scope::Secrets) {
        let libs = st.native_libs();
        let abi = [
            "lib/arm64-v8a/",
            "lib/armeabi-v7a/",
            "lib/x86_64/",
            "lib/x86/",
        ]
        .into_iter()
        .find(|a| libs.iter().any(|l| l.0.starts_with(a)));
        for (lib, _) in libs
            .iter()
            .filter(|l| abi.is_some_and(|a| l.0.starts_with(a)))
        {
            if prog.cancel.load(Ordering::Relaxed) {
                break;
            }
            let Some(data) = st.read_entry(lib) else {
                continue;
            };
            for s in elf::strings(&data, 6) {
                if m.line(&format!("\"{s}\"")) {
                    // line 0: the report lists strings in its own order, so there is no line to jump to
                    hits.push((lib.clone(), 0, s));
                }
            }
        }
    }
    if m.scope.decompiles() {
        let mut list = candidates(st, m).unwrap_or_else(|| (0..st.classes.len()).collect());
        if m.app_only {
            let before = list.len();
            list.retain(|&k| st.origin[k] != Origin::Library);
            hidden += before - list.len();
        }
        hits.extend(scan(st, m, &list, prog));
    }
    // first-party first for the API scopes: app code, then resources / native / other code,
    // then known libraries. Stable, so the (target, line) order holds within each rank.
    if matches!(m.scope, Scope::Urls | Scope::Http | Scope::Secrets) {
        hits.sort_by_key(|h| target_rank(st, &h.0));
    }
    hits.truncate(LIMIT);
    (hits, hidden)
}

/// 0 app code, 1 other code or a resource / native target, 2 known library: the order the
/// API scopes rank their hits by.
fn target_rank(st: &State, t: &str) -> u8 {
    st.find(t).map_or(1, |k| match st.origin[k] {
        Origin::App => 0,
        Origin::Other => 1,
        Origin::Library => 2,
    })
}

/// The source label and rank of a hit's target: code by origin, or "resources" / "native".
fn hit_kind(st: &State, t: &str) -> (&'static str, u8) {
    if let Some(k) = st.find(t) {
        match st.origin[k] {
            Origin::App => ("app code", 0),
            Origin::Other => ("other code", 1),
            Origin::Library => ("library code", 2),
        }
    } else if t.starts_with("lib/") {
        ("native", 1)
    } else {
        ("resources", 1)
    }
}

/// Distinct URL hosts among these hits.
#[cfg(any(test, feature = "gui"))]
pub fn host_count(hits: &[Hit]) -> usize {
    let re = regex::Regex::new(r#"(?:https?|wss?)://([^/?#\s"'<>\\]+)"#).unwrap();
    let mut set = std::collections::BTreeSet::new();
    for (_, _, t) in hits {
        if let Some(c) = re.captures(t) {
            set.insert(c[1].to_string());
        }
    }
    set.len()
}

/// One host's row of the `urls` summary.
#[derive(Default)]
struct HostRow {
    hits: usize,
    by_source: BTreeMap<&'static str, usize>,
    /// referenced from app-owned code
    app: bool,
}

/// One-line summaries for the API scopes, shown above the results in text output: `urls`
/// gets a host table (per-host counts by source, app-referenced hosts first), `http` and
/// `secrets` a source count. CSV/JSON stay machine-shaped.
pub fn search_summary(st: &State, scope: Scope, hits: &[Hit]) -> Vec<String> {
    let mut out = Vec::new();
    if hits.is_empty() {
        return out;
    }
    if scope == Scope::Urls {
        let mut hosts: BTreeMap<String, HostRow> = BTreeMap::new();
        let mut bare = 0usize;
        let re = regex::Regex::new(r#"(?:https?|wss?)://([^/?#\s"'<>\\]+)"#).unwrap();
        for (t, _, text) in hits {
            let (kind, rank) = hit_kind(st, t);
            let Some(c) = re.captures(text) else {
                bare += 1;
                continue;
            };
            let e = hosts.entry(c[1].to_string()).or_default();
            e.hits += 1;
            *e.by_source.entry(kind).or_insert(0) += 1;
            e.app |= rank == 0;
        }
        let mut list: Vec<(String, HostRow)> = hosts.into_iter().collect();
        list.sort_by(|a, b| {
            b.1.app
                .cmp(&a.1.app)
                .then_with(|| b.1.hits.cmp(&a.1.hits))
                .then_with(|| a.0.cmp(&b.0))
        });
        out.push(format!(
            "// hosts ({}), app-referenced first{}",
            list.len(),
            if bare > 0 {
                format!(
                    ", {bare} hit{} without a host",
                    if bare == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            }
        ));
        for (h, row) in &list {
            let parts: Vec<String> = row
                .by_source
                .iter()
                .map(|(k, c)| format!("{k} {c}"))
                .collect();
            out.push(format!("  {h}    {}    {}", row.hits, parts.join(", ")));
        }
    } else if matches!(scope, Scope::Http | Scope::Secrets) {
        let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        for (t, _, _) in hits {
            *counts.entry(hit_kind(st, t).0).or_insert(0) += 1;
        }
        let parts: Vec<String> = counts.iter().map(|(k, c)| format!("{k} {c}")).collect();
        out.push(format!("// hits by source: {}", parts.join(", ")));
    }
    out
}

/// Read `list` (class ids) on all cores and keep matching lines. A scan over every class with
/// no cache yet fills the cache on the way.
fn scan(st: &State, m: &Matcher, list: &[usize], prog: &Progress) -> Vec<Hit> {
    prog.total.store(list.len(), Ordering::Relaxed);
    let reader = st.cache.read().unwrap().clone();
    let writer =
        (reader.is_none() && list.len() == st.classes.len() && CACHE_WRITE.load(Ordering::Relaxed))
            .then(|| cache::Writer::create(&st.cache_dir, st.classes.len()))
            .flatten();
    let next = AtomicUsize::new(0);
    let found = Mutex::new(Vec::new());
    let full = AtomicBool::new(false);
    std::thread::scope(|s| {
        for _ in 0..threads() {
            let _ = std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn_scoped(s, || {
                    let mut f = reader.as_ref().and_then(|r| r.file());
                    let mut mine = Vec::new();
                    loop {
                        if prog.cancel.load(Ordering::Relaxed) || full.load(Ordering::Relaxed) {
                            break;
                        }
                        let n = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&k) = list.get(n) else { break };
                        let text = match (&reader, f.as_mut()) {
                            (Some(r), Some(f)) => st.java_of(k, Some((r, f))),
                            _ => st.java_of(k, None),
                        };
                        if let Some(text) = text {
                            if let Some(w) = &writer {
                                w.put(k, &text);
                            }
                            for (ln, l) in text.lines().enumerate() {
                                if m.line(l) {
                                    mine.push((
                                        st.classes[k].0.clone(),
                                        ln + 1,
                                        l.trim().to_string(),
                                    ));
                                }
                            }
                            // keep scanning to finish the cache, but stop collecting
                            if mine.len() >= LIMIT && writer.is_none() {
                                full.store(true, Ordering::Relaxed);
                            }
                        }
                        prog.done.fetch_add(1, Ordering::Relaxed);
                    }
                    found.lock().unwrap().extend(mine);
                });
        }
    });
    if let Some(w) = writer
        && !prog.cancel.load(Ordering::Relaxed)
        && next.load(Ordering::Relaxed) >= list.len()
        && let Some(r) = w.finish()
    {
        *st.cache.write().unwrap() = Some(Arc::new(r));
    }
    let mut v = found.into_inner().unwrap();
    v.sort();
    v
}
