//! `apkscope patch`: apply an XML patch bundle to an APK, then rebuild and sign.

use apkscope_core::{apksign, arsc, axml, dex, rebuild, res, smali, table};
use std::collections::BTreeMap;

use crate::cli::PatchArgs;
use crate::export;
use crate::patch::{self, Bundle, Op, Patch};
use crate::session;

/// Apply one bundle. Everything is collected before the APK is rewritten, so a bundle that
/// fails anywhere — a class that is not there, a `from` that no method has — changes nothing.
pub fn patch(a: &PatchArgs) -> Result<(), String> {
    let text = std::fs::read_to_string(&a.bundle).map_err(|e| format!("{}: {e}", a.bundle))?;
    let bundle = patch::parse(&text).map_err(|e| format!("{}: {e}", a.bundle))?;
    let chosen = select(&bundle, &a.only, &a.skip)?;
    if a.list {
        return list(&bundle, &chosen);
    }
    let (Some(input), Some(output)) = (&a.input, &a.output) else {
        return Err("give the APK to patch and where to write it (or --list)".into());
    };
    let t = std::time::Instant::now();
    let src = session::prepare(input)?;
    if let Some(target) = &bundle.target {
        check_target(target, &src)?;
    }
    let jar = input.to_lowercase().ends_with(".jar");
    let key = super::build::sign_key(jar, !a.sign.no_sign, &a.sign)?;
    // file paths in the bundle are relative to the bundle itself; absolute paths stay as they are
    let dir = std::path::Path::new(&a.bundle)
        .parent()
        .map_or_else(std::path::PathBuf::new, ToOwned::to_owned);
    let mut edits = rebuild::Edits::new();
    let mut strings = BTreeMap::new();
    let mut manifest = None;
    let mut classes: Option<smali::Classes> = None;
    let mut smali_texts: BTreeMap<String, String> = BTreeMap::new();
    let chosen_len = chosen.len();
    for p in chosen {
        for op in &p.ops {
            match op {
                Op::Return { .. } | Op::Replace { .. } | Op::Body { .. } => {
                    let note = format!("{}: {}", p.name, where_(op));
                    let msg = code_op(op, &note, &mut classes, &mut smali_texts, &src)?;
                    crate::note!("{note}: {msg}");
                }
                Op::Any { ops, line } => {
                    // one at a time until one lands: a miss has cost nothing, the class text
                    // is only committed by an op that succeeds
                    let note = format!("{}: <any> (line {line})", p.name);
                    let mut missed = Vec::new();
                    let mut landed = None;
                    for (i, alt_op) in ops.iter().enumerate() {
                        let alt = format!("{note}, alternative {} of {}", i + 1, ops.len());
                        match code_op(alt_op, &alt, &mut classes, &mut smali_texts, &src) {
                            Ok(msg) => {
                                landed = Some((i + 1, msg));
                                break;
                            }
                            Err(e) => missed.push(e),
                        }
                    }
                    match landed {
                        Some((i, msg)) => crate::note!("{note}: alternative {i} landed: {msg}"),
                        None => {
                            return Err(format!(
                                "{note}: no alternative landed:\n  {}",
                                missed.join("\n  ")
                            ));
                        }
                    }
                }
                Op::Str {
                    name,
                    config,
                    value,
                } => {
                    strings.insert(
                        (config.clone().unwrap_or_default(), name.clone()),
                        value.clone(),
                    );
                }
                Op::Manifest { file } => manifest = Some(dir.join(file)),
                Op::Entry {
                    name,
                    src: from,
                    delete,
                } => match (from, delete) {
                    (Some(f), false) => {
                        let f = dir.join(f);
                        let data =
                            std::fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                        edits.insert(name.clone(), Some(data));
                    }
                    (None, true) => {
                        edits.insert(name.clone(), None);
                    }
                    // the other two combinations are refused when the bundle is parsed
                    _ => {
                        return Err(format!(
                            "{}: <entry {name}> needs src=... or delete=yes",
                            p.name
                        ));
                    }
                },
            }
        }
    }
    if let Some(classes) = &mut classes {
        for (class, text) in &smali_texts {
            classes
                .set_text(text)
                .map_err(|e| format!("{class}: {e}"))?;
        }
        let changed = classes.write(&mut edits)?;
        crate::note!(
            "assembled {} edited class(es) into {}",
            smali_texts.len(),
            changed.join(", ")
        );
    }
    if !strings.is_empty() {
        let table = export::open(&src)?
            .read("resources.arsc")
            .ok_or("no resources.arsc: <string> edits need one")?;
        edits.insert(
            "resources.arsc".into(),
            Some(arsc::set_strings(&table, &strings)?),
        );
    }
    if let Some(m) = &manifest {
        // text manifest -> binary XML, typed like the original
        let text = std::fs::read_to_string(m).map_err(|e| format!("{}: {e}", m.display()))?;
        let mut apk = export::open(&src)?;
        let t = apk
            .read("resources.arsc")
            .and_then(|d| table::read(&d).ok());
        let names = t
            .as_ref()
            .map(res::Names::new)
            .ok_or("no resources.arsc to resolve @references")?;
        let bin = axml::encode(&text, apk.read("AndroidManifest.xml").as_deref(), &names)
            .map_err(|e| format!("{}: {e}", m.display()))?;
        edits.insert("AndroidManifest.xml".into(), Some(bin));
    }
    let size = export::write_apk(&src, &edits, std::path::Path::new(output), key.as_ref())?;
    crate::note!(
        "wrote {output} ({} bytes, {} edits, {} of {} patch(es)) in {:.1}s",
        size,
        edits.len(),
        chosen_len,
        bundle.patches.len(),
        t.elapsed().as_secs_f64()
    );
    if let Some(k) = &key {
        for l in apksign::verify(output)? {
            crate::note!("{l}");
        }
        if let Some(c) = apkscope_core::sign::parse_cert(&k.cert) {
            crate::note!("signer: {}  (SHA-256 {})", c.subject, c.sha256);
        }
    }
    Ok(())
}

/// One code op — `<return>`, `<replace>` or `<body>` — applied to the class's smali text.
/// The text is only committed when the op lands, so what the caller gets back on an error
/// is a class exactly as it was: an `<any>` alternative that misses leaves nothing behind.
fn code_op(
    op: &Op,
    note: &str,
    classes: &mut Option<smali::Classes>,
    smali: &mut BTreeMap<String, String>,
    src: &str,
) -> Result<String, String> {
    let (class, method, sig) = match op {
        Op::Return {
            class, method, sig, ..
        }
        | Op::Replace {
            class, method, sig, ..
        }
        | Op::Body {
            class, method, sig, ..
        } => (class, method, sig),
        _ => return Err(format!("{note}: not a code op")),
    };
    let at = patch::At {
        class: class.clone(),
        method: method.clone(),
        sig: sig.clone(),
        note: note.to_string(),
    };
    let text = class_text(classes, smali, class, src).map_err(|e| format!("{note}: {e}"))?;
    let mut trial = text.clone();
    let msg = match op {
        Op::Return { value, .. } => {
            let n = patch::returns(&mut trial, &at, value.as_deref())?;
            format!("{n} method(s) stubbed")
        }
        Op::Replace { from, to, once, .. } => {
            let n = patch::replaces(&mut trial, &at, from, to, *once)?;
            format!("{n} line(s) changed")
        }
        Op::Body { body, .. } => {
            let n = patch::bodies(&mut trial, &at, body)?;
            format!("{n} body(ies) replaced")
        }
        _ => unreachable!("the match above took the only ops that reach here"),
    };
    *text = trial;
    Ok(msg)
}

/// The `<target>` against the APK's own manifest: the one place a bundle says what it is for.
fn check_target(target: &patch::Target, src: &str) -> Result<(), String> {
    let data = crate::quick::read_entry(src, "AndroidManifest.xml")
        .ok()
        .flatten()
        .ok_or("no AndroidManifest.xml: the bundle's <target> cannot be checked")?;
    let text = res::decode_xml(&data, None)?;
    let (package, name, code) = patch::manifest_id(&text);
    target.matches(package.as_deref(), name.as_deref(), code)
}

/// The patches to apply: `--only` names them, `--skip` drops them; both name real patches, so
/// a typo cannot quietly deselect half a bundle.
fn select<'a>(
    bundle: &'a Bundle,
    only: &[String],
    skip: &[String],
) -> Result<Vec<&'a Patch>, String> {
    let known = |names: &[String], what: &str| {
        names.iter().try_for_each(|n| {
            bundle
                .patches
                .iter()
                .any(|p| &p.name == n)
                .then_some(())
                .ok_or_else(|| format!("no patch named {n:?} to {what}"))
        })
    };
    known(only, "apply")?;
    known(skip, "skip")?;
    Ok(bundle
        .patches
        .iter()
        .filter(|p| only.is_empty() || only.contains(&p.name))
        .filter(|p| !skip.contains(&p.name))
        .collect())
}

/// `--list`: the bundle, its target and its patches, with the chosen ones marked.
fn list(bundle: &Bundle, chosen: &[&Patch]) -> Result<(), String> {
    let mut out = String::new();
    let meta = [
        bundle.name.as_deref().map(|v| format!("name: {v}")),
        bundle.author.as_deref().map(|v| format!("author: {v}")),
        bundle.version.as_deref().map(|v| format!("version: {v}")),
        bundle.description.as_deref().map(str::to_string),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    if !meta.is_empty() {
        out.push_str(&meta);
        out.push_str("\n\n");
    }
    if let Some(t) = &bundle.target {
        let mut line = format!("target: package {}", t.package);
        if let Some(v) = &t.version_name {
            line.push_str(&format!(", versionName {v}"));
        }
        for (v, cmp) in [(t.min_version_code, ">="), (t.max_version_code, "<=")] {
            if let Some(v) = v {
                line.push_str(&format!(", versionCode {cmp} {v}"));
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    for p in &bundle.patches {
        out.push_str(&format!(
            "{}{} ({} op(s))",
            match chosen.iter().any(|c| c.name == p.name) {
                true => "[x] ",
                false => "[ ] ",
            },
            p.name,
            p.ops.len()
        ));
        if let Some(d) = &p.description {
            out.push_str(&format!(" - {d}"));
        }
        out.push('\n');
    }
    crate::cli::emit(&out)
}

/// How an op is named in notes and errors.
fn where_(op: &Op) -> String {
    match op {
        Op::Return {
            class,
            method,
            line,
            ..
        } => {
            format!("<return {class}#{method}> (line {line})")
        }
        Op::Replace {
            class,
            method,
            line,
            ..
        } => {
            format!("<replace {class}#{method}> (line {line})")
        }
        Op::Body {
            class,
            method,
            line,
            ..
        } => {
            format!("<body {class}#{method}> (line {line})")
        }
        Op::Any { line, .. } => format!("<any> (line {line})"),
        Op::Str { name, .. } => format!("<string {name}>"),
        Op::Manifest { file } => format!("<manifest {file}>"),
        Op::Entry { name, .. } => format!("<entry {name}>"),
    }
}

/// The class's smali as edited so far: printed from the DEX once, then kept for the next op on
/// the same class. The DEX files only open for editing when a bundle edits code.
fn class_text<'a>(
    classes: &mut Option<smali::Classes>,
    smali: &'a mut BTreeMap<String, String>,
    class: &str,
    src: &str,
) -> Result<&'a mut String, String> {
    // `a.b.C`, `a/b/C` and `La/b/C;` are the same class: the cache is keyed by the descriptor
    // so two ops that spell the class differently edit one text, instead of printing two and
    // letting the second set_text quietly drop the first's edits
    match smali.entry(dex::descriptor(class)) {
        std::collections::btree_map::Entry::Occupied(e) => Ok(e.into_mut()),
        std::collections::btree_map::Entry::Vacant(e) => {
            let print = |c: &mut smali::Classes| {
                let (doc, model) = c.class(class)?;
                Ok::<String, apkscope_core::Error>(smali::text::print(doc.source(), &model))
            };
            let text = match classes {
                Some(c) => print(c)?,
                None => {
                    let mut c = smali::Classes::new(export::open(src)?.dex()?);
                    let text = print(&mut c)?;
                    *classes = Some(c);
                    text
                }
            };
            Ok(e.insert(text))
        }
    }
}

#[cfg(test)]
#[path = "../../tests/apkscope/commands_patch.rs"]
mod tests;
