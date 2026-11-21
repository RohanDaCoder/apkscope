//! One opened APK: the DEX files, the reference index, the decompiled-code cache and
//! the decoded resources. `load` builds it; the accessors live in `state`, the
//! viewer-only surface in `view`, native libraries in `native`, the manifest overview
//! in `manifest` and hit export in `export`. Shared by the viewer and the `search`
//! command.

mod export;
mod manifest;
mod native;
mod state;
#[cfg(feature = "gui")]
mod view;

// Bin crate: the facade re-exports below are consumed through `crate::session::…`
// paths by the viewer build and the test mirror, not internally — so in the
// no-gui, non-test configuration some are legitimately unreferenced.
#[allow(unused_imports)]
pub use self::export::{export, json_extend};
#[allow(unused_imports)]
pub use self::manifest::{overview, sdk_share};
#[cfg(feature = "gui")]
pub use self::view::memory_used;
#[cfg(any(test, feature = "gui"))]
pub use crate::search::host_count;

// The query machinery lives in `crate::search` (the seam both query paths share; the
// stateless path switched to it in the same step). These re-exports keep every
// `crate::session::…` path the CLI, MCP server and viewer use resolving, so no `use`
// line outside the two folders changes in this PR.
pub(crate) use crate::search::SECRET_RE;
#[allow(unused_imports)]
pub use crate::search::{
    CACHE_WRITE, Const, Format, Hit, INPUTS, LIMIT, Matcher, Origin, Progress, Scope, THREADS,
    api_sheet, attr, csv_field, entropy_bits, entropy_secrets, excerpt, json_str,
    looks_like_secret, native_lib_meaning, origin, prepare, search, search_stats, search_summary,
    smali, string_constants, threads,
};

use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::cache;
use crate::dex::{self, Dex};
use crate::export::Apk;
use crate::index::Index;
use crate::res;

pub struct State {
    pub path: String,
    /// the APK actually read (differs from `path` for bundles and .dex)
    pub file: String,
    pub dexes: Vec<Dex<'static>>,
    /// top-level classes: (java name, dex, class index); the position is the class id
    pub classes: Vec<(String, usize, usize)>,
    /// res path -> text (manifest, values XML) decoded at startup
    pub texts: BTreeMap<String, String>,
    /// res path -> path inside the APK, decoded on demand
    pub files: BTreeMap<String, String>,
    names: Option<res::Names>,
    apk: Mutex<Apk>,
    /// every zip entry name, for the overview
    pub entries: Vec<String>,
    pub package: String,
    pub methods: usize,
    /// origin of each top-level class (same order as `classes`)
    pub origin: Vec<Origin>,
    natives: OnceLock<Vec<(String, String)>>,
    pub index: OnceLock<Index>,
    pub indexed: AtomicUsize,
    pub(crate) cache: RwLock<Option<Arc<cache::Reader>>>,
    pub(crate) cache_dir: std::path::PathBuf,
}

pub fn load(path: &str, deobf: bool) -> Result<State, String> {
    let real = prepare(path)?;
    let mut apk = crate::export::open(&real)?;
    // a session lives until the process exits, so the DEX bytes can be 'static
    let raw: Vec<&'static [u8]> = std::mem::take(&mut apk.dex)
        .into_iter()
        .map(|(_, b)| &*Box::leak(b.into_boxed_slice()))
        .collect();
    let dexes: Vec<Dex<'static>> = raw.iter().filter_map(|b| Dex::parse(b).ok()).collect();
    if deobf {
        crate::deobf::init(&dexes);
    }
    let notes = crate::re::load_notes(path)?;
    let tag = crate::re::notes_tag(&notes);
    crate::deobf::set_notes(notes);
    // inline string decryption needs every DEX, from any thread
    crate::re::set_dexes(Box::leak(
        raw.iter()
            .filter_map(|b| Dex::parse(b).ok())
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    ));
    crate::imports::init(&dexes);
    let mut classes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (d, dx) in dexes.iter().enumerate() {
        let nest = dx.nesting();
        for (c, parent) in nest.parent.iter().enumerate() {
            if parent.is_none()
                && let Ok(desc) = dx.class_descriptor(c)
                && seen.insert(desc.clone())
            {
                classes.push((dex::java_name(&desc), d, c));
            }
        }
    }
    classes.sort();
    let entries: Vec<String> = apk
        .zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .collect();
    let table = apk
        .read("resources.arsc")
        .and_then(|d| apkscope_core::table::read(&d).ok());
    let names = table.as_ref().map(res::Names::new);
    let mut texts = BTreeMap::new();
    let mut files = BTreeMap::new();
    let mut package = String::new();
    if let Some(m) = apk.read("AndroidManifest.xml")
        && let Ok(t) = res::decode_xml(&m, names.as_ref())
    {
        package = attr(&t, "package").unwrap_or_default();
        texts.insert("AndroidManifest.xml".to_string(), t);
    }
    if let (Some(t), Some(n)) = (&table, &names) {
        let dec = res::decode_table(t, n);
        for (p, text) in dec.values {
            texts.insert(format!("res/{p}"), text);
        }
        for (p, src) in dec.files {
            files.insert(format!("res/{p}"), src);
        }
    }
    let methods = dexes
        .iter()
        .map(|d| {
            (0..d.class_count)
                .map(|c| d.class_data(c).map_or(0, |x| x.methods.len()))
                .sum::<usize>()
        })
        .sum();
    let origin = classes.iter().map(|c| origin(&c.0, &package)).collect();
    let cache_dir = cache::dir_for(&real, deobf, tag);
    let reader = cache::Reader::open(&cache_dir, classes.len()).map(Arc::new);
    Ok(State {
        path: path.to_string(),
        file: real.clone(),
        dexes,
        classes,
        texts,
        files,
        names,
        apk: Mutex::new(apk),
        entries,
        package,
        methods,
        origin,
        natives: OnceLock::new(),
        index: OnceLock::new(),
        indexed: AtomicUsize::new(0),
        cache: RwLock::new(reader),
        cache_dir,
    })
}

#[cfg(test)]
#[path = "../../tests/apkscope/session.rs"]
mod tests;
