//! Unity IL2CPP games: C# method names and addresses, string constants, and which method uses
//! which string, read from `global-metadata.dat` + arm64 `libil2cpp.so` without decompiling everything.
//! Built and checked against metadata v39 (Unity 6).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};

fn u(d: &[u8], off: usize, n: usize) -> u64 {
    d.get(off..off + n)
        .map_or(0, |b| b.iter().rev().fold(0, |a, &x| (a << 8) | x as u64))
}

fn cstr(d: &[u8], off: usize) -> &str {
    let s = d.get(off..).unwrap_or_default();
    std::str::from_utf8(&s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())]).unwrap_or("?")
}

pub struct Method {
    pub addr: u64,
    pub image: String,
    /// `Namespace.Class::Method`
    pub name: String,
}

struct Sec {
    addr: u64,
    off: u64,
    size: u64,
    ty: u32,
    flags: u64,
}

pub struct Game {
    so: Vec<u8>,
    secs: Vec<Sec>,
    /// R_AARCH64_RELATIVE: where -> value
    rel: HashMap<u64, u64>,
    /// sorted by address; methods that share one body (identical code folding) share an address
    pub methods: Vec<Method>,
    pub literals: Vec<String>,
}

/// Pull `global-metadata.dat` and the arm64 `libil2cpp.so` out of APKs, split APKs or `.xapk`/`.apks` bundles.
pub fn load(paths: &[String]) -> Result<Game, String> {
    let (mut meta, mut so) = (None, None);
    for p in paths {
        let f = std::fs::File::open(p).map_err(|e| format!("{p}: {e}"))?;
        find(f, &mut meta, &mut so, true).map_err(|e| format!("{p}: {e}"))?;
    }
    let meta = meta.ok_or("no global-metadata.dat found (not a Unity IL2CPP game?)")?;
    let so = so
        .ok_or("no lib/arm64-v8a/libil2cpp.so found; pass the arm64 split APK too, or the .xapk")?;
    Game::new(&meta, so)
}

fn find<R: Read + Seek>(
    r: R,
    meta: &mut Option<Vec<u8>>,
    so: &mut Option<Vec<u8>>,
    outer: bool,
) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(r).map_err(|e| e.to_string())?;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        let n = f.name().map_err(|e| e.to_string())?.into_owned();
        let kind = if meta.is_none() && n.ends_with("Metadata/global-metadata.dat") {
            1
        } else if so.is_none() && n.ends_with("arm64-v8a/libil2cpp.so") {
            2
        } else if outer && n.ends_with(".apk") {
            3
        } else {
            continue;
        };
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf).map_err(|e| format!("{n}: {e}"))?;
        match kind {
            1 => *meta = Some(buf),
            2 => *so = Some(buf),
            _ => {
                find(std::io::Cursor::new(buf), meta, so, false).map_err(|e| format!("{n}: {e}"))?
            }
        }
    }
    Ok(())
}

// metadata header sections (v38+: offset, size, count)
const LITERALS: usize = 0;
const LITERAL_DATA: usize = 1;
const STRINGS: usize = 2;
const METHODS: usize = 5;
const FIELDS: usize = 11;
const TYPEDEFS: usize = 19;
const IMAGES: usize = 20;

/// A string-literal hit: (address, its index in the string table).
pub type LitHit = (u64, usize);
/// A direct call: (address, target address).
pub type CallHit = (u64, u64);

impl Game {
    fn new(md: &[u8], so: Vec<u8>) -> Result<Game, String> {
        if u(md, 0, 4) != 0xFAB1_1BAF {
            return Err("global-metadata.dat is not plain IL2CPP metadata (encrypted?)".into());
        }
        let ver = u(md, 4, 4);
        if ver < 38 {
            return Err(format!(
                "metadata v{ver}: only v38+ (Unity 6) is supported so far"
            ));
        }
        let sec: Vec<(usize, usize, usize)> = (0..31)
            .map(|i| {
                (
                    u(md, 8 + i * 12, 4) as usize,
                    u(md, 12 + i * 12, 4) as usize,
                    u(md, 16 + i * 12, 4) as usize,
                )
            })
            .collect();
        // Unity 6 stores type, type-definition and generic-container indices in 1, 2 or 4 bytes;
        // the record sizes give the widths: field = 8+T, typedef = 68+3T+G, method = 24+TD+T+G
        let rec = |i: usize| sec[i].1 / sec[i].2.max(1);
        let bad = || format!("metadata v{ver}: unexpected record sizes");
        let t = rec(FIELDS).checked_sub(8).ok_or_else(bad)?;
        let g = rec(TYPEDEFS).checked_sub(68 + 3 * t).ok_or_else(bad)?;
        let td = rec(METHODS).checked_sub(24 + t + g).ok_or_else(bad)?;
        if ![t, g, td].iter().all(|w| [1, 2, 4].contains(w)) {
            return Err(bad());
        }
        let s = |i: u64| cstr(md, sec[STRINGS].0 + i as usize).to_string();

        // string literals: sorted start offsets into one blob, plus an end offset
        let (lo, ld) = (sec[LITERALS], sec[LITERAL_DATA].0);
        let offs: Vec<usize> = (0..lo.2).map(|i| u(md, lo.0 + i * 4, 4) as usize).collect();
        let literals = offs
            .windows(2)
            .map(|w| {
                String::from_utf8_lossy(md.get(ld + w[0]..ld + w[1]).unwrap_or_default())
                    .into_owned()
            })
            .collect();

        let mut g_ = Game {
            secs: sections(&so)?,
            so,
            rel: HashMap::new(),
            methods: Vec::new(),
            literals,
        };
        g_.relocs();

        // each assembly's code module starts with a pointer to its name, then the method count and pointer table
        let mut by_name: HashMap<String, u64> = HashMap::new();
        for sc in g_.secs.iter().filter(|x| x.ty == 1 && x.flags & 5 == 0) {
            let d = g_
                .so
                .get(sc.off as usize..(sc.off + sc.size) as usize)
                .unwrap_or_default();
            let mut start = 0;
            for (i, &b) in d.iter().enumerate() {
                if b == 0 {
                    if let Ok(n) = std::str::from_utf8(&d[start..i])
                        && n.ends_with(".dll")
                    {
                        by_name
                            .entry(n.to_string())
                            .or_insert(sc.addr + start as u64);
                    }
                    start = i + 1;
                }
            }
        }
        let wanted: HashSet<u64> = by_name.values().copied().collect();
        let mut refs: HashMap<u64, Vec<u64>> = HashMap::new();
        for (&at, &v) in &g_.rel {
            if wanted.contains(&v) {
                refs.entry(v).or_default().push(at);
            }
        }

        let (img, tds, ms) = (sec[IMAGES], sec[TYPEDEFS], sec[METHODS]);
        let (irec, tdrec, mrec) = (rec(IMAGES), rec(TYPEDEFS), rec(METHODS));
        let mut methods = Vec::new();
        for k in 0..img.2 {
            let o = img.0 + k * irec;
            let image = s(u(md, o, 4));
            let (first, count) = (u(md, o + 8, td), u(md, o + 8 + td, td));
            let Some(module) = by_name.get(&image).and_then(|a| refs.get(a)).and_then(|v| {
                v.iter()
                    .copied()
                    .find(|&m| (1..1 << 20).contains(&g_.read(m + 8)))
            }) else {
                continue;
            };
            let (n, table) = (g_.read(module + 8), g_.read(module + 16));
            for ty in first..first + count {
                let o = tds.0 + ty as usize * tdrec;
                let (name, ns) = (s(u(md, o, 4)), s(u(md, o + 4, 4)));
                let base = 8 + 3 * t + g;
                let (mstart, mcount) = (u(md, o + base + 8, 4), u(md, o + base + 36, 2));
                // shortcut: nested types print without their outer class, the link needs the .so type table
                let class = if ns.is_empty() {
                    name
                } else {
                    format!("{ns}.{name}")
                };
                for m in mstart..mstart + mcount {
                    let mo = ms.0 + m as usize * mrec;
                    let token = u(md, mo + 12 + td + t + g, 4);
                    let idx = (token & 0xff_ffff).wrapping_sub(1);
                    if idx >= n {
                        continue;
                    }
                    let addr = g_.read(table + idx * 8);
                    if addr != 0 {
                        methods.push(Method {
                            addr,
                            image: image.clone(),
                            name: format!("{class}::{}", s(u(md, mo, 4))),
                        });
                    }
                }
            }
        }
        methods.sort_by(|a, b| (a.addr, &a.name).cmp(&(b.addr, &b.name)));
        g_.methods = methods;
        Ok(g_)
    }

    fn relocs(&mut self) {
        for sc in self.secs.iter().filter(|x| x.ty == 4) {
            for k in 0..sc.size / 24 {
                let o = (sc.off + k * 24) as usize;
                if u(&self.so, o + 8, 4) == 1027 {
                    self.rel.insert(u(&self.so, o, 8), u(&self.so, o + 16, 8));
                }
            }
        }
    }

    fn off(&self, addr: u64) -> Option<usize> {
        self.secs
            .iter()
            .find(|s| s.ty != 8 && s.addr != 0 && (s.addr..s.addr + s.size).contains(&addr))
            .map(|s| (addr - s.addr + s.off) as usize)
    }

    /// pointer-sized value at `addr` as the loader leaves it (relocated pointers included)
    fn read(&self, addr: u64) -> u64 {
        self.rel
            .get(&addr)
            .copied()
            .unwrap_or_else(|| self.off(addr).map_or(0, |o| u(&self.so, o, 8)))
    }

    /// The method holding `addr`, and how many others share that body.
    pub fn method_at(&self, addr: u64) -> Option<(&Method, usize)> {
        let i = self
            .methods
            .partition_point(|m| m.addr <= addr)
            .checked_sub(1)?;
        let start = self.methods[i].addr;
        let first = self.methods.partition_point(|m| m.addr < start);
        Some((&self.methods[first], i - first))
    }

    /// String literals and direct calls in `[from, to)`.
    pub fn scan(&self, from: u64, to: u64) -> (Vec<LitHit>, Vec<CallHit>) {
        let (mut lits, mut calls) = (Vec::new(), Vec::new());
        let Some(o) = self.off(from) else {
            return (lits, calls);
        };
        let code = self.so.get(o..o + (to - from) as usize).unwrap_or_default();
        let w: Vec<u32> = code
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        for (i, &x) in w.iter().enumerate() {
            let pc = from + i as u64 * 4;
            if let Some(page) = adrp_page(pc, x) {
                let rd = x & 31;
                // the matching 64-bit load from that page, usually a few instructions later
                let Some(&ld) = w[i + 1..w.len().min(i + 16)]
                    .iter()
                    .find(|&&y| y & 0xffc0_0000 == 0xf940_0000 && (y >> 5) & 31 == rd)
                else {
                    continue;
                };
                let at = page + ((ld as u64 >> 10) & 0xfff) * 8;
                // a GOT entry holding the slot's address, or the slot itself
                let slot = self.rel.get(&at).copied().unwrap_or(at);
                if let Some(k) = literal_index(self.read(slot)).filter(|&k| k < self.literals.len())
                {
                    lits.push((pc, k));
                }
            } else if x & 0xfc00_0000 == 0x9400_0000 {
                let imm = ((x & 0x3ff_ffff) as i64) << 38 >> 36;
                calls.push((pc, pc.wrapping_add(imm as u64)));
            }
        }
        (lits, calls)
    }

    /// Every string-literal use in the game code.
    pub fn literal_uses(&self) -> Vec<(u64, usize)> {
        self.secs
            .iter()
            .filter(|s| s.flags & 4 != 0)
            .flat_map(|s| self.scan(s.addr, s.addr + s.size).0)
            .collect()
    }

    /// End of the method body starting at `addr` (the next method's start, capped).
    pub fn end_of(&self, addr: u64) -> u64 {
        let i = self.methods.partition_point(|m| m.addr <= addr);
        self.methods
            .get(i)
            .map_or(addr + 0x1000, |m| m.addr)
            .min(addr + 0x10000)
    }
}

fn sections(d: &[u8]) -> Result<Vec<Sec>, String> {
    if d.get(..4) != Some(b"\x7fELF") || d[4] != 2 || u(d, 18, 2) != 0xb7 {
        return Err("libil2cpp.so is not a 64-bit arm64 ELF".into());
    }
    let (shoff, ent, n) = (
        u(d, 0x28, 8) as usize,
        u(d, 0x3a, 2) as usize,
        u(d, 0x3c, 2) as usize,
    );
    Ok((0..n)
        .map(|i| {
            let o = shoff + i * ent;
            Sec {
                ty: u(d, o + 4, 4) as u32,
                flags: u(d, o + 8, 8),
                addr: u(d, o + 0x10, 8),
                off: u(d, o + 0x18, 8),
                size: u(d, o + 0x20, 8),
            }
        })
        .collect())
}

/// Page an `adrp` instruction at `pc` points to.
fn adrp_page(pc: u64, x: u32) -> Option<u64> {
    if x & 0x9f00_0000 != 0x9000_0000 {
        return None;
    }
    let imm = ((((x >> 5) & 0x7ffff) << 2 | (x >> 29) & 3) as i64) << 43 >> 43;
    Some((pc & !0xfff).wrapping_add((imm << 12) as u64))
}

/// IL2CPP's lazily resolved metadata slots start as `(kind << 29) | (index << 1) | 1`; kind 5 is a string literal.
fn literal_index(v: u64) -> Option<usize> {
    (v < 1 << 32 && v & 1 == 1 && v >> 29 == 5).then_some(((v & 0x1fff_ffff) >> 1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adrp() {
        // adrp x0 at 0x12719f0 in a real game, +0x1c5a pages
        assert_eq!(adrp_page(0x12719f0, 0xd000e2c0), Some(0x2ecb000));
        assert_eq!(adrp_page(0x1000, 0x91182000), None); // add, not adrp
    }

    #[test]
    fn literal_slots() {
        assert_eq!(literal_index(0xa000_8233), Some(16665));
        assert_eq!(literal_index(0x8000_0003), None); // kind 4: a method, not a string
        assert_eq!(literal_index(0x2d72c90), None); // a plain pointer
    }
}
