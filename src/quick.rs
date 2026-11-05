//! Stateless queries straight from the APK: DEX files are inflated in parallel, nothing is
//! indexed or cached, and only the classes a query names get decompiled. This is the fast path
//! for agents and scripts; `session` is the indexed, cached path for repeated searches.

use std::fmt::Write as _;
use std::io::Read;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::bytecode::{self, Ref};
use crate::dex::{self, Dex};
use crate::{java, session};

/// Run `f(0..n)` on all cores, results in index order.
pub fn par<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..session::threads().min(n).max(1) {
            let _ = std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(s, || {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    if k >= n {
                        break;
                    }
                    let v = f(k);
                    out.lock().unwrap()[k] = Some(v);
                }
            });
        }
    });
    out.into_inner().unwrap().into_iter().map(|v| v.expect("worker panicked")).collect()
}

/// Run `f(k)` on all cores, starting the `k`s in `order`; `sink(k, result)` runs on the calling
/// thread in index order as soon as each result (and all before it) is in, so output can be
/// written while later work still runs.
fn par_sink<T: Send>(order: &[usize], f: impl Fn(usize) -> T + Sync, mut sink: impl FnMut(usize, T)) {
    let n = order.len();
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<std::thread::Result<T>>>> = Mutex::new((0..n).map(|_| None).collect());
    let ready = std::sync::Condvar::new();
    std::thread::scope(|s| {
        for _ in 0..session::threads().min(n).max(1) {
            let _ = std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(s, || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                // a panic is handed to the caller instead of leaving it waiting for this slot
                let v = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(order[i])));
                slots.lock().unwrap()[order[i]] = Some(v);
                ready.notify_all();
            });
        }
        for k in 0..n {
            let mut g = slots.lock().unwrap();
            let v = loop {
                match g[k].take() {
                    Some(v) => break v,
                    None => g = ready.wait(g).unwrap(),
                }
            };
            drop(g);
            match v {
                Ok(v) => sink(k, v),
                Err(p) => std::panic::resume_unwind(p),
            }
        }
    });
}

/// One DEX file, inflated only as far as a query needs: names and the string pool sit in the
/// first third of a d8-built DEX, code and class data after them.
pub struct Lazy {
    pub name: String,
    /// compressed bytes, only the first `got` read yet; the rest waits in `file` for an inflate
    raw: Vec<u8>,
    got: usize,
    file: Option<std::fs::File>,
    /// full-size buffer; only `have` bytes are real yet (the rest is left over from another DEX)
    pub buf: Vec<u8>,
    have: usize,
    deflated: bool,
    /// more DEX files than threads: skipping a file's tail shortens the whole scan; otherwise
    /// every file inflates at once anyway and a prefix pass would only add work
    pub prefix_pays: bool,
}

impl Lazy {
    /// Inflate at least the first `n` bytes. libdeflate can't resume, but when its output
    /// buffer is short it stops with valid output except for at most one match's worth (258
    /// bytes) at the end, so a prefix is a fresh, fast run into a slightly longer buffer.
    pub fn upto(&mut self, n: usize) -> Result<(), String> {
        const SLACK: usize = 1 << 12;
        let n = n.min(self.buf.len());
        if n <= self.have || !self.deflated {
            return Ok(());
        }
        let mut d = libdeflater::Decompressor::new();
        if n + SLACK >= self.buf.len() {
            self.read_raw(usize::MAX)?;
            let k = d.deflate_decompress(&self.raw[..self.got], &mut self.buf).map_err(|e| format!("{}: inflate failed: {e}", self.name))?;
            self.buf.truncate(k);
            self.have = k;
            return Ok(());
        }
        // the entry's own ratio with half again as margin (DEX packs ~3:1, so a fixed 9/8 read
        // and faulted in ~3x too much); a short read fails the run, which then retries with all
        let ratio = self.raw.len() as f64 / self.buf.len().max(1) as f64;
        self.read_raw(((n + SLACK) as f64 * ratio * 1.5) as usize + (1 << 16))?;
        loop {
            match d.deflate_decompress(&self.raw[..self.got], &mut self.buf[..n + SLACK]) {
                Ok(k) => self.have = k,
                Err(libdeflater::DecompressionError::InsufficientSpace) => self.have = n,
                Err(_) if self.got < self.raw.len() => {
                    self.read_raw(usize::MAX)?;
                    continue;
                }
                Err(e) => return Err(format!("{}: inflate failed: {e}", self.name)),
            }
            return Ok(());
        }
    }

    /// Have at least the first `n` compressed bytes (or all of them) in `raw`.
    fn read_raw(&mut self, n: usize) -> Result<(), String> {
        let n = n.min(self.raw.len());
        if let (Some(f), true) = (&mut self.file, self.got < n) {
            f.read_exact(&mut self.raw[self.got..n]).map_err(|e| format!("{}: {e}", self.name))?;
            self.got = n;
        }
        Ok(())
    }

    pub fn all(&mut self) -> Result<(), String> {
        self.upto(usize::MAX)
    }

    /// Header, id tables and every string: enough for class, member and string names.
    pub fn names(&mut self) -> Result<(), String> {
        self.names_or_all(1.0)
    }

    /// `names`, or the whole file at once when the strings end past fraction `miss` of it: for
    /// a caller that then needs everything with chance 1 - miss, a prefix p costs p + (1 - miss)
    /// on average (libdeflate restarts from byte 0) against 1 for one full run.
    pub fn names_or_all(&mut self, miss: f64) -> Result<(), String> {
        self.upto(0x70)?;
        // id tables (types, protos, fields, methods, classes) end before the strings' data
        let tables = [(0x40, 4), (0x48, 12), (0x50, 8), (0x58, 8), (0x60, 32), (0x38, 4)];
        let end = tables.iter().map(|&(h, size)| self.word(h + 4) + self.word(h) * size).max().unwrap_or(0);
        let (sn, so) = (self.word(0x38), self.word(0x3C));
        // only the string ids for now: they say where the last string is
        self.upto(so + sn * 4)?;
        let last = (0..sn).map(|i| self.word(so + i * 4)).max().unwrap_or(0).max(end);
        if last as f64 > miss * self.buf.len() as f64 {
            return self.all();
        }
        self.grow_past(last)
    }

    /// `class_names` for a caller that will want the whole file if a class is there. A prefix of
    /// fraction p and then a fresh full run (libdeflate can't resume) cost 1 + p; d8 / R8 put
    /// string data last, so p is often 0.7-0.9 and inflating everything at once is cheaper.
    /// Redex-built files keep strings early (p ~ 0.25) and stay on the prefix.
    pub fn class_names_or_all(&mut self) -> Result<(), String> {
        if self.class_names_fraction()? > 0.5 {
            return self.all();
        }
        self.class_names()
    }

    /// How far into the file the class descriptors reach (0..1), from the header and type ids
    /// alone (a few percent of the file).
    pub fn class_names_fraction(&mut self) -> Result<f64, String> {
        self.upto(0x70)?;
        let (so, tn, to) = (self.word(0x3C), self.word(0x40), self.word(0x44));
        self.upto(to + tn * 4)?;
        let bound = self.word(so + self.word(to + tn.saturating_sub(1) * 4) * 4);
        Ok(bound as f64 / self.buf.len().max(1) as f64)
    }

    /// Like `names`, but only as far as the class descriptors go: sorted by content, they
    /// end long before the lowercase identifiers that fill the rest of the string pool.
    pub fn class_names(&mut self) -> Result<(), String> {
        // unlike `names`, this pays even with few DEX files: class descriptors sit early in the
        // string pool, so most of each file is never inflated (or touched in memory)
        self.upto(0x70)?;
        let (so, tn, to, cn, co) = (self.word(0x3C), self.word(0x40), self.word(0x44), self.word(0x60), self.word(0x64));
        // string and type ids come first and are small; type ids are sorted by string, so the
        // last type's string ends at or after every class descriptor. One inflate then covers
        // the class defs and those strings, instead of a second run from byte 0.
        self.upto(to + tn * 4)?;
        let bound = self.word(so + self.word(to + tn.saturating_sub(1) * 4) * 4);
        self.upto((co + cn * 32).max(bound + (1 << 16)))?;
        let last = (0..cn).map(|c| self.word(so + self.word(to + self.word(co + c * 32) * 4) * 4)).max().unwrap_or(0);
        self.grow_past(last)
    }

    /// The u32 at `o` among the inflated bytes, 0 past them.
    fn word(&self, o: usize) -> usize {
        self.buf[..self.have].get(o..o + 4).map_or(0, |x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize)
    }

    /// Inflate until the string starting at `last` is complete (its NUL is in).
    fn grow_past(&mut self, last: usize) -> Result<(), String> {
        let mut want = last + (1 << 16);
        loop {
            self.upto(want)?;
            if self.have >= self.buf.len() || self.buf.get(last..self.have).is_some_and(|t| t.iter().skip(5).any(|&b| b == 0)) {
                return Ok(());
            }
            want *= 2;
        }
    }

    pub fn dex(&self) -> Option<Dex<'_>> {
        Dex::parse(&self.buf[..self.have]).ok()
    }
}

// Each worker thread keeps its last buffers: their pages are already mapped, and with every
// thread inflating at once, faulting in fresh pages costs more than the inflate itself.
thread_local! {
    static SPARE: std::cell::Cell<(Vec<u8>, Vec<u8>)> = Default::default();
}

impl Drop for Lazy {
    fn drop(&mut self) {
        if self.deflated {
            SPARE.with(|s| s.set((std::mem::take(&mut self.raw), std::mem::take(&mut self.buf))));
        }
    }
}

/// `n` bytes, from `spare` when it is long enough; its old contents stay.
fn reuse(mut spare: Vec<u8>, n: usize) -> Vec<u8> {
    if spare.len() < n {
        return vec![0; n];
    }
    spare.truncate(n);
    spare
}

/// Root-level classes*.dex of an APK (or bundle / bare .dex) in runtime order (classes.dex,
/// classes2.dex, ...), compressed bytes read in parallel; `f` decides how much of each to inflate.
pub fn scan<T: Send>(path: &str, f: impl Fn(&mut Lazy) -> Result<T, String> + Sync) -> Result<Vec<T>, String> {
    let mut all = Vec::new();
    scan_each(path, false, |_| true, f, |r| all.push(r))?;
    all.into_iter().collect()
}

/// `scan`, handing each file's result to `sink` in runtime order as soon as it is in.
/// `stream`: start files in runtime order, so the first results come early (output written
/// while the rest is still worked on); otherwise biggest first, so the whole scan ends sooner.
/// Only the DEX files whose name passes `keep` are read.
pub fn scan_each<T: Send>(path: &str, stream: bool, keep: impl Fn(&str) -> bool, f: impl Fn(&mut Lazy) -> Result<T, String> + Sync, sink: impl FnMut(Result<T, String>)) -> Result<(), String> {
    use std::io::{Seek, SeekFrom};
    let mut sink = sink;
    let real = session::prepare(path)?;
    let mut file = std::fs::File::open(&real).map_err(|e| format!("{path}: {e}"))?;
    // root-level classes*.dex only
    let dex = |n: &[u8]| n.starts_with(b"classes") && n.ends_with(b".dex") && !n.contains(&b'/');
    let mut entries = central_dir(&mut file, &dex).map_err(|e| format!("{path}: {e}"))?;
    entries.retain(|e| keep(&e.0));
    entries.sort_by_key(|e| e.0.trim_start_matches("classes").trim_end_matches(".dex").parse::<u32>().unwrap_or(1));
    let prefix_pays = entries.len() > session::threads();
    let mut order: Vec<usize> = (0..entries.len()).collect();
    if !stream {
        // biggest files start first, so no thread is left with a big one at the end
        order.sort_by_key(|&k| std::cmp::Reverse(entries[k].3));
    }
    par_sink(&order, |k| -> Result<T, String> {
        let (name, local, csize, size, method) = &entries[k];
        let err = |e: std::io::Error| format!("{name}: {e}");
        let mut fh = std::fs::File::open(&real).map_err(err)?;
        let mut h = [0u8; 30];
        fh.seek(SeekFrom::Start(*local)).map_err(err)?;
        fh.read_exact(&mut h).map_err(err)?;
        if h[..4] != [0x50, 0x4b, 3, 4] {
            return Err(format!("{name}: bad local header"));
        }
        let skip = u16::from_le_bytes([h[26], h[27]]) as i64 + u16::from_le_bytes([h[28], h[29]]) as i64;
        fh.seek(SeekFrom::Current(skip)).map_err(err)?;
        let mut lazy = match method {
            0 => {
                let mut raw = vec![0; *csize];
                fh.read_exact(&mut raw).map_err(err)?;
                Lazy { name: name.clone(), have: raw.len(), buf: raw, raw: Vec::new(), got: 0, file: None, deflated: false, prefix_pays }
            }
            // the size from the zip directory caps the output: a lying entry can't balloon memory
            8 => {
                let (raw, buf) = SPARE.with(|s| s.take());
                Lazy { name: name.clone(), raw: reuse(raw, *csize), got: 0, file: Some(fh), buf: reuse(buf, *size), have: 0, deflated: true, prefix_pays }
            }
            m => return Err(format!("{name}: unsupported compression {m}")),
        };
        f(&mut lazy)
    }, |_, r| sink(r));
    Ok(())
}

/// One file out of an APK, without indexing the whole archive (big apps have 10k+ entries).
pub fn read_entry(path: &str, want: &str) -> Result<Option<Vec<u8>>, String> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(session::prepare(path)?).map_err(|e| format!("{path}: {e}"))?;
    let Some((_, local, csize, size, method)) = central_dir(&mut f, &|n| n == want.as_bytes())?.into_iter().next() else { return Ok(None) };
    let err = |e: std::io::Error| format!("{want}: {e}");
    let mut h = [0u8; 30];
    f.seek(SeekFrom::Start(local)).map_err(err)?;
    f.read_exact(&mut h).map_err(err)?;
    if h[..4] != [0x50, 0x4b, 3, 4] {
        return Err(format!("{want}: bad local header"));
    }
    f.seek(SeekFrom::Current(u16::from_le_bytes([h[26], h[27]]) as i64 + u16::from_le_bytes([h[28], h[29]]) as i64)).map_err(err)?;
    let mut raw = vec![0; csize];
    f.read_exact(&mut raw).map_err(err)?;
    match method {
        0 => Ok(Some(raw)),
        8 => {
            let mut out = vec![0; size];
            let k = libdeflater::Decompressor::new().deflate_decompress(&raw, &mut out).map_err(|e| format!("{want}: inflate failed: {e}"))?;
            out.truncate(k);
            Ok(Some(out))
        }
        m => Err(format!("{want}: unsupported compression {m}")),
    }
}

/// (name, local header offset, compressed size, size, method) of the zip entries whose raw name
/// passes `keep`, from the central directory. Zip64 archives go through the zip crate instead.
/// Big apps have 40k+ entries and a query wants a handful: names are only copied for those.
fn central_dir(f: &mut std::fs::File, keep: &dyn Fn(&[u8]) -> bool) -> Result<Vec<(String, u64, usize, usize, u16)>, String> {
    use std::io::{Seek, SeekFrom};
    let len = f.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    let tail_len = len.min(65_557);
    let mut tail = vec![0; tail_len as usize];
    f.seek(SeekFrom::Start(len - tail_len)).map_err(|e| e.to_string())?;
    f.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let eocd = (0..tail.len().saturating_sub(21)).rev().find(|&i| tail[i..i + 4] == [0x50, 0x4b, 5, 6]).ok_or("not a zip file")?;
    let u16_at = |d: &[u8], o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
    let u32_at = |d: &[u8], o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let (count, size, off) = (u16_at(&tail, eocd + 10), u32_at(&tail, eocd + 12), u32_at(&tail, eocd + 16));
    if count == u16::MAX || size == u32::MAX || off == u32::MAX {
        return zip_crate_dir(f, keep);
    }
    let mut cd = vec![0; size as usize];
    f.seek(SeekFrom::Start(off as u64)).map_err(|e| e.to_string())?;
    f.read_exact(&mut cd).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut p = 0;
    while p + 46 <= cd.len() && cd[p..p + 4] == [0x50, 0x4b, 1, 2] {
        let (n, x, c) = (u16_at(&cd, p + 28) as usize, u16_at(&cd, p + 30) as usize, u16_at(&cd, p + 32) as usize);
        let raw = cd.get(p + 46..p + 46 + n).ok_or("truncated central directory")?;
        let (cs, us, lo) = (u32_at(&cd, p + 20), u32_at(&cd, p + 24), u32_at(&cd, p + 42));
        if cs == u32::MAX || us == u32::MAX || lo == u32::MAX {
            return zip_crate_dir(f, keep);
        }
        if keep(raw) {
            out.push((String::from_utf8_lossy(raw).into_owned(), lo as u64, cs as usize, us as usize, u16_at(&cd, p + 10)));
        }
        p += 46 + n + x + c;
    }
    Ok(out)
}

fn zip_crate_dir(f: &mut std::fs::File, keep: &dyn Fn(&[u8]) -> bool) -> Result<Vec<(String, u64, usize, usize, u16)>, String> {
    let mut zip = zip::ZipArchive::new(f.try_clone().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let e = zip.by_index_raw(i).map_err(|e| e.to_string())?;
        if !keep(e.name_raw()) {
            continue;
        }
        let method = match e.compression() {
            zip::CompressionMethod::Stored => 0,
            zip::CompressionMethod::Deflated => 8,
            _ => 99,
        };
        out.push((e.name().map_err(|e| e.to_string())?.into_owned(), e.header_start(), e.compressed_size() as usize, e.size() as usize, method));
    }
    Ok(out)
}

/// `com.a.B`, `com/a/B` or `Lcom/a/B;` -> `Lcom/a/B;`
pub fn descriptor(name: &str) -> String {
    if name.starts_with('L') && name.ends_with(';') {
        return name.to_string();
    }
    format!("L{};", name.replace('.', "/"))
}

/// Package of the manifest, from the APK; empty if it can't be read.
pub fn package(path: &str) -> String {
    read_entry(path, "AndroidManifest.xml").ok().flatten().and_then(|m| crate::res::decode_xml(&m, None).ok()).and_then(|t| session::attr(&t, "package")).unwrap_or_default()
}

/// Every class (`descriptor = false`: Java names), optionally under `prefix`, skipping known
/// libraries with `app_only`.
pub fn classes(path: &str, prefix: &str, app_only: bool, raw: bool) -> Result<Vec<String>, String> {
    let mut lists = Vec::new();
    classes_each(path, prefix, app_only, raw, |l| lists.push(l))?;
    Ok(lists.into_iter().collect::<Result<Vec<_>, _>>()?.concat())
}

/// `classes`, each DEX file's list handed to `sink` in runtime order as soon as it is done.
pub fn classes_each(path: &str, prefix: &str, app_only: bool, raw: bool, sink: impl FnMut(Result<Vec<String>, String>)) -> Result<(), String> {
    let pkg = if app_only { package(path) } else { String::new() };
    scan_each(path, false, |_| true, |l| {
        l.class_names()?;
        let Some(dx) = l.dex() else { return Ok(vec![]) };
        Ok((0..dx.class_count)
            .filter_map(|c| dx.class_descriptor(c).ok())
            .filter_map(|d| {
                // the plain listing (no filter, descriptors) needs no Java name at all
                if raw && prefix.is_empty() && !app_only {
                    return Some(d);
                }
                let j = dex::java_name(&d);
                let keep = (j.starts_with(prefix) || d[1..d.len().saturating_sub(1).max(1)].replace('/', ".").starts_with(prefix))
                    && (!app_only || session::origin(&j, &pkg) != session::Origin::Library);
                keep.then(|| if raw { d } else { j })
            })
            .collect::<Vec<_>>())
    }, sink)
}

/// Defined methods as `Lcls;->name(params)Ret` lines, optionally of one class / with a name
/// filter: one text per DEX file, each line ending in a newline (millions of lines, so no
/// `String` each).
pub fn methods(path: &str, class: Option<&str>, filter: &str) -> Result<Vec<String>, String> {
    let mut all = Vec::new();
    methods_each(path, class, filter, |t| all.push(t))?;
    all.into_iter().collect()
}

/// `methods`, each DEX file's text handed to `sink` in runtime order as soon as it is done.
pub fn methods_each(path: &str, class: Option<&str>, filter: &str, sink: impl FnMut(Result<String, String>)) -> Result<(), String> {
    let want = class.map(descriptor);
    let low = filter.to_lowercase();
    // listing everything streams; one class is found faster biggest-file first
    scan_each(path, want.is_none(), |_| true, |l| {
        if let Some(w) = &want {
            // a DEX that doesn't define the class needs nothing more
            l.class_names_or_all()?;
            if l.dex().is_none_or(|d| d.find_class(w).is_none()) {
                return Ok(String::new());
            }
        }
        l.all()?;
        let Some(dx) = l.dex() else { return Ok(String::new()) };
        let mut out = String::new();
        let list: Vec<usize> = match &want {
            Some(w) => dx.find_class(w).into_iter().collect(),
            None => (0..dx.class_count).collect(),
        };
        // signatures repeat a lot: format each proto once
        let mut protos: std::collections::HashMap<u32, String> = Default::default();
        for c in list {
            let cls = dx.class_descriptor(c).unwrap_or_default();
            let _ = dx.each_method(c, |midx, _| {
                let Ok((_, sid)) = dx.member_ids(true, midx) else { return };
                // millions of lines: borrow plain UTF-8 names, append pieces without formatting
                let raw = dx.string_bytes(sid).unwrap_or_default();
                let name: std::borrow::Cow<str> = std::str::from_utf8(raw).map_or_else(|_| dex::mutf8(raw).into(), Into::into);
                if !low.is_empty() && !contains_ci(&name, &low) {
                    return;
                }
                let p = dx.method_proto(midx).unwrap_or(0);
                let proto = protos.entry(p).or_insert_with(|| dx.proto(p).unwrap_or_default());
                for part in [cls.as_str(), "->", &name, proto, "\n"] {
                    out.push_str(part);
                }
            });
        }
        Ok(out)
    }, sink)
}

/// `hay.to_lowercase().contains(low)` (`low` already lowercase), without allocating for ASCII.
fn contains_ci(hay: &str, low: &str) -> bool {
    if !hay.is_ascii() {
        return hay.to_lowercase().contains(low);
    }
    let (h, n) = (hay.as_bytes(), low.as_bytes());
    n.is_empty() || h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// What changed from `old` to `new`: classes, methods of classes both define, permissions and
/// (with `strings`) string-pool values. One line each, `+ ...` added or `- ...` removed, under
/// a `## section: +N -M` header.
pub fn diff(old: &str, new: &str, strings: bool) -> Result<Vec<String>, String> {
    use std::collections::BTreeSet;
    let both = |f: &dyn Fn(&str) -> Result<BTreeSet<String>, String>| -> Result<(BTreeSet<String>, BTreeSet<String>), String> { Ok((f(old)?, f(new)?)) };
    let mut out = Vec::new();
    let mut section = |title: &str, (a, b): &(BTreeSet<String>, BTreeSet<String>)| {
        let (added, gone): (Vec<&String>, Vec<&String>) = (b.difference(a).collect(), a.difference(b).collect());
        out.push(format!("## {title}: +{} -{}", added.len(), gone.len()));
        out.extend(added.iter().map(|s| format!("+ {s}")));
        out.extend(gone.iter().map(|s| format!("- {s}")));
    };
    let classes = both(&|p| Ok(classes(p, "", false, true)?.into_iter().collect()))?;
    section("classes", &classes);
    // methods only of classes both versions have: an added class's methods are no news
    let kept: BTreeSet<&String> = classes.0.intersection(&classes.1).collect();
    let methods = both(&|p| {
        Ok(methods(p, None, "")?.iter().flat_map(|t| t.lines()).filter(|m| m.split_once("->").is_some_and(|(c, _)| kept.contains(&c.to_string()))).map(str::to_string).collect())
    })?;
    section("methods", &methods);
    let perms = both(&|p| {
        let m = read_entry(p, "AndroidManifest.xml")?.ok_or_else(|| format!("{p}: no AndroidManifest.xml"))?;
        Ok(permissions(&crate::res::decode_xml(&m, None)?).into_iter().collect())
    })?;
    section("permissions", &perms);
    if strings {
        section("strings", &both(&|p| Ok(self::strings(p, "")?.into_iter().collect()))?);
    }
    Ok(out)
}

/// `uses-permission` (and `-sdk-23`) names in decoded manifest XML, whose attributes are
/// printed one per line: each element is taken up to the next tag.
fn permissions(xml: &str) -> Vec<String> {
    xml.split("<uses-permission").skip(1).filter_map(|e| session::attr(e.split('<').next()?, "android:name")).collect()
}

/// String-pool values containing `filter` (case-insensitive), in DEX then pool order.
pub fn strings(path: &str, filter: &str) -> Result<Vec<String>, String> {
    let re = regex::bytes::RegexBuilder::new(&regex::escape(filter)).case_insensitive(true).build().map_err(|e| e.to_string())?;
    let lists = scan(path, |l| {
        l.names()?;
        let Some(dx) = l.dex() else { return Ok(vec![]) };
        Ok((0..dx.counts()[0] as u32).filter_map(|i| dx.string_bytes(i).ok()).filter(|b| re.is_match(b)).map(dex::mutf8).collect::<Vec<_>>())
    })?;
    Ok(lists.concat())
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    String,
    Type,
    Method,
    Field,
    Extends,
    Impl,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "string" => Kind::String,
            "type" | "class" => Kind::Type,
            "method" => Kind::Method,
            "field" => Kind::Field,
            "extends" | "subclass" => Kind::Extends,
            "impl" | "implements" => Kind::Impl,
            _ => return None,
        })
    }
}

/// What `refs` looks for.
pub struct Query {
    pub kind: Kind,
    /// string text, type name, or member name
    pub text: String,
    /// owner class of a method / field
    pub class: Option<String>,
    /// `class` is a substring of the owner's descriptor
    pub fuzzy_class: bool,
    /// `text` is a regex (strings and member names)
    pub regex: bool,
    pub ignore_case: bool,
}

/// One reference: (dex, referencing method or class, what matched)
pub type Ref_ = (String, String, String);

/// A compiled `Query`, matched one DEX at a time.
pub struct Finder<'q> {
    q: &'q Query,
    re: regex::bytes::Regex,
    ty: String,
    owner: Option<String>,
}

impl<'q> Finder<'q> {
    pub fn new(q: &'q Query) -> Result<Finder<'q>, String> {
        let pat = if q.regex { q.text.clone() } else { regex::escape(&q.text) };
        let pat = match q.kind {
            // member names match whole unless a regex is given
            Kind::Method | Kind::Field if !q.regex => format!("^{pat}$"),
            _ => pat,
        };
        let re = regex::bytes::RegexBuilder::new(&pat).case_insensitive(q.ignore_case).build().map_err(|e| e.to_string())?;
        let owner = q.class.as_deref().map(|c| if q.fuzzy_class { c.replace('.', "/") } else { descriptor(c) });
        Ok(Finder { q, re, ty: descriptor(&q.text), owner })
    }

    fn owner_ok(&self, o: &str) -> bool {
        match &self.owner {
            None => true,
            Some(w) if self.q.fuzzy_class => o.contains(w.as_str()),
            Some(w) => o == w,
        }
    }

    /// Subclasses / implementors (only for `extends` / `impl`).
    fn supertypes(&self, dname: &str, dx: &Dex) -> Vec<Ref_> {
        let mut out = Vec::new();
        for c in 0..dx.class_count {
            // `extends` runs on a DEX inflated only through its names: interface lists aren't there
            let hit = match self.q.kind {
                Kind::Extends => dx.superclass(c).ok().flatten().as_deref() == Some(self.ty.as_str()),
                _ => dx.class_def(c).is_ok_and(|d| d.interfaces.iter().any(|i| *i == self.ty)),
            };
            if let (true, Ok(class)) = (hit, dx.class_descriptor(c)) {
                out.push((dname.to_string(), class, self.ty.clone()));
            }
        }
        out
    }

    /// Ids of this DEX's table that match, as a bitmap; None if nothing does.
    fn wanted(&self, dx: &Dex) -> Option<Vec<bool>> {
        let table = match self.q.kind {
            Kind::String => 0,
            Kind::Type => 1,
            Kind::Field => 2,
            _ => 3,
        };
        let mut want = vec![false; dx.counts()[table]];
        // exact names: one binary search in the sorted string table instead of a scan
        let exact = !self.q.regex && !self.q.ignore_case;
        match self.q.kind {
            Kind::String => self.matching_strings(dx, &mut want),
            Kind::Type => {
                let t = dx.find_string(self.ty.as_bytes()).and_then(|sid| dx.find_type(sid))?;
                want[t as usize] = true;
            }
            _ => {
                let method = self.q.kind == Kind::Method;
                let name = if exact { Some(dx.find_string(self.q.text.as_bytes())?) } else { None };
                for (id, w) in want.iter_mut().enumerate() {
                    let Ok((owner, sid)) = dx.member_ids(method, id as u32) else { continue };
                    let named = match name {
                        Some(n) => sid == n,
                        None => dx.string_bytes(sid).is_ok_and(|b| self.re.is_match(b)),
                    };
                    *w = named && (self.owner.is_none() || dx.type_name(owner).is_ok_and(|o| self.owner_ok(&o)));
                }
            }
        }
        want.contains(&true).then_some(want)
    }

    /// Strings matching the query: the regex runs once over the string-data area, and each hit
    /// is mapped back to its string and checked against that string alone.
    fn matching_strings(&self, dx: &Dex, want: &mut [bool]) {
        let offs = dx.string_offsets();
        let (Some(first), Some(last)) = (offs.first(), offs.last()) else { return };
        let data = dx.bytes();
        let end = (last.0 as usize..data.len()).find(|&i| data[i] == 0).unwrap_or(data.len());
        let Some(blob) = data.get(first.0 as usize..end) else { return };
        let base = first.0 as usize;
        let mut from = 0;
        while let Some(m) = self.re.find_at(blob, from) {
            let at = (base + m.start()) as u32;
            let k = offs.partition_point(|o| o.0 <= at).saturating_sub(1);
            let id = offs[k].1;
            if dx.string_bytes(id).is_ok_and(|b| self.re.is_match(b)) {
                want[id as usize] = true;
            }
            // go on after this string, so a match can't hide one inside it
            let next = offs.get(k + 1).map_or(blob.len(), |o| o.0 as usize - base);
            from = next.max(m.start() + 1);
            if from >= blob.len() {
                break;
            }
        }
    }

    /// Methods whose bytecode uses a wanted id; stops once `full()` says enough were found.
    fn users(&self, dname: &str, dx: &Dex, want: &[bool], full: impl Fn(usize) -> bool) -> Vec<Ref_> {
        let wanted_ref = match self.q.kind {
            Kind::String => Ref::String,
            Kind::Type => Ref::Type,
            Kind::Field => Ref::Field,
            _ => Ref::Method,
        };
        let mut out = Vec::new();
        for c in 0..dx.class_count {
            let _ = dx.each_method(c, |midx, insns| {
                let Some(insns) = insns else { return };
                let mut seen: Vec<u32> = Vec::new();
                bytecode::for_each_ref(insns, |r, idx| {
                    if r == wanted_ref && want.get(idx as usize) == Some(&true) && !seen.contains(&idx) {
                        seen.push(idx);
                    }
                });
                for id in seen {
                    let what = match self.q.kind {
                        Kind::String => dx.string(id).unwrap_or_default(),
                        Kind::Type => dx.type_name(id).unwrap_or_default(),
                        Kind::Field => dx.field_ref(id).unwrap_or_default(),
                        _ => dx.method_ref(id).unwrap_or_default(),
                    };
                    out.push((dname.to_string(), dx.method_ref(midx).unwrap_or_default(), what));
                }
            });
            if full(out.len()) {
                break;
            }
        }
        out
    }
}

/// Code references to a string, type, method or field (the methods that use it), or the
/// subclasses / implementors of a type. Stops after `limit` hits.
pub fn refs(path: &str, q: &Query, limit: usize) -> Result<Vec<Ref_>, String> {
    let f = Finder::new(q)?;
    let found = AtomicUsize::new(0);
    // (DEX files checked, how many matched): when most match, a prefix pass is wasted work
    let (checked, matched) = (AtomicUsize::new(0), AtomicUsize::new(0));
    // runtime order: with --limit, which files are skipped once enough is found stays as before
    let mut lists = Vec::new();
    scan_each(path, true, |_| true, |l| {
        if found.load(Ordering::Relaxed) >= limit {
            return Ok(vec![]);
        }
        let (c, m) = (checked.load(Ordering::Relaxed), matched.load(Ordering::Relaxed));
        if matches!(q.kind, Kind::Extends) {
            l.names()?; // superclasses are in the class defs: names are all it needs
        } else if !l.prefix_pays {
            // every file inflates at once anyway: one that matches would pay prefix + full
            l.all()?;
        } else {
            // chance this file doesn't match, from the files seen so far (even odds at first)
            l.names_or_all(if c == 0 { 0.5 } else { (c - m) as f64 / c as f64 })?;
        }
        if q.kind == Kind::Impl {
            l.all()?; // interface lists live in the data section
        }
        let dname = l.name.clone();
        let Some(dx) = l.dex() else { return Ok(vec![]) };
        let out = if matches!(q.kind, Kind::Extends | Kind::Impl) {
            f.supertypes(&dname, &dx)
        } else {
            checked.fetch_add(1, Ordering::Relaxed);
            let Some(want) = f.wanted(&dx) else { return Ok(vec![]) };
            matched.fetch_add(1, Ordering::Relaxed);
            drop(dx);
            // only now the code: a DEX whose tables don't mention the query is never inflated past them
            l.all()?;
            let Some(dx) = l.dex() else { return Ok(vec![]) };
            f.users(&dname, &dx, &want, |n| found.load(Ordering::Relaxed) + n >= limit)
        };
        found.fetch_add(out.len(), Ordering::Relaxed);
        Ok(out)
    }, |r| lists.push(r))?;
    let mut all = lists.into_iter().collect::<Result<Vec<_>, String>>()?.concat();
    all.truncate(limit);
    Ok(all)
}

/// `refs` over DEX files already in memory (the warm MCP worker).
pub fn refs_loaded(dexes: &[(String, Dex)], q: &Query, limit: usize) -> Result<Vec<Ref_>, String> {
    let f = Finder::new(q)?;
    let found = AtomicUsize::new(0);
    let lists = par(dexes.len(), |k| {
        let (dname, dx) = (&dexes[k].0, &dexes[k].1);
        let out = match q.kind {
            Kind::Extends | Kind::Impl => f.supertypes(dname, dx),
            _ => f.wanted(dx).map(|w| f.users(dname, dx, &w, |n| found.load(Ordering::Relaxed) + n >= limit)).unwrap_or_default(),
        };
        found.fetch_add(out.len(), Ordering::Relaxed);
        out
    });
    let mut all = lists.concat();
    all.truncate(limit);
    Ok(all)
}

/// Decompiled Java (or Smali) of the named classes, only their DEX files' names deobfuscated.
/// Unknown names come back as errors next to the found ones.
pub fn decompile(path: &str, names: &[String], smali: bool, deobf: bool) -> Result<Vec<(String, Result<String, String>)>, String> {
    crate::deobf::set_notes(crate::re::load_notes(path)?);
    let wanted: Vec<String> = names.iter().map(|n| descriptor(n)).collect();
    let seen: Vec<std::sync::atomic::AtomicBool> = wanted.iter().map(|_| Default::default()).collect();
    // only DEX files that define a wanted class are inflated past their names. classes.dex goes
    // alone first: the runtime looks there first, and app / startup classes live there, so
    // usually the bigger files are never opened. Classes still missing come from the others.
    // shortcut: past classes.dex, a class defined twice comes from whichever DEX finds it first,
    // not always the lowest-numbered one as at runtime; duplicates are rare.
    let look = |l: &mut Lazy, skip: &[bool]| -> Result<Option<(Vec<(usize, usize)>, Vec<u8>)>, String> {
        if seen.iter().all(|s| s.load(Ordering::Relaxed)) {
            return Ok(None);
        }
        l.class_names_or_all()?;
        let hits: Vec<(usize, usize)> = match l.dex() {
            Some(dx) => wanted.iter().enumerate().filter(|(k, _)| !skip[*k]).filter_map(|(k, w)| dx.find_class(w).map(|c| (k, c))).collect(),
            None => vec![],
        };
        if hits.is_empty() {
            return Ok(None);
        }
        hits.iter().for_each(|h| seen[h.0].store(true, Ordering::Relaxed));
        l.all()?;
        Ok(Some((hits, std::mem::take(&mut l.buf))))
    };
    let mut found = Vec::new();
    let none = vec![false; wanted.len()];
    // classes.dex alone first pays only when its names end early (Redex layout): then a miss
    // costs a short prefix. When they run deep (d8 / R8, p ~ 0.7-0.9) it would need a near-full
    // inflate before the others even start, so every file goes in one parallel pass instead:
    // max(T) rather than T(classes.dex) + T(rest).
    let spread = std::sync::atomic::AtomicBool::new(false);
    scan_each(path, false, |n| n == "classes.dex", |l| {
        if l.class_names_fraction()? > 0.5 {
            spread.store(true, Ordering::Relaxed);
            return Ok(None);
        }
        look(l, &none)
    }, |r| found.push(r))?;
    if spread.load(Ordering::Relaxed) {
        scan_each(path, false, |_| true, |l| look(l, &none), |r| found.push(r))?;
    } else {
        let in_first: Vec<bool> = seen.iter().map(|s| s.load(Ordering::Relaxed)).collect();
        if in_first.contains(&false) {
            scan_each(path, false, |n| n != "classes.dex", |l| look(l, &in_first), |r| found.push(r))?;
        }
    }
    let found = found.into_iter().collect::<Result<Vec<_>, String>>()?;
    let files: Vec<(Vec<(usize, usize)>, Vec<u8>)> = found.into_iter().flatten().collect();
    let dexes: Vec<Dex> = files.iter().filter_map(|f| Dex::parse(&f.1).ok()).collect();
    // where each class lives: (dex, class index); the first DEX wins, as at runtime
    let mut spots: Vec<Option<(usize, usize)>> = vec![None; names.len()];
    for (x, (hits, _)) in files.iter().enumerate() {
        for &(k, c) in hits {
            spots[k].get_or_insert((x, c));
        }
    }
    if !smali {
        // names are resolved against the DEX files that hold the classes asked for; references
        // into other DEX files keep their original names
        if deobf {
            crate::deobf::init(&dexes);
        }
        crate::imports::init(&dexes);
    }
    Ok(par(names.len(), |k| {
        let text = match spots[k] {
            None => Err(format!("class {} not found", names[k])),
            Some((x, c)) if smali => Ok(session::smali(&dexes[x], c)),
            Some((x, c)) => java::class(&dexes[x], c),
        };
        (names[k].clone(), text)
    }))
}

/// The method(s) named `name` cut out of decompiled Java: declaration line through its closing
/// brace. An original obfuscated name finds its renamed method too (`a` -> `ma`).
pub fn method_of(java: &str, name: &str) -> Option<String> {
    method_named(java, name).or_else(|| method_named(java, &crate::deobf::method("", name)?))
}

fn method_named(java: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = java.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut out = String::new();
    let mut k = 0;
    while k < lines.len() {
        let t = lines[k].trim();
        let decl = t.contains(&format!(" {name}(")) && !t.ends_with(';') && !t.starts_with("return") && !t.contains(" = ") && !t.contains('.');
        if decl {
            let ind = indent(lines[k]);
            let end = (k + 1..lines.len()).find(|&e| indent(lines[e]) == ind && lines[e].trim() == "}").unwrap_or(k);
            // annotations directly above belong to the method
            let start = (0..k).rev().take_while(|&a| lines[a].trim().starts_with('@')).last().unwrap_or(k);
            let _ = writeln!(out, "{}\n", lines[start..=end].join("\n"));
            k = end + 1;
        } else {
            k += 1;
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive_contains() {
        for (h, n) in [("openConnection", "connect"), ("OPEN", "open"), ("abc", "x"), ("Ã‰tÃ©", "Ã©tÃ©"), ("abc", "Ã©"), ("a", "")] {
            assert_eq!(contains_ci(h, n), h.to_lowercase().contains(n), "{h} {n}");
        }
    }

    #[test]
    fn manifest_permissions() {
        let xml = "<manifest>\n    <uses-permission\n        android:name=\"android.permission.INTERNET\" />\n    <uses-permission-sdk-23\n        android:name=\"android.permission.CAMERA\" />\n    <application\n        android:name=\"App\" />\n</manifest>";
        assert_eq!(permissions(xml), ["android.permission.INTERNET", "android.permission.CAMERA"]);
    }

    #[test]
    fn mutf8_nul_and_supplementary() {
        assert_eq!(dex::mutf8(b"plain"), "plain");
        assert_eq!(dex::mutf8(&[b'a', 0xc0, 0x80, b'b']), "a\0b");
        // U+1F600 as two 3-byte surrogates
        assert_eq!(dex::mutf8(&[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]), "\u{1F600}");
    }

    #[test]
    fn descriptors() {
        assert_eq!(descriptor("com.a.B"), "Lcom/a/B;");
        assert_eq!(descriptor("Lcom/a/B;"), "Lcom/a/B;");
        assert_eq!(descriptor("com/a/B$C"), "Lcom/a/B$C;");
    }

    #[test]
    fn cuts_one_method() {
        let java = "class A {\n    @Override\n    public void run(int x) {\n        go(x);\n    }\n\n    void other() {\n        run(1);\n    }\n}\n";
        let m = method_of(java, "run").unwrap();
        assert!(m.contains("@Override") && m.contains("go(x);") && !m.contains("other"));
        assert!(method_of(java, "missing").is_none());
    }
}
