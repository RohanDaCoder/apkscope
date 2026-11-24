//! Editing commands: rename, comment, smali, res, verify.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use apkscope_core::smali::Printer;

use crate::cli::{self, NoteArgs, ResArgs, SmaliArgs, VerifyArgs};

pub fn rename(a: &NoteArgs) -> Result<(), String> {
    note(a, false)
}

pub fn comment(a: &NoteArgs) -> Result<(), String> {
    note(a, true)
}

fn note(a: &NoteArgs, is_comment: bool) -> Result<(), String> {
    let mut n = crate::re::load_notes(&a.apk)?;
    let text = a.text.join(" ");
    let key = match a
        .target
        .split_once('#')
        .or_else(|| a.target.split_once("->"))
    {
        Some((c, m)) => format!("{}->{m}", crate::re::desc(c)),
        None => crate::re::desc(&a.target),
    };
    crate::re::apply_note(&mut n, &key, is_comment, &text);
    crate::re::save_notes(&a.apk, &n)?;
    crate::note!(
        "saved {} ({key})",
        crate::re::project_file(&a.apk).display()
    );
    Ok(())
}

pub fn smali(a: &SmaliArgs) -> Result<(), String> {
    if let Some(dir) = &a.out_dir {
        return smali_dir(&a.apk, &a.classes, dir);
    }
    let [class] = a.classes.as_slice() else {
        return Err(
            "give one class to print, or --out-dir DIR to write several (or every class when none is given)"
                .into(),
        );
    };
    apkscope_core::smali::export_class(&a.apk, class)
        .map_err(String::from)
        .and_then(|t| cli::emit(&t))
}

/// The APK's DEX files, ready to print Smali from, in load order.
fn printers(apk: &str) -> Result<Vec<(String, Printer)>, String> {
    let dex = apkscope_core::apk::open(apk)
        .and_then(|mut z| z.dex())
        .map_err(String::from)?;
    dex.into_iter()
        .map(|(name, bytes)| {
            Printer::new(bytes)
                .map(|p| (name.clone(), p))
                .map_err(|e| format!("{name}: {e}"))
        })
        .collect()
}

/// Each class's (DEX file, class index), by descriptor.
type Defined = HashMap<String, (usize, usize)>;

/// Where each class is defined, by descriptor: (file, class index). The first file in load order
/// wins, as Android resolves it; the second value counts the later duplicates left out.
fn defined(dex: &[(String, Printer)]) -> Result<(Defined, usize), String> {
    let mut at = HashMap::new();
    let mut dups = 0;
    for (k, (name, p)) in dex.iter().enumerate() {
        for i in 0..p.len() {
            let d = p.descriptor(i).map_err(|e| format!("{name}: {e}"))?;
            match at.entry(d) {
                Entry::Vacant(v) => {
                    v.insert((k, i));
                }
                Entry::Occupied(_) => dups += 1,
            }
        }
    }
    Ok((at, dups))
}

/// The folder a DEX file's classes go to, as apktool names it: smali/ for classes.dex,
/// smali_classes2/ for classes2.dex.
fn smali_folder(dex: &str) -> String {
    match dex.strip_suffix(".dex") {
        Some("classes") | None => "smali".into(),
        Some(stem) => format!("smali_{}", stem.replace('/', "_")),
    }
}

/// One path segment, safe as a file name on every system: no characters Windows refuses, no
/// device name (con, nul, com1...), no trailing dot or space. `build` reads a class's name from
/// its text, so the file name only has to be unique.
fn safe_segment(p: &str) -> String {
    let mut s: String = p
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*' => '_',
            c if c < ' ' => '_',
            c => c,
        })
        .collect();
    if s.is_empty() || s.ends_with('.') || s.ends_with(' ') {
        s.push('_');
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_lowercase();
    let device = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.as_bytes()[3].is_ascii_digit());
    if device {
        s.insert(0, '_');
    }
    s
}

/// `folder/a/b/C.smali` for `La/b/C;`, unique without regard to case (obfuscated `a` and `A`
/// would be one file on Windows and macOS): a clash gets `C~2.smali`.
fn smali_path(folder: &str, desc: &str, used: &mut HashSet<String>) -> String {
    let inner = desc
        .strip_prefix('L')
        .and_then(|d| d.strip_suffix(';'))
        .unwrap_or(desc);
    let mut parts: Vec<String> = inner.split('/').map(safe_segment).collect();
    let file = parts.pop().unwrap_or_default();
    parts.insert(0, folder.to_string());
    let dir = parts.join("/");
    for n in 1.. {
        let path = match n {
            1 => format!("{dir}/{file}.smali"),
            n => format!("{dir}/{file}~{n}.smali"),
        };
        if used.insert(path.to_lowercase()) {
            return path;
        }
    }
    unreachable!("some suffix is free")
}

/// `apkscope smali --out-dir`: `classes` (every class when empty) as one file each, on all cores.
fn smali_dir(apk: &str, classes: &[String], out: &str) -> Result<(), String> {
    let t = std::time::Instant::now();
    let (n, dups) = write_smali(apk, classes, out)?;
    let skipped = match (classes.is_empty(), dups) {
        (true, 1..) => format!(
            " ({dups} duplicate definitions in later DEX files left out, as Android ignores them)"
        ),
        _ => String::new(),
    };
    crate::note!(
        "wrote {n} classes to {out} in {:.1}s{skipped}; edit, then: apkscope build {apk} out.apk --smali {out}",
        t.elapsed().as_secs_f64()
    );
    Ok(())
}

/// `classes` (every class when empty) of `apk` as one `.smali` file each under `out`, laid out as
/// apktool does; how many were written, and how many later duplicate definitions were left out.
pub(crate) fn write_smali(
    apk: &str,
    classes: &[String],
    out: &str,
) -> Result<(usize, usize), String> {
    let dex = printers(apk)?;
    let (at, dups) = defined(&dex)?;
    let mut jobs: Vec<(usize, usize, String)> = Vec::new();
    let mut used = HashSet::new();
    if classes.is_empty() {
        let mut all: Vec<(&String, &(usize, usize))> = at.iter().collect();
        all.sort_by_key(|&(_, &(k, i))| (k, i));
        for (desc, &(k, i)) in all {
            jobs.push((k, i, smali_path(&smali_folder(&dex[k].0), desc, &mut used)));
        }
    } else {
        for c in classes {
            let desc = apkscope_core::dex::descriptor(c);
            let &(k, i) = at
                .get(&desc)
                .ok_or_else(|| format!("class not found: {c}"))?;
            jobs.push((k, i, smali_path(&smali_folder(&dex[k].0), &desc, &mut used)));
        }
    }
    let root = std::path::Path::new(out);
    // each folder once, not a check per file: on Windows every file system call is slow
    let dirs: std::collections::BTreeSet<&str> = jobs
        .iter()
        .filter_map(|(_, _, p)| p.rsplit_once('/').map(|d| d.0))
        .collect();
    for d in dirs {
        let dir = root.join(d);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let errors: Vec<String> = crate::quick::par(jobs.len(), |j| {
        let (k, i, path) = &jobs[j];
        let to = root.join(path);
        let text = dex[*k].1.smali(*i).map_err(|e| format!("{path}: {e}"))?;
        std::fs::write(&to, text).map_err(|e| format!("{}: {e}", to.display()))
    })
    .into_iter()
    .filter_map(Result::err)
    .collect();
    if let Some(first) = errors.first() {
        return Err(format!(
            "{} of {} classes not written; first: {first}",
            errors.len(),
            jobs.len()
        ));
    }
    Ok((jobs.len(), dups))
}

/// The `.smali` files under `dir`, recursively, sorted.
fn smali_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in rd {
        let p = e.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if p.is_dir() {
            smali_files(&p, out)?;
        } else if p
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("smali"))
        {
            out.push(p);
        }
    }
    Ok(())
}

/// The Smali texts `build --smali` applies, from files and folders. A file is always taken; from
/// a folder (as `smali --out-dir` writes it) only the classes whose text differs from the APK's
/// own, or that the APK lacks, so a whole-app folder costs one compare per class and holds in
/// memory only what was edited. Deleting a file there does not remove its class.
pub(crate) fn smali_sources(apk: &str, paths: &[String]) -> Result<Vec<String>, String> {
    let mut texts = Vec::new();
    let mut in_dirs = Vec::new();
    for p in paths {
        let path = std::path::Path::new(p);
        if path.is_dir() {
            let before = in_dirs.len();
            smali_files(path, &mut in_dirs)?;
            if in_dirs.len() == before {
                return Err(format!("{p}: no .smali files in this folder"));
            }
        } else {
            texts.push(std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?);
        }
    }
    if in_dirs.is_empty() {
        return Ok(texts);
    }
    in_dirs.sort();
    let dex = printers(apk)?;
    let (at, _) = defined(&dex)?;
    let changed: Vec<Result<Option<String>, String>> = crate::quick::par(in_dirs.len(), |j| {
        let f = &in_dirs[j];
        let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let name = apkscope_core::smali::text::class_name(&text)
            .map_err(|e| format!("{}: {e}", f.display()))?;
        let same = match at.get(&apkscope_core::dex::descriptor(&name)) {
            Some(&(k, i)) => dex[k].1.smali(i).is_ok_and(|own| own == text),
            None => false,
        };
        Ok((!same).then_some(text))
    });
    let total = in_dirs.len();
    let mut n = 0;
    for c in changed {
        if let Some(text) = c? {
            texts.push(text);
            n += 1;
        }
    }
    crate::note!("{n} of {total} .smali files in the folder(s) differ from the APK");
    Ok(texts)
}

/// `apkscope res`: the manifest and `res/` of an APK as files, for `build --res`.
fn res_cmd(apk: &str, out: &str) -> Result<(), String> {
    let mut a = crate::export::Apk::new(crate::input::Input::open(apk)?)?;
    let files = apkscope_core::resources::decode(&mut a)?;
    for (path, data) in &files {
        let to = std::path::Path::new(out).join(path);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&to, data).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    crate::note!("wrote {} file(s) to {out}", files.len());
    Ok(())
}

pub fn res(a: &ResArgs) -> Result<(), String> {
    res_cmd(&a.apk, &a.out)
}

pub fn verify(a: &VerifyArgs) -> Result<(), String> {
    apkscope_core::apksign::verify(&a.apk)
        .map(|lines| lines.iter().for_each(|l| println!("{l}")))
        .map_err(String::from)
}

#[cfg(test)]
#[path = "../../tests/apkscope/commands_edit.rs"]
mod tests;
