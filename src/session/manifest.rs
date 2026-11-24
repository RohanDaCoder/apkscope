//! The manifest overview: package and versions, code share per origin, application
//! flags, permissions, exported components, deep links, the signature summary and
//! the native-library meanings.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::search::{LIBRARIES, Origin, attr, native_lib_meaning};

use super::State;

/// Classes and methods per code source, counted from the DEX: the app's own packages,
/// unknown (often obfuscated) code, and each known SDK prefix. Top counts first — the
/// "vendor noise vs mine" triage for big multi-dex apps.
pub fn sdk_share(st: &State) -> Vec<(String, usize, usize)> {
    let mut rows: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (k, (name, d, c)) in st.classes.iter().enumerate() {
        let group = match st.origin[k] {
            Origin::App => "app code".to_string(),
            Origin::Other => "other code".to_string(),
            Origin::Library => LIBRARIES
                .iter()
                .find(|p| name.starts_with(*p))
                .map_or("libraries".to_string(), |p| {
                    p.trim_end_matches('.').to_string()
                }),
        };
        let e = rows.entry(group).or_default();
        e.0 += 1;
        e.1 += st.dexes[*d].class_data(*c).map_or(0, |x| x.methods.len());
    }
    let mut out: Vec<(String, usize, usize)> =
        rows.into_iter().map(|(g, (c, m))| (g, c, m)).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Security-relevant summary of the manifest and APK layout, as readable text.
pub fn overview(st: &State) -> String {
    let m = st
        .texts()
        .get("AndroidManifest.xml")
        .map(String::as_str)
        .unwrap_or("");
    // elements as (tag, attributes text, depth-ordered); the decoder writes one element per `<`
    let elems: Vec<(&str, &str)> = m
        .split('<')
        .skip(1)
        .filter_map(|e| {
            let e = e.split('>').next()?;
            let tag = e.split_whitespace().next()?;
            Some((tag, e))
        })
        .collect();
    let a = |e: &str, n: &str| attr(e, &format!("android:{n}"));
    let mut o = String::new();
    let _ = writeln!(o, "// Overview of {}\n", st.path);
    let _ = writeln!(o, "package           {}", st.package);
    if let Some((_, e)) = elems.iter().find(|x| x.0 == "manifest") {
        let _ = writeln!(
            o,
            "version           {} ({})",
            a(e, "versionName").unwrap_or_default(),
            a(e, "versionCode").unwrap_or_default()
        );
    }
    if let Some((_, e)) = elems.iter().find(|x| x.0 == "uses-sdk") {
        let _ = writeln!(
            o,
            "min / target SDK  {} / {}",
            a(e, "minSdkVersion").unwrap_or("?".into()),
            a(e, "targetSdkVersion").unwrap_or("?".into())
        );
    }
    let _ = writeln!(
        o,
        "classes           {} top-level, {} methods, {} dex",
        st.classes.len(),
        st.methods(),
        st.dexes.len()
    );
    let _ = writeln!(o, "\n// code by origin (classes / methods), top first");
    for (g, c, m) in sdk_share(st).into_iter().take(12) {
        let _ = writeln!(o, "{g:<28}{c:>6} / {m}");
    }
    if let Some((_, e)) = elems.iter().find(|x| x.0 == "application") {
        let _ = writeln!(o, "\n// application flags");
        for f in [
            "debuggable",
            "allowBackup",
            "usesCleartextTraffic",
            "networkSecurityConfig",
            "requestLegacyExternalStorage",
            "name",
        ] {
            if let Some(v) = a(e, f) {
                let warn = matches!(
                    (f, v.as_str()),
                    ("debuggable", "true")
                        | ("usesCleartextTraffic", "true")
                        | ("allowBackup", "true")
                );
                let _ = writeln!(o, "{f:<29}{v}{}", if warn { "    <- review" } else { "" });
            }
        }
    }
    const DANGEROUS: [&str; 16] = [
        "READ_CONTACTS",
        "WRITE_CONTACTS",
        "READ_SMS",
        "SEND_SMS",
        "RECEIVE_SMS",
        "READ_CALL_LOG",
        "CALL_PHONE",
        "RECORD_AUDIO",
        "CAMERA",
        "ACCESS_FINE_LOCATION",
        "ACCESS_COARSE_LOCATION",
        "ACCESS_BACKGROUND_LOCATION",
        "READ_PHONE_STATE",
        "READ_EXTERNAL_STORAGE",
        "BODY_SENSORS",
        "REQUEST_INSTALL_PACKAGES",
    ];
    let perms: Vec<String> = elems
        .iter()
        .filter(|x| x.0.starts_with("uses-permission"))
        .filter_map(|x| a(x.1, "name"))
        .collect();
    let _ = writeln!(o, "\n// permissions ({})", perms.len());
    for p in &perms {
        let flag = DANGEROUS.iter().any(|d| p.ends_with(d));
        let _ = writeln!(o, "{p}{}", if flag { "    <- dangerous" } else { "" });
    }
    // components: exported explicitly, or implicitly through an intent-filter
    let _ = writeln!(o, "\n// exported components (reachable by other apps)");
    let mut deep = Vec::new();
    // (tag, name, exported attribute, has an intent-filter)
    let mut cur: Option<(String, String, Option<String>, bool)> = None;
    let flush = |c: Option<(String, String, Option<String>, bool)>, o: &mut String| {
        if let Some((tag, name, exp, filt)) = c {
            // before Android 12 a component with an intent-filter and no `exported` is exported
            match (exp.as_deref(), filt) {
                (Some("true"), _) => {
                    let _ = writeln!(o, "{tag:<10}{name}");
                }
                (None, true) => {
                    let _ = writeln!(o, "{tag:<10}{name}    (exported via intent-filter)");
                }
                _ => {}
            }
        }
    };
    for (tag, e) in &elems {
        match *tag {
            "activity" | "activity-alias" | "service" | "receiver" | "provider" => {
                flush(cur.take(), &mut o);
                cur = Some((
                    tag.to_string(),
                    a(e, "name").unwrap_or_default(),
                    a(e, "exported"),
                    false,
                ));
            }
            "intent-filter" => {
                if let Some(c) = cur.as_mut() {
                    c.3 = true;
                }
            }
            "data" => {
                let (s, h, p) = (
                    a(e, "scheme"),
                    a(e, "host"),
                    a(e, "path").or(a(e, "pathPrefix")).or(a(e, "pathPattern")),
                );
                if s.is_some() || h.is_some() {
                    deep.push(format!(
                        "{}://{}{}",
                        s.unwrap_or("*".into()),
                        h.unwrap_or("*".into()),
                        p.unwrap_or_default()
                    ));
                }
            }
            _ => {}
        }
    }
    flush(cur.take(), &mut o);
    deep.sort();
    deep.dedup();
    let _ = writeln!(o, "\n// deep links ({})", deep.len());
    for d in &deep {
        let _ = writeln!(o, "{d}");
    }
    let _ = writeln!(
        o,
        "
// signature"
    );
    o += &crate::sign::summary(&st.file, &st.entries, |p| st.read_entry(p));
    let libs: Vec<&String> = st
        .entries
        .iter()
        .filter(|e| e.starts_with("lib/") && e.ends_with(".so"))
        .collect();
    let _ = writeln!(o, "\n// native libraries ({}), with meanings", libs.len());
    for l in &libs {
        let _ = match native_lib_meaning(l) {
            Some(m) => writeln!(o, "{l}    {m}"),
            None => writeln!(o, "{l}"),
        };
    }
    let _ = writeln!(
        o,
        "\n// {}",
        crate::assets::summary(st).unwrap_or_else(|| "assets: none".into())
    );
    o
}
