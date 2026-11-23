//! Rebuild an APK with edits, into anything that writes: untouched entries are copied byte for
//! byte (no recompression), replaced or added files are written fresh, old v1 signature files
//! are dropped, uncompressed data is aligned the way `zipalign` does (4 bytes, 4096 for native
//! libraries). What it writes is unsigned: [`crate::apksign::sign`] signs it where it is.

use crate::{Error, Result};
use std::collections::BTreeMap;
use std::io::{self, Read, Seek, Write};

/// name -> new content, or None to delete
pub type Edits = BTreeMap<String, Option<Vec<u8>>>;

/// 1981-01-01 00:00, the fixed timestamp Android's build tools use
const DOS_DATE: u16 = (1 << 9) | (1 << 5) | 1;

struct Entry {
    name: String,
    method: u16,
    crc: u32,
    csize: u32,
    usize: u32,
    offset: u32,
}

const MANIFEST: &str = "META-INF/MANIFEST.MF";

/// v1 (JAR) signature files: stale once the APK changes, and replaced by v2/v3
pub fn is_v1_signature(name: &str) -> bool {
    let up = name.to_uppercase();
    up.starts_with("META-INF/")
        && !up[9..].contains('/')
        && (up == "META-INF/MANIFEST.MF"
            || [".SF", ".RSA", ".DSA", ".EC"]
                .iter()
                .any(|x| up.ends_with(x)))
}

/// Entries that must stay uncompressed: Android maps them straight from the file.
fn must_store(name: &str) -> bool {
    name == "resources.arsc" || name.ends_with(".so")
}

fn alignment(name: &str, method: u16) -> usize {
    match (method, name.ends_with(".so")) {
        (0, true) => 4096,
        (0, false) => 4,
        _ => 1,
    }
}

fn push16(o: &mut Vec<u8>, v: u16) {
    o.extend_from_slice(&v.to_le_bytes());
}

fn push32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}

/// Where the rebuilt APK goes, and how much of it is written.
struct Out<W> {
    out: W,
    at: u64,
}

impl<W: Write> Write for Out<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let n = self.out.write(data)?;
        self.at += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// Write the local file header (with alignment padding) of an entry whose data, `csize` bytes,
/// the caller writes next.
fn header<W: Write>(
    out: &mut Out<W>,
    name: &str,
    method: u16,
    crc: u32,
    csize: u32,
    usize: u32,
) -> Result<Entry> {
    let offset = u32::try_from(out.at).map_err(|_| Error::TooLarge("the APK".into()))?;
    let align = alignment(name, method);
    let base = offset as usize + 30 + name.len();
    let mut h = Vec::with_capacity(30 + name.len() + align + 6);
    let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
    push32(&mut h, 0x0403_4b50);
    push16(&mut h, if method == 0 { 10 } else { 20 });
    push16(&mut h, flags);
    push16(&mut h, method);
    push16(&mut h, 0);
    push16(&mut h, DOS_DATE);
    push32(&mut h, crc);
    push32(&mut h, csize);
    push32(&mut h, usize);
    push16(&mut h, name.len() as u16);
    // pad with Android's alignment extra field (id 0xd935): id, size, alignment, zeros
    let extra = if align > 1 {
        let mut len = 6;
        while !(base + len).is_multiple_of(align) {
            len += 1;
        }
        len
    } else {
        0
    };
    push16(&mut h, extra as u16);
    h.extend_from_slice(name.as_bytes());
    if extra > 0 {
        let end = h.len() + extra;
        push16(&mut h, 0xd935);
        push16(&mut h, (extra - 4) as u16);
        push16(&mut h, align as u16);
        h.resize(end, 0);
    }
    out.write_all(&h).map_err(Error::Write)?;
    Ok(Entry {
        name: name.to_string(),
        method,
        crc,
        csize,
        usize,
        offset,
    })
}

/// Rebuild `src` with `edits` applied into `out`, unsigned. How many bytes that wrote.
pub fn rebuild<W: Write>(src: &str, edits: &Edits, out: W) -> Result<u64> {
    let f = std::fs::File::open(src).map_err(Error::io(src))?;
    rebuild_from(f, edits, out).map_err(|e| e.at(src))
}

/// [`rebuild`] from anything that reads and seeks, such as a `Cursor` over the APK's bytes.
pub fn rebuild_from<R: Read + Seek, W: Write>(src: R, edits: &Edits, out: W) -> Result<u64> {
    archive(&mut zip::ZipArchive::new(src)?, edits, out)
}

/// [`rebuild`] an archive already open. An entry the edits leave alone goes from the archive to
/// `out` as it is read, so the APK is never whole in memory.
pub fn archive<R: Read + Seek, W: Write>(
    zip: &mut zip::ZipArchive<R>,
    edits: &Edits,
    out: W,
) -> Result<u64> {
    let mut out = Out { out, at: 0 };
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // a manifest without signature files beside it is a plain JAR's, not a v1 signature
    let v1_signed = zip
        .file_names()
        .flatten()
        .any(|n| is_v1_signature(&n) && !n.eq_ignore_ascii_case(MANIFEST));
    for i in 0..zip.len() {
        let mut e = zip.by_index_raw(i)?;
        let name = e.name()?.into_owned();
        let stale = is_v1_signature(&name) && (v1_signed || !name.eq_ignore_ascii_case(MANIFEST));
        if name.ends_with('/') || stale || !seen.insert(name.clone()) {
            continue;
        }
        match edits.get(&name) {
            Some(None) => continue,
            Some(Some(new)) => {
                // a file the APK kept uncompressed stays that way: Android runs uncompressed
                // DEX (useEmbeddedDex, system JARs) and maps other stored files in place
                let stored = e.compression() == zip::CompressionMethod::Stored;
                drop(e);
                entries.push(fresh(&mut out, &name, new, stored)?);
            }
            None => {
                let method = match e.compression() {
                    zip::CompressionMethod::Stored => 0,
                    zip::CompressionMethod::Deflated => 8,
                    other => {
                        return Err(Error::UnsupportedCompression {
                            entry: name,
                            method: format!("{other:?}"),
                        });
                    }
                };
                let csize = u32::try_from(e.compressed_size())
                    .map_err(|_| Error::TooLarge(name.clone()))?;
                entries.push(header(
                    &mut out,
                    &name,
                    method,
                    e.crc32(),
                    csize,
                    e.size() as u32,
                )?);
                copy(&mut e, &mut out).map_err(|err| err.at(&name))?;
            }
        }
    }
    // added files
    for (name, new) in edits {
        if let Some(new) = new
            && !seen.contains(name)
        {
            entries.push(fresh(&mut out, name, new, false)?);
        }
    }
    if entries.len() > 0xffff {
        return Err(Error::TooManyEntries);
    }
    let start = u32::try_from(out.at).map_err(|_| Error::TooLarge("the APK".into()))?;
    let mut cd = Vec::with_capacity(entries.len() * 96);
    for e in &entries {
        push32(&mut cd, 0x0201_4b50);
        push16(&mut cd, 20);
        push16(&mut cd, if e.method == 0 { 10 } else { 20 });
        push16(&mut cd, if e.name.is_ascii() { 0 } else { 0x800 });
        push16(&mut cd, e.method);
        push16(&mut cd, 0);
        push16(&mut cd, DOS_DATE);
        push32(&mut cd, e.crc);
        push32(&mut cd, e.csize);
        push32(&mut cd, e.usize);
        push16(&mut cd, e.name.len() as u16);
        cd.extend_from_slice(&[0; 8]); // extra and comment length, disk, internal attrs
        push32(&mut cd, 0); // external attrs
        push32(&mut cd, e.offset);
        cd.extend_from_slice(e.name.as_bytes());
    }
    let mut end = Vec::with_capacity(22);
    push32(&mut end, 0x0605_4b50);
    push32(&mut end, 0);
    push16(&mut end, entries.len() as u16);
    push16(&mut end, entries.len() as u16);
    push32(&mut end, cd.len() as u32);
    push32(&mut end, start);
    push16(&mut end, 0);
    out.write_all(&cd).map_err(Error::Write)?;
    out.write_all(&end).map_err(Error::Write)?;
    out.flush().map_err(Error::Write)?;
    Ok(out.at)
}

/// Copy an entry's data, as the archive has it, into `out`.
fn copy<W: Write>(from: &mut impl Read, out: &mut Out<W>) -> Result<()> {
    let mut buf = [0; 1 << 16];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(zip::result::ZipError::Io(e).into()),
        };
        out.write_all(&buf[..n]).map_err(Error::Write)?;
    }
}

/// A new or replaced file: stored when Android needs it uncompressed or the file it replaces
/// was (`stored`), else deflated.
fn fresh<W: Write>(out: &mut Out<W>, name: &str, data: &[u8], stored: bool) -> Result<Entry> {
    let crc = crc32fast::hash(data);
    let usize = u32::try_from(data.len()).map_err(|_| Error::TooLarge(name.into()))?;
    let z = match stored || must_store(name) {
        true => None,
        // keep it stored when deflate doesn't help (already-compressed media)
        false => {
            Some(miniz_oxide::deflate::compress_to_vec(data, 6)).filter(|z| z.len() < data.len())
        }
    };
    let (method, data) = match &z {
        Some(z) => (8, &z[..]),
        None => (0, data),
    };
    let entry = header(out, name, method, crc, data.len() as u32, usize)?;
    out.write_all(data).map_err(Error::Write)?;
    Ok(entry)
}

#[cfg(test)]
#[path = "../../tests/apkscope-core/rebuild.rs"]
mod tests;
