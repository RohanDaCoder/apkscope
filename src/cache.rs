//! On-disk cache of decompiled classes, one per APK: `code.bin` holds deflated Java text,
//! `code.idx` the (offset, length) of each top-level class. The index is written last, so a
//! cache without it (interrupted fill) is ignored and rebuilt.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `%LOCALAPPDATA%\apkscope\cache` (or `$XDG_CACHE_HOME/apkscope`, `~/.cache/apkscope`)
pub fn root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("apkscope").join("cache")
}

/// Cache folder for one APK: changes when the file, the deobfuscation choice or apkscope changes.
/// `tag` = hash of the user's renames and comments (0 = none).
pub fn dir_for(apk: &str, deobf: bool, tag: u64) -> PathBuf {
    let meta = fs::metadata(apk).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let name = Path::new(apk)
        .file_stem()
        .map_or("apk".into(), |s| s.to_string_lossy().into_owned());
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    root().join(format!(
        "{safe}-{size:x}-{mtime:x}-{}-{}{}",
        if deobf { "d" } else { "n" },
        crate::upgrade::identity().version,
        if tag == 0 {
            String::new()
        } else {
            format!("-u{:x}", tag & 0xffff_ffff)
        }
    ))
}

/// How long an unused cache or bundle copy is kept.
const UNUSED: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

/// The apkscope version a cache folder was made by, from its name
/// (`<name>-<size>-<mtime>-<d|n>-<version>[-u<tag>]`).
pub fn version_of(dir_name: &str) -> Option<&str> {
    let at = dir_name.rfind("-d-").or_else(|| dir_name.rfind("-n-"))?;
    let rest = &dir_name[at + 3..];
    Some(rest.split("-u").next().unwrap_or(rest))
}

/// Drop what no run will read again: cache folders made by another apkscope version (the
/// version is in the folder name, so they can never match), and cache folders, bundle copies
/// and leftover `.part` files nobody has used for 30 days. `keep` is the folder this run uses;
/// it is marked used. Runs once per load, in the background; a deletion that fails stays for
/// the next run.
pub fn prune(keep: &Path) {
    let keep = keep.to_path_buf();
    // mark this run's cache used: its index file's mtime is what the age rule reads
    if let Ok(f) = File::options().write(true).open(keep.join("code.idx")) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
    std::thread::spawn(move || {
        let now = std::time::SystemTime::now();
        let old = |p: &Path, marker: &Path| {
            fs::metadata(marker)
                .or_else(|_| fs::metadata(p))
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).is_ok_and(|d| d > UNUSED))
        };
        let me = crate::upgrade::identity().version;
        let Ok(rd) = fs::read_dir(root()) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p == keep || !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name == "inputs" {
                // bundle and .dex copies: by age; a cut-off `.part` after a day
                for f in fs::read_dir(&p).into_iter().flatten().flatten() {
                    let fp = f.path();
                    let part = fp.extension().is_some_and(|x| x == "part");
                    let day = std::time::Duration::from_secs(24 * 3600);
                    let stale = fs::metadata(&fp)
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| {
                            now.duration_since(t)
                                .is_ok_and(|d| d > if part { day } else { UNUSED })
                        });
                    if stale {
                        let _ = fs::remove_file(&fp);
                    }
                }
                continue;
            }
            let other_version = version_of(&name).is_some_and(|v| v != me);
            if other_version || old(&p, &p.join("code.idx")) {
                let _ = fs::remove_dir_all(&p);
            }
        }
    });
}

/// Total size of every cache, in bytes.
#[cfg(feature = "gui")]
pub fn total_size() -> u64 {
    fn walk(p: &Path) -> u64 {
        fs::read_dir(p).map_or(0, |rd| {
            rd.flatten()
                .map(|e| {
                    if e.path().is_dir() {
                        walk(&e.path())
                    } else {
                        e.metadata().map_or(0, |m| m.len())
                    }
                })
                .sum()
        })
    }
    walk(&root())
}

#[cfg(feature = "gui")]
pub fn clear_all() -> std::io::Result<()> {
    match fs::remove_dir_all(root()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

pub struct Reader {
    bin: PathBuf,
    slots: Vec<(u64, u32)>,
}

impl Reader {
    pub fn open(dir: &Path, classes: usize) -> Option<Reader> {
        let mut raw = Vec::new();
        File::open(dir.join("code.idx"))
            .ok()?
            .read_to_end(&mut raw)
            .ok()?;
        if raw.len() != classes * 12 {
            return None;
        }
        let slots = raw
            .as_chunks::<12>()
            .0
            .iter()
            .map(|c| {
                (
                    u64::from_le_bytes(c[..8].try_into().unwrap()),
                    u32::from_le_bytes(c[8..].try_into().unwrap()),
                )
            })
            .collect();
        Some(Reader {
            bin: dir.join("code.bin"),
            slots,
        })
    }

    /// A handle for one reading thread.
    pub fn file(&self) -> Option<File> {
        File::open(&self.bin).ok()
    }

    pub fn get(&self, f: &mut File, k: usize) -> Option<String> {
        let &(off, len) = self.slots.get(k)?;
        if len == 0 {
            return None;
        }
        let mut buf = vec![0; len as usize];
        f.seek(SeekFrom::Start(off)).ok()?;
        f.read_exact(&mut buf).ok()?;
        String::from_utf8(miniz_oxide::inflate::decompress_to_vec(&buf).ok()?).ok()
    }
}

/// Filled by the threads of a full scan; `finish` makes it readable.
pub struct Writer {
    dir: PathBuf,
    out: Mutex<(File, u64)>,
    slots: Mutex<Vec<(u64, u32)>>,
}

impl Writer {
    pub fn create(dir: &Path, classes: usize) -> Option<Writer> {
        fs::create_dir_all(dir).ok()?;
        let _ = fs::remove_file(dir.join("code.idx"));
        let f = File::create(dir.join("code.bin")).ok()?;
        Some(Writer {
            dir: dir.to_path_buf(),
            out: Mutex::new((f, 0)),
            slots: Mutex::new(vec![(0, 0); classes]),
        })
    }

    pub fn put(&self, k: usize, text: &str) {
        // level 1: compression must not slow the scan down
        let z = miniz_oxide::deflate::compress_to_vec(text.as_bytes(), 1);
        let off = {
            let mut o = self.out.lock().unwrap();
            let off = o.1;
            if o.0.write_all(&z).is_err() {
                return;
            }
            o.1 += z.len() as u64;
            off
        };
        if let Some(s) = self.slots.lock().unwrap().get_mut(k) {
            *s = (off, z.len() as u32);
        }
    }

    /// Write the index; only a complete scan should call this.
    pub fn finish(self) -> Option<Reader> {
        let (mut f, _) = self.out.into_inner().ok()?;
        f.flush().ok()?;
        let slots = self.slots.into_inner().ok()?;
        let mut raw = Vec::with_capacity(slots.len() * 12);
        for &(o, l) in &slots {
            raw.extend_from_slice(&o.to_le_bytes());
            raw.extend_from_slice(&l.to_le_bytes());
        }
        fs::write(self.dir.join("code.idx"), raw).ok()?;
        Reader::open(&self.dir, slots.len())
    }
}

#[cfg(test)]
#[path = "../tests/apkscope/cache.rs"]
mod tests;
