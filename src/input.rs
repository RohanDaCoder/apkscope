//! The APK a command reads, without copying it. A plain `.apk`/`.jar` is the file itself; the
//! `base.apk` of a `.xapk`/`.apks`/`.apkm` bundle is read where it sits inside the bundle when the
//! bundle stores it uncompressed (bundle tools do). Only a deflated base split, or a bare `.dex`,
//! needs a copy on disk, which `search::prepare` makes and keeps.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A seekable, read-only view of `base..base + len` of a file: positions are relative to `base`
/// and clamped to the range, so it reads like the file it would have been copied to.
pub struct Input {
    f: File,
    path: PathBuf,
    base: u64,
    len: u64,
    pos: u64,
}

impl Input {
    /// The bytes `base..base + len` of `f` (opened from `path`).
    pub fn region(mut f: File, path: &Path, base: u64, len: u64) -> std::io::Result<Input> {
        f.seek(SeekFrom::Start(base))?;
        Ok(Input {
            f,
            path: path.to_path_buf(),
            base,
            len,
            pos: 0,
        })
    }

    fn whole(path: &str) -> Result<Input, String> {
        let f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let len = f.metadata().map_err(|e| format!("{path}: {e}"))?.len();
        Input::region(f, Path::new(path), 0, len).map_err(|e| format!("{path}: {e}"))
    }

    /// The APK of `path`, as `search::prepare` would resolve it, but in place when it can be.
    pub fn open(path: &str) -> Result<Input, String> {
        let ext = Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if matches!(ext.as_str(), "apks" | "xapk" | "apkm")
            && let Some(i) = stored_base(path)?
        {
            return Ok(i);
        }
        Input::whole(&crate::search::prepare(path)?)
    }

    /// Another handle on the same bytes, for another thread.
    pub fn reopen(&self) -> std::io::Result<Input> {
        Input::region(File::open(&self.path)?, &self.path, self.base, self.len)
    }

    pub fn len(&self) -> u64 {
        self.len
    }
}

/// The bundle's base split as a region, when it is stored; `None` when it is deflated.
fn stored_base(path: &str) -> Result<Option<Input>, String> {
    let f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{path}: {e}"))?;
    let names: Vec<String> = zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .filter(|n| n.ends_with(".apk"))
        .collect();
    // the base split holds the code; fall back to the largest APK inside (as `prepare` does)
    let base = names
        .iter()
        .find(|n| n.rsplit('/').next() == Some("base.apk"))
        .cloned()
        .or_else(|| {
            names
                .iter()
                .max_by_key(|n| zip.by_name(n).map(|e| e.size()).unwrap_or(0))
                .cloned()
        })
        .ok_or("no APK inside this bundle")?;
    let e = zip.by_name(&base).map_err(|e| e.to_string())?;
    if e.compression() != zip::CompressionMethod::Stored {
        return Ok(None);
    }
    let (Some(start), len) = (e.data_start(), e.compressed_size()) else {
        return Ok(None);
    };
    drop(e);
    let f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    Input::region(f, Path::new(path), start, len)
        .map(Some)
        .map_err(|e| format!("{path}: {e}"))
}

impl Read for Input {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let max = (self.len - self.pos).min(b.len() as u64) as usize;
        if max == 0 {
            return Ok(0);
        }
        let k = self.f.read(&mut b[..max])?;
        self.pos += k as u64;
        Ok(k)
    }
}

impl Seek for Input {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        let to = match p {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
        }
        .clamp(0, self.len as i128) as u64;
        self.f.seek(SeekFrom::Start(self.base + to))?;
        self.pos = to;
        Ok(to)
    }
}

#[cfg(test)]
#[path = "../tests/apkscope/input.rs"]
mod tests;
