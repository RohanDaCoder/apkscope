//! Editing commands: rename, comment, smali, res, verify.

use crate::cli::{self, NoteArgs, ResArgs, SmaliArgs, VerifyArgs};
use crate::session;

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
    eprintln!(
        "saved {} ({key})",
        crate::re::project_file(&a.apk).display()
    );
    Ok(())
}

pub fn smali(a: &SmaliArgs) -> Result<(), String> {
    apkscope_core::smali::export_class(&a.apk, &a.class)
        .map_err(String::from)
        .and_then(|t| cli::emit(&t))
}

/// `apkscope res`: the manifest and `res/` of an APK as files, for `build --res`.
fn res_cmd(apk: &str, out: &str) -> Result<(), String> {
    let mut a = crate::export::open(&session::prepare(apk)?)?;
    let files = apkscope_core::resources::decode(&mut a)?;
    for (path, data) in &files {
        let to = std::path::Path::new(out).join(path);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&to, data).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    eprintln!("wrote {} file(s) to {out}", files.len());
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
