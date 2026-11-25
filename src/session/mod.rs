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
#[cfg(test)]
pub(crate) use crate::search::xml_entries;
#[allow(unused_imports)]
pub use crate::search::{
    CACHE_WRITE, Const, Format, Hit, INPUTS, LIMIT, Matcher, Origin, Progress, Scope, THREADS,
    api_sheet, attr, csv_field, entropy_bits, entropy_secrets, excerpt, json_str,
    looks_like_secret, native_lib_meaning, origin, prepare, search, search_stats, search_summary,
    smali, string_constants, threads,
};

use std::collections::BTreeMap;
use std::io::Read as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::cache;
use crate::dex::{self, Dex};
use crate::export::Apk;
use crate::index::Index;
use crate::res;

pub struct State {
    pub path: String,
    /// a file holding the APK on its own, made only when a command needs one (build, verify,
    /// decoding resources to a folder); reads go through `apk`, in place inside a bundle
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    file: OnceLock<String>,
    /// shared with inline string decryption (`re::set_dexes`): one parse, one set of tables
    pub dexes: Arc<Vec<Dex<'static>>>,
    /// top-level classes: (java name, dex, class index); the position is the class id
    pub classes: Vec<(String, usize, usize)>,
    /// the decoded manifest; the rest of the resources wait in `res` until something asks
    manifest: Option<String>,
    /// (res path -> text of the values XML and the manifest, res path -> path inside the APK),
    /// decoded from the resource table the first time a command reads resources
    res: OnceLock<Resources>,
    names: Option<res::Names>,
    pub(crate) apk: Mutex<Apk<crate::input::Input>>,
    /// every zip entry name, for the overview
    pub entries: Vec<String>,
    pub package: String,
    /// methods with a body or not, over every class; counted when first asked for
    methods: OnceLock<usize>,
    /// origin of each top-level class (same order as `classes`)
    pub origin: Vec<Origin>,
    natives: OnceLock<Vec<(String, String)>>,
    pub index: OnceLock<Index>,
    pub indexed: AtomicUsize,
    pub(crate) cache: RwLock<Option<Arc<cache::Reader>>>,
    pub(crate) cache_dir: std::path::PathBuf,
}

/// Decoded resources: res path -> text, and res path -> path inside the APK.
type Resources = (BTreeMap<String, String>, BTreeMap<String, String>);

impl State {
    /// The APK as a file of its own, for the commands that write or verify it: the input itself,
    /// or for a bundle a copy of its base split (made once, then kept by `search::prepare`).
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn file(&self) -> String {
        self.file
            .get_or_init(|| prepare(&self.path).unwrap_or_else(|_| self.path.clone()))
            .clone()
    }

    /// The APK's bytes again, in place: for readers that need their own handle.
    pub fn input(&self) -> Result<crate::input::Input, String> {
        crate::input::Input::open(&self.path)
    }

    /// Every decoded text resource by res path: the manifest and the values XML of every
    /// configuration. Decoded from the table on first use, so commands that never read resources
    /// (smali, classes, decompile, …) never pay for 80 languages of strings.
    /// How many methods the APK defines (every class's class_data; only `info` asks).
    pub fn methods(&self) -> usize {
        *self.methods.get_or_init(|| {
            self.dexes
                .iter()
                .map(|d| {
                    (0..d.class_count)
                        .map(|c| d.class_data(c).map_or(0, |x| x.methods.len()))
                        .sum::<usize>()
                })
                .sum()
        })
    }

    pub fn texts(&self) -> &BTreeMap<String, String> {
        &self.resources().0
    }

    /// Res path -> path inside the APK of every file resource (layouts, drawables, …).
    pub fn files(&self) -> &BTreeMap<String, String> {
        &self.resources().1
    }

    fn resources(&self) -> &Resources {
        self.res.get_or_init(|| {
            let mut texts = BTreeMap::new();
            let mut files = BTreeMap::new();
            if let Some(m) = &self.manifest {
                texts.insert("AndroidManifest.xml".to_string(), m.clone());
            }
            let table = self
                .apk
                .lock()
                .ok()
                .and_then(|mut a| a.read("resources.arsc"))
                .and_then(|d| apkscope_core::table::read(&d).ok());
            if let (Some(t), Some(n)) = (&table, &self.names) {
                let dec = res::decode_table(t, n);
                for (p, text) in dec.values {
                    texts.insert(format!("res/{p}"), text);
                }
                for (p, src) in dec.files {
                    files.insert(format!("res/{p}"), src);
                }
            }
            (texts, files)
        })
    }
}

/// What the heavy reads of `load` produce: the parsed DEX files in load order, the
/// resource names and the decoded manifest.
type Loaded = (Vec<Dex<'static>>, Option<res::Names>, Option<String>);

/// The heavy reads of `load`, split so the independent ones run at once: the DEX files
/// inflate, parse and pre-build their nesting tables on all cores — one zip handle per
/// worker over a reopened input, pulling entry names from one shared queue, results
/// kept in `Apk::dex`'s load order — while the resource table and the manifest are read
/// through the session's own handle on a thread beside them. A session lives until the
/// process exits, so the DEX bytes the workers inflate are `'static`, as the serial
/// read's were.
///
/// Every DEX in every position is still read — the queue covers the whole list — a DEX
/// that does not parse is skipped exactly as the serial read skipped it, and when a
/// worker's read fails, or no reopen opens an archive, the serial path takes over and
/// returns the bytes and the error `Apk::dex` itself would have.
fn load_dexes(
    path: &str,
    names: &[String],
    apk: &mut Apk<crate::input::Input>,
) -> Result<Loaded, String> {
    /// The DEX files, read through the session's own handle: the exact reads this
    /// replaces, byte for byte, error for error.
    fn serial(apk: &mut Apk<crate::input::Input>) -> Result<Vec<Dex<'static>>, String> {
        Ok(apk
            .dex()?
            .into_iter()
            .filter_map(|(_, b)| Dex::parse(&*Box::leak(b.into_boxed_slice())).ok())
            .collect())
    }
    let threads = crate::search::threads().min(names.len()).max(1);
    let inputs: Vec<crate::input::Input> = (0..threads)
        .filter_map(|_| crate::input::Input::open(path).ok())
        .collect();
    if inputs.is_empty() {
        let (names, manifest) = read_side(apk);
        return serial(apk).map(|dexes| (dexes, names, manifest));
    }
    let next = &AtomicUsize::new(0);
    let failed = &AtomicBool::new(false);
    let (side, slots, live) = std::thread::scope(|s| {
        // the resource table and the manifest, through the handle the workers do not use
        let side = s.spawn(|| read_side(apk));
        let mut handles = Vec::new();
        for input in inputs {
            let Ok(mut zip) = zip::ZipArchive::new(input) else {
                continue;
            };
            handles.push(s.spawn(move || {
                // one queue item per DEX: its load-order slot and its parse, `None` when
                // the file did not parse (skipped, as the serial read skips it)
                let mut out: Vec<(usize, Option<Dex<'static>>)> = Vec::new();
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(name) = names.get(k) else {
                        break;
                    };
                    match zip.by_name(name) {
                        Ok(mut e) => {
                            let mut buf = Vec::with_capacity(e.size() as usize);
                            if e.read_to_end(&mut buf).is_ok() {
                                let raw: &'static [u8] = Box::leak(buf.into_boxed_slice());
                                let dex = Dex::parse(raw).ok();
                                // build the nesting table here, on this core: the import
                                // paths and the class list below read it from every DEX
                                if let Some(d) = &dex {
                                    d.nesting();
                                }
                                out.push((k, dex));
                            } else {
                                failed.store(true, Ordering::Relaxed);
                            }
                        }
                        Err(_) => {
                            failed.store(true, Ordering::Relaxed);
                        }
                    }
                }
                out
            }));
        }
        // collect in load order: slot k holds the parse of entry k
        let live = handles.is_empty();
        let mut slots: Vec<Option<Dex<'static>>> = names.iter().map(|_| None).collect();
        for h in handles {
            for (k, dex) in h.join().expect("dex worker") {
                slots[k] = dex;
            }
        }
        let side = side.join().expect("resource side read");
        (side, slots, live)
    });
    if failed.load(Ordering::Relaxed) || live {
        return serial(apk).map(|dexes| (dexes, side.0, side.1));
    }
    Ok((slots.into_iter().flatten().collect(), side.0, side.1))
}

/// The resource table and the manifest, the reads `load` did after the DEX files: the
/// table parsed for `Names` and dropped (only the names are kept — the values wait in
/// `res` until something asks), the manifest decoded with those names.
fn read_side(apk: &mut Apk<crate::input::Input>) -> (Option<res::Names>, Option<String>) {
    let table = apk
        .read("resources.arsc")
        .and_then(|d| apkscope_core::table::read(&d).ok());
    let names = table.as_ref().map(res::Names::new);
    drop(table);
    let manifest = apk
        .read("AndroidManifest.xml")
        .and_then(|m| res::decode_xml(&m, names.as_ref()).ok());
    (names, manifest)
}

pub fn load(path: &str, deobf: bool) -> Result<State, String> {
    let mut apk = Apk::new(crate::input::Input::open(path)?)?;
    let (dexes, names, manifest) = load_dexes(path, &apk.dex_names(), &mut apk)?;
    if deobf {
        crate::deobf::init(&dexes);
    }
    let notes = crate::re::load_notes(path)?;
    let tag = crate::re::notes_tag(&notes);
    crate::deobf::set_notes(notes);
    // inline string decryption needs every DEX, from any thread: the same parsed files
    let dexes = Arc::new(dexes);
    crate::re::set_dexes(dexes.clone());
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
    // the manifest names the package; the values text is decoded later, when asked for
    let package = manifest
        .as_deref()
        .and_then(|t| attr(t, "package"))
        .unwrap_or_default();
    let origin = classes.iter().map(|c| origin(&c.0, &package)).collect();
    let cache_dir = cache::dir_for(path, deobf, tag);
    let reader = cache::Reader::open(&cache_dir, classes.len()).map(Arc::new);
    cache::prune(&cache_dir);
    Ok(State {
        path: path.to_string(),
        file: OnceLock::new(),
        dexes,
        classes,
        manifest,
        res: OnceLock::new(),
        names,
        apk: Mutex::new(apk),
        entries,
        package,
        methods: OnceLock::new(),
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
