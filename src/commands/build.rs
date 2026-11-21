//! `apkscope build`: apply edits, zipalign, sign v2 + v3, then verify the result.

use apkscope_core::{apksign, arsc, axml, rebuild, res, resources, smali, table};

use crate::cli::BuildArgs;
use crate::export;
use crate::session;

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
    if (jar || !sign) && (key.is_some() || cert.is_some()) {
        return Err("--key and --cert sign an APK: not a .jar, and not with --no-sign".into());
    }
    let key = match (key, cert) {
        _ if jar || !sign => None,
        (Some(k), Some(c)) => Some(apksign::load(
            std::path::Path::new(&k),
            std::path::Path::new(&c),
        )?),
        (None, None) => {
            let (k, dir) = apksign::default_key()?;
            eprintln!("signing with apkscope's key in {}", dir.display());
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
        let files = crate::read_tree(std::path::Path::new(dir))?;
        let mut apk = export::open(&src)?;
        let changed = resources::edits(&mut apk, &files, true)?;
        eprintln!("{} resource file(s) changed", changed.len());
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
        eprintln!(
            "assembled {} smali class(es) into {}",
            smali_srcs.len(),
            changed.join(", ")
        );
    }
    let (apk, cd) = rebuild::rebuild(&src, &edits)?;
    let signed = match &key {
        Some(key) => apksign::sign(apk, cd, key)?,
        None => apk,
    };
    std::fs::write(output, &signed).map_err(|e| format!("{output}: {e}"))?;
    eprintln!(
        "wrote {output} ({} bytes, {} edits) in {:.1}s",
        signed.len(),
        edits.len(),
        t.elapsed().as_secs_f64()
    );
    if key.is_some() {
        for l in apksign::verify(output)? {
            eprintln!("{l}");
        }
    }
    Ok(())
}
