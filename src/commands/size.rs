//! `size`: where an APK's bytes go, read from the zip directory alone (nothing is inflated).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use crate::cli::{self, SizeArgs};
use crate::session;

struct Entry {
    name: String,
    packed: u64,
    size: u64,
    crc: u32,
}

fn kb(n: u64) -> String {
    if n >= 10 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else {
        format!("{} KB", n.div_ceil(1024))
    }
}

/// Group an entry for the breakdown: dex, native lib per ABI, res, assets, other.
fn group(name: &str) -> String {
    if name.starts_with("classes") && name.ends_with(".dex") {
        "dex".into()
    } else if let Some(rest) = name.strip_prefix("lib/") {
        format!("lib/{}", rest.split('/').next().unwrap_or(""))
    } else if name == "resources.arsc" {
        "resources.arsc".into()
    } else if name.starts_with("res/") {
        "res".into()
    } else if name.starts_with("assets/") {
        "assets".into()
    } else if name.starts_with("META-INF/") {
        "META-INF".into()
    } else {
        "other".into()
    }
}

pub fn size(a: &SizeArgs) -> Result<(), String> {
    let real = session::prepare(&a.apk)?;
    let f = std::fs::File::open(&real).map_err(|e| format!("{}: {e}", a.apk))?;
    let total = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", a.apk))?;
    let mut all = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        let e = z.by_index_raw(i).map_err(|e| e.to_string())?;
        if e.is_dir() {
            continue;
        }
        all.push(Entry {
            name: e.name().map(|n| n.to_string()).unwrap_or_default(),
            packed: e.compressed_size(),
            size: e.size(),
            crc: e.crc32(),
        });
    }
    let json = cli::out_opts().json;
    // by group
    let mut groups: BTreeMap<String, (u64, u64, usize)> = BTreeMap::new();
    for e in &all {
        let g = groups.entry(group(&e.name)).or_default();
        g.0 += e.packed;
        g.1 += e.size;
        g.2 += 1;
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.1.0));
    // duplicates: same CRC and size (from the zip directory, no hashing)
    let mut same: HashMap<(u32, u64), Vec<&Entry>> = HashMap::new();
    for e in all.iter().filter(|e| e.size > 0) {
        same.entry((e.crc, e.size)).or_default().push(e);
    }
    let mut dups: Vec<Vec<&Entry>> = same.into_values().filter(|v| v.len() > 1).collect();
    // wasted bytes = every copy but one
    dups.sort_by_key(|v| std::cmp::Reverse(v[0].packed * (v.len() as u64 - 1)));
    let mut biggest: Vec<&Entry> = all.iter().collect();
    biggest.sort_by_key(|e| std::cmp::Reverse(e.packed));
    if json {
        let v = serde_json::json!({
            "file_bytes": total,
            "entries": all.len(),
            "groups": groups.iter().map(|(g, (p, s, n))| serde_json::json!({"group": g, "packed": p, "size": s, "files": n})).collect::<Vec<_>>(),
            "top": biggest.iter().take(a.top).map(|e| serde_json::json!({"name": e.name, "packed": e.packed, "size": e.size})).collect::<Vec<_>>(),
            "duplicates": dups.iter().map(|v| serde_json::json!({"packed_each": v[0].packed, "files": v.iter().map(|e| &e.name).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        });
        return cli::emit(&format!("{v:#}\n"));
    }
    let mut o = String::new();
    let _ = writeln!(o, "// {}  {}, {} entries", a.apk, kb(total), all.len());
    let _ = writeln!(o, "\n// by kind (compressed / uncompressed, files)");
    for (g, (p, s, n)) in &groups {
        let pct = if total > 0 {
            *p as f64 * 100.0 / total as f64
        } else {
            0.0
        };
        let _ = writeln!(
            o,
            "{g:<24}{:>10} {pct:>5.1}%   {:>10}   {n}",
            kb(*p),
            kb(*s)
        );
    }
    let _ = writeln!(
        o,
        "\n// biggest {} entries (compressed / uncompressed)",
        a.top.min(all.len())
    );
    for e in biggest.iter().take(a.top) {
        let _ = writeln!(o, "{:>10} {:>10}   {}", kb(e.packed), kb(e.size), e.name);
    }
    let wasted: u64 = dups
        .iter()
        .map(|v| v[0].packed * (v.len() as u64 - 1))
        .sum();
    let _ = writeln!(
        o,
        "\n// duplicate files (same CRC and size): {} groups, {} could be saved",
        dups.len(),
        kb(wasted)
    );
    for v in dups.iter().take(a.top) {
        let _ = writeln!(o, "{:>10} x{}", kb(v[0].packed), v.len());
        for e in v {
            let _ = writeln!(o, "             {}", e.name);
        }
    }
    cli::emit(&o)
}
