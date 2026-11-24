//! `apkscope build`: apply edits, zipalign, sign v2 + v3, then verify the result.

use apkscope_core::{apksign, arsc, axml, rebuild, res, resources, smali, table};

use crate::cli::BuildArgs;
use crate::export;
use crate::session;

/// A password from `pass:TEXT`, `env:VAR` or `file:PATH` (first line), else the text itself
/// (the forms apksigner takes). `env:` / `file:` keep it out of shell history and process lists.
fn password(spec: &str) -> Result<String, String> {
    if let Some(t) = spec.strip_prefix("pass:") {
        Ok(t.to_string())
    } else if let Some(v) = spec.strip_prefix("env:") {
        std::env::var(v).map_err(|_| format!("environment variable {v} is not set"))
    } else if let Some(f) = spec.strip_prefix("file:") {
        let t = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
        Ok(t.lines().next().unwrap_or("").to_string())
    } else {
        Ok(spec.to_string())
    }
}

/// `--no-sign` leaves the APK unsigned; a JAR never is signed, Android does not verify one.
pub fn build(a: &BuildArgs) -> Result<(), String> {
    let input = a.input.as_str();
    let output = a.output.as_str();
    let mut edits = rebuild::Edits::new();
    for v in &a.put {
        let (name, file) = v.split_once('=').ok_or("--put needs name=file")?;
        edits.insert(
            name.to_string(),
            Some(std::fs::read(file).map_err(|e| format!("{file}: {e}"))?),
        );
    }
    for name in &a.delete {
        edits.insert(name.clone(), None);
    }
    let sign = !a.no_sign;
    let mut strings = std::collections::BTreeMap::new();
    for v in &a.string {
        let (k, text) = v.split_once('=').ok_or("--string needs name=value")?;
        let (name, config) = k.split_once('@').unwrap_or((k, ""));
        strings.insert((config.to_string(), name.to_string()), text.to_string());
    }
    let smali_srcs: Vec<String> = a
        .smali
        .iter()
        .map(|f| std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}")))
        .collect::<Result<_, _>>()?;
    let res_dir = a.res.clone();
    let manifest = a.manifest.clone();
    let key = a.key.clone();
    let cert = a.cert.clone();
    let jar = input.to_lowercase().ends_with(".jar");
    if (jar || !sign) && (key.is_some() || cert.is_some() || a.ks.is_some()) {
        return Err("--key/--cert/--ks sign an APK: not a .jar, and not with --no-sign".into());
    }
    if a.ks.is_some() && (key.is_some() || cert.is_some()) {
        return Err("use either --ks or --key/--cert".into());
    }
    if a.ks.is_none() && (a.ks_pass.is_some() || a.ks_key_pass.is_some() || a.ks_alias.is_some()) {
        return Err("--ks-pass, --ks-key-pass and --ks-alias go with --ks".into());
    }
    let key = match (key, cert) {
        _ if jar || !sign => None,
        _ if a.ks.is_some() => {
            let ks = a.ks.as_deref().unwrap_or_default();
            let store = password(a.ks_pass.as_deref().ok_or("--ks needs --ks-pass")?)?;
            let key_pass = a.ks_key_pass.as_deref().map(password).transpose()?;
            Some(apksign::load_keystore(
                std::path::Path::new(ks),
                &store,
                key_pass.as_deref(),
                a.ks_alias.as_deref(),
            )?)
        }
        (Some(k), Some(c)) => Some(apksign::load(
            std::path::Path::new(&k),
            std::path::Path::new(&c),
        )?),
        (None, None) => {
            let (k, dir) = apksign::default_key()?;
            crate::note!("signing with apkscope's key in {}", dir.display());
            Some(k)
        }
        _ => return Err("--key and --cert go together".into()),
    };
    let t = std::time::Instant::now();
    let src = session::prepare(input)?;
    if res_dir.is_some() && (manifest.is_some() || !strings.is_empty()) {
        return Err(
            "--res has the manifest and the strings: not with --manifest or --string".into(),
        );
    }
    if let Some(dir) = &res_dir {
        // everything `apkscope res` wrote: a values file that is gone has no values
        let (files, complete, kind) = foreign_tree(crate::read_tree(std::path::Path::new(dir))?);
        if let Some(kind) = kind {
            crate::note!(
                "{dir}: {kind} layout; changed and new files are applied, files missing from it are kept"
            );
        }
        let mut apk = export::open(&src)?;
        let changed = resources::edits(&mut apk, &files, complete)?;
        crate::note!("{} resource file(s) changed", changed.len());
        edits.extend(changed);
    }
    if let Some(m) = &manifest {
        // text manifest -> binary XML, typed like the original
        let text = std::fs::read_to_string(m).map_err(|e| format!("{m}: {e}"))?;
        let mut a = export::open(&src)?;
        let table = a.read("resources.arsc").and_then(|d| table::read(&d).ok());
        let names = table
            .as_ref()
            .map(res::Names::new)
            .ok_or("no resources.arsc to resolve @references")?;
        let bin = axml::encode(&text, a.read("AndroidManifest.xml").as_deref(), &names)
            .map_err(|e| format!("{m}: {e}"))?;
        edits.insert("AndroidManifest.xml".into(), Some(bin));
    }
    if !strings.is_empty() {
        let table = export::open(&src)?
            .read("resources.arsc")
            .ok_or("no resources.arsc")?;
        edits.insert(
            "resources.arsc".into(),
            Some(arsc::set_strings(&table, &strings)?),
        );
    }
    if !smali_srcs.is_empty() {
        let changed = smali::inject(&src, &smali_srcs, &mut edits)?;
        crate::note!(
            "assembled {} smali class(es) into {}",
            smali_srcs.len(),
            changed.join(", ")
        );
    }
    let size = export::write_apk(&src, &edits, std::path::Path::new(output), key.as_ref())?;
    crate::note!(
        "wrote {output} ({size} bytes, {} edits) in {:.1}s",
        edits.len(),
        t.elapsed().as_secs_f64()
    );
    if let Some(k) = &key {
        for l in apksign::verify(output)? {
            crate::note!("{l}");
        }
        if let Some(c) = apkscope_core::sign::parse_cert(&k.cert) {
            crate::note!("signer: {}  (SHA-256 {})", c.subject, c.sha256);
        }
        match export::min_sdk(&src, &edits) {
            Some(m) if m >= 24 => crate::note!("signed v2 + v3 (minSdkVersion {m}: no v1 needed)"),
            m => crate::note!(
                "signed v1 + v2 + v3 (minSdkVersion {}: Android 5-6 check the v1 JAR signature)",
                m.map_or("unset".into(), |m| m.to_string())
            ),
        }
    }
    Ok(())
}

type Tree = std::collections::BTreeMap<String, Vec<u8>>;

/// A resource folder from another tool, mapped onto the layout `apkscope res` writes
/// (`AndroidManifest.xml`, `res/...`): apktool (`apktool.yml` + `res/`) or APKEditor
/// (`resources/package_1/res/`). Returns (files, complete, the tool's name). Another tool's
/// folder is never taken as complete, so a file it leaves out is not removed from the APK;
/// its `public.xml` (an id listing, not an edit) is ignored.
fn foreign_tree(files: Tree) -> (Tree, bool, Option<&'static str>) {
    let keep = |k: &str| {
        k == "AndroidManifest.xml" || (k.starts_with("res/") && k != "res/values/public.xml")
    };
    if files.contains_key("apktool.yml") {
        let out = files.into_iter().filter(|(k, _)| keep(k)).collect();
        return (out, false, Some("apktool"));
    }
    const EDITOR: &str = "resources/package_1/";
    if files.keys().any(|k| k.starts_with(EDITOR)) {
        let out = files
            .into_iter()
            .filter_map(|(k, v)| {
                let k = k.strip_prefix(EDITOR).map(str::to_string).unwrap_or(k);
                keep(&k).then_some((k, v))
            })
            .collect();
        return (out, false, Some("APKEditor"));
    }
    (files, true, None)
}
