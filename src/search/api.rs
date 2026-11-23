//! The app's declared HTTP API (Retrofit): endpoints read straight from the
//! annotations, base URLs and call-site arguments resolved to facts, one curl line
//! per endpoint. The `api` search scope's sheet.

use std::collections::BTreeMap;

use crate::dex;
use crate::index;
use crate::session::State;

use super::{Hit, LIMIT, Matcher, Origin};

/// One endpoint of a Retrofit interface, read straight from the annotations.
struct Endpoint {
    class: String,
    method: String,
    verb: String,
    path: String,
    /// (annotation kind, name when the annotation carries one, parameter type)
    params: Vec<(&'static str, Option<String>, String)>,
    /// raw return type, e.g. `Call` — generic parameters are erased in the DEX
    ret: String,
    dex: usize,
    method_idx: u32,
}

/// One endpoint's call sites: what its callers pass, and the classes they reference.
#[derive(Default)]
struct Callers {
    class: String,
    /// parameter name -> literal or resolved constant, from positional call arguments
    args: BTreeMap<String, String>,
    /// response-model candidates: classes the callers reference as `X.class`
    models: Vec<String>,
}

/// `I` -> `int`, `Ljava/lang/String;` -> `String`, `[B` -> `byte[]`
fn short_ty(t: &str) -> String {
    let base = t.trim_start_matches('[');
    let array = if base.len() != t.len() { "[]" } else { "" };
    let named = match base {
        "I" => "int".to_string(),
        "Z" => "boolean".to_string(),
        "B" => "byte".to_string(),
        "S" => "short".to_string(),
        "C" => "char".to_string(),
        "J" => "long".to_string(),
        "F" => "float".to_string(),
        "D" => "double".to_string(),
        "V" => "void".to_string(),
        _ => base
            .trim_start_matches('L')
            .trim_end_matches(';')
            .rsplit('/')
            .next()
            .unwrap_or(base)
            .to_string(),
    };
    format!("{named}{array}")
}

/// The Retrofit parameter annotations this reads; the second field says whether the
/// annotation names the parameter (`value = "key"`).
fn param_kind(t: &str) -> Option<(&'static str, bool)> {
    Some(match t {
        "Lretrofit2/http/Query;" => ("query", true),
        "Lretrofit2/http/QueryName;" => ("query name", false),
        "Lretrofit2/http/QueryMap;" => ("query map", false),
        "Lretrofit2/http/Path;" => ("path", true),
        "Lretrofit2/http/Field;" => ("field", true),
        "Lretrofit2/http/FieldMap;" => ("field map", false),
        "Lretrofit2/http/Body;" => ("body", false),
        "Lretrofit2/http/Header;" => ("header", true),
        "Lretrofit2/http/HeaderMap;" => ("header map", false),
        "Lretrofit2/http/Part;" => ("part", true),
        "Lretrofit2/http/PartMap;" => ("part map", false),
        "Lretrofit2/http/Url;" => ("url", false),
        _ => return None,
    })
}

/// Every Retrofit-annotated interface method: verb, path, parameters, return type.
/// Discovery runs on the reference index (the `users` of the annotation types), so no
/// class is decompiled to build this list.
fn retrofit_endpoints(st: &State) -> Vec<Endpoint> {
    const VERBS: [&str; 7] = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];
    let Some(idx) = st.index.get() else {
        return vec![];
    };
    // obfuscated Retrofit: the renamed annotation types (empty when the names are intact)
    let renamed = crate::retrofit_map::retrofit_names(st);
    let real_of = |t: String| renamed.get(&t).cloned().unwrap_or(t);
    let named = |real: String| {
        renamed
            .iter()
            .find(|(_, r)| **r == real)
            .map_or(real.clone(), |(o, _)| o.clone())
    };
    let mut ifaces: Vec<usize> = Vec::new();
    for (d, dx) in st.dexes.iter().enumerate() {
        for v in VERBS {
            let Some(tid) = dx
                .find_string(named(format!("Lretrofit2/http/{v};")).as_bytes())
                .and_then(|s| dx.find_type(s))
            else {
                continue;
            };
            for &k in idx.users(d, index::TYPE, tid as usize) {
                if !ifaces.contains(&(k as usize)) {
                    ifaces.push(k as usize);
                }
            }
        }
    }
    ifaces.sort_unstable();
    let mut out = Vec::new();
    for &k in &ifaces {
        let (class, d, c) = &st.classes[k];
        let dx = &st.dexes[*d];
        let Ok(annos) = dx.annotations(*c) else {
            continue;
        };
        let mut methods: Vec<u32> = annos.methods.keys().copied().collect();
        methods.sort_unstable();
        for mid in methods {
            let mut verb = String::new();
            let mut path = String::new();
            for a in &annos.methods[&mid] {
                let Ok(t) = dx.type_name(a.type_idx).map(real_of) else {
                    continue;
                };
                let Some(v) = t
                    .strip_prefix("Lretrofit2/http/")
                    .and_then(|r| r.strip_suffix(';'))
                    .filter(|v| VERBS.contains(v))
                else {
                    continue;
                };
                verb = v.to_string();
                for (n, val) in &a.elements {
                    if dx.string(*n).is_ok_and(|n| n == "value")
                        && let dex::Value::Str(s) = val
                        && let Ok(p) = dx.string(*s)
                    {
                        path = p;
                    }
                }
            }
            if verb.is_empty() {
                continue;
            }
            let Ok((_, name, proto)) = dx.method_parts(mid) else {
                continue;
            };
            let Ok((ret, types)) = dx.proto_parts(proto) else {
                continue;
            };
            let pannos = annos.params.get(&mid);
            let mut params = Vec::new();
            for (i, ty) in types.iter().enumerate() {
                let mut kind = "arg";
                let mut pname = None;
                if let Some(list) = pannos
                    && let Some(ann) = list.get(i)
                {
                    for a in ann {
                        let Ok(t) = dx.type_name(a.type_idx).map(real_of) else {
                            continue;
                        };
                        if let Some((k, named)) = param_kind(&t) {
                            kind = k;
                            if named {
                                for (n, val) in &a.elements {
                                    if dx.string(*n).is_ok_and(|n| n == "value")
                                        && let dex::Value::Str(s) = val
                                        && let Ok(p) = dx.string(*s)
                                    {
                                        pname = Some(p);
                                    }
                                }
                            }
                        }
                    }
                }
                params.push((kind, pname, short_ty(ty)));
            }
            out.push(Endpoint {
                class: class.clone(),
                method: name,
                verb,
                path,
                params,
                ret: short_ty(&ret),
                dex: *d,
                method_idx: mid,
            });
        }
    }
    out
}

/// The argument list of the first `call` on `line`, nesting respected.
fn call_args(line: &str, call: &str) -> Option<String> {
    let at = line.find(call)? + call.len();
    let b = line.as_bytes();
    let (mut depth, mut end) = (1i32, at);
    while end < b.len() {
        match b[end] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        end += 1;
    }
    line.get(at..end).map(str::to_string)
}

/// Top-level comma split of an argument list (nesting and string literals respected).
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut depth, mut quote) = (0i32, false);
    for ch in s.chars() {
        match ch {
            '"' => {
                quote = !quote;
                cur.push(ch);
            }
            '(' | '[' | '{' if !quote => {
                depth += 1;
                cur.push(ch);
            }
            ')' | ']' | '}' if !quote => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 && !quote => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// The stored value of a `Class.FIELD` static string constant, when the app defines it.
/// Local lookup for the base-URL and call-site resolutions.
fn const_value(st: &State, expr: &str) -> Option<String> {
    let (cls, field) = expr.rsplit_once('.')?;
    let short = format!(".{}", cls.rsplit('.').next().unwrap_or(cls));
    for dx in &st.dexes {
        for c in 0..dx.class_count {
            let Ok(desc) = dx.class_descriptor(c) else {
                continue;
            };
            if !dex::java_name(&desc).ends_with(&short) {
                continue;
            }
            let Ok(data) = dx.class_data(c) else { continue };
            let vals = dx.static_values(c).unwrap_or_default();
            for (k, (f, acc)) in data.fields.iter().enumerate() {
                if acc & 0x8 == 0 {
                    break;
                }
                let Ok((_, _, name)) = dx.field_parts(*f) else {
                    continue;
                };
                if name == field
                    && let Some(dex::Value::Str(s)) = vals.get(k)
                    && let Ok(v) = dx.string(*s)
                {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// The initializer of a local in decompiled text, one hop: `Type name = <expr>;`. Locals the
/// decompiler hoists out of a call still resolve this way.
fn local_init(text: &str, name: &str) -> Option<String> {
    let needle = format!(" {name} = ");
    let l = text.lines().find(|l| l.contains(&needle))?;
    let at = l.find(&needle)? + needle.len();
    Some(l[at..].trim().trim_end_matches(';').trim().to_string())
}

/// A call argument resolved to a fact: a string literal as-is, a `Class.FIELD` reference
/// through its stored value, or a local through its initializer (one hop). None when the
/// argument is computed — never a guess.
fn arg_value(st: &State, text: &str, a: &str) -> Option<String> {
    if a.starts_with('"') && a.ends_with('"') && a.len() > 1 {
        return Some(a[1..a.len() - 1].to_string());
    }
    if let Some(v) = const_value(st, a) {
        return Some(v);
    }
    let init = local_init(text, a)?;
    if init.starts_with('"') && init.ends_with('"') && init.len() > 1 {
        Some(init[1..init.len() - 1].to_string())
    } else {
        const_value(st, &init)
    }
}

/// (class, base URL) of every `Retrofit.Builder.baseUrl(...)` call: the literal, or a
/// `Class.FIELD` reference resolved to its stored value.
fn base_urls(st: &State) -> Vec<(String, String)> {
    let Some(idx) = st.index.get() else {
        return vec![];
    };
    let mut out = Vec::new();
    for (d, dx) in st.dexes.iter().enumerate() {
        for id in 0..dx.counts()[3] as u32 {
            let Ok((owner, name, _)) = dx.method_parts(id) else {
                continue;
            };
            if owner != "Lretrofit2/Retrofit$Builder;" || name != "baseUrl" {
                continue;
            }
            for &k in idx.users(d, index::METHOD, id as usize) {
                let (class, _, _) = &st.classes[k as usize];
                let Some(text) = st.class_text(class, false) else {
                    continue;
                };
                for line in text.lines() {
                    let Some(arg) = call_args(line, "baseUrl(") else {
                        continue;
                    };
                    if let Some(url) = arg_value(st, &text, arg.trim()) {
                        out.push((class.clone(), url));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The declared fields of a class, as `name: type` pairs — the shape of a response model.
fn model_fields(st: &State, class: &str) -> Option<Vec<String>> {
    let k = st.find(class)?;
    let (_, d, c) = &st.classes[k];
    let dx = &st.dexes[*d];
    let data = dx.class_data(*c).ok()?;
    let mut out = Vec::new();
    for (f, _) in &data.fields {
        if out.len() >= 8 {
            break;
        }
        if let Ok((_, ty, name)) = dx.field_parts(*f) {
            out.push(format!("{}: {}", name, short_ty(&ty)));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// What the endpoint's callers pass and which classes they reference, read by decompiling
/// the classes that call the interface method. Heuristic — labelled `inferred` in the
/// output: positional argument matching over the decompiled call.
fn endpoint_callers(st: &State, e: &Endpoint) -> Callers {
    let mut c = Callers::default();
    let Some(idx) = st.index.get() else { return c };
    let mut ks: Vec<usize> = idx
        .users(e.dex, index::METHOD, e.method_idx as usize)
        .iter()
        .map(|&k| k as usize)
        .collect();
    ks.sort_unstable();
    let class_re = regex::Regex::new(r"(\w+)\.class").unwrap();
    for k in ks.iter().take(4) {
        let (class, _, _) = &st.classes[*k];
        // the interface's own declaration is one of the "users" of its methods; not a caller
        if *class == e.class {
            continue;
        }
        let Some(text) = st.class_text(class, false) else {
            continue;
        };
        if c.class.is_empty() {
            c.class = class.clone();
        }
        for cap in class_re.captures_iter(&text) {
            let short = format!(".{}", &cap[1]);
            let Some((full, ..)) = st.classes.iter().find(|(n, _, _)| n.ends_with(&short)) else {
                continue;
            };
            let lib = st
                .find(full)
                .is_some_and(|k| st.origin[k] == Origin::Library);
            if !lib && !c.models.contains(full) {
                c.models.push(full.clone());
            }
        }
        let call = format!(".{}(", e.method);
        for line in text.lines() {
            let Some(src) = call_args(line, &call) else {
                continue;
            };
            for (i, a) in split_args(&src).into_iter().enumerate() {
                let Some(name) = e.params.get(i).and_then(|(_, n, _)| n.clone()) else {
                    continue;
                };
                if let Some(v) = arg_value(st, &text, &a) {
                    c.args.insert(name, v);
                }
            }
        }
    }
    c
}

/// One curl line per endpoint: query parameters and fields filled from resolved call-site
/// arguments, `$name` where nothing was resolved — never a guessed value.
fn curl_line(e: &Endpoint, base: &str, args: &BTreeMap<String, String>) -> String {
    let url = format!("{}/{}", base, e.path.trim_start_matches('/'));
    let val = |n: &str| args.get(n).cloned().unwrap_or_else(|| format!("${n}"));
    let named = |want: &str| {
        e.params
            .iter()
            .filter(|(k, n, _)| *k == want && n.is_some())
            .map(|(_, n, _)| n.as_deref().unwrap_or(""))
            .collect::<Vec<_>>()
    };
    if e.verb == "GET" {
        let q: Vec<String> = named("query")
            .into_iter()
            .map(|n| format!("{n}={}", val(n)))
            .collect();
        let qs = if q.is_empty() {
            String::new()
        } else {
            format!("?{}", q.join("&"))
        };
        return format!("curl '{url}{qs}'");
    }
    let f: Vec<String> = named("field")
        .into_iter()
        .map(|n| format!("{n}={}", val(n)))
        .collect();
    let d = if f.is_empty() {
        String::new()
    } else {
        format!(" -d '{}'", f.join("&"))
    };
    format!("curl -X {} '{url}'{}", e.verb, d)
}

/// The `search api` sheet: base URLs, then one line per endpoint — verb, path, parameters,
/// return type, inferred model and arguments — and one curl line per endpoint. Labeled
/// `inferred` where the DEX cannot state a fact (generics are erased).
pub fn api_sheet(st: &State, m: &Matcher) -> Vec<Hit> {
    let bases = base_urls(st);
    let mut hits: Vec<Hit> = Vec::new();
    for (class, base) in &bases {
        let text = format!("base URL: {base}");
        if m.line(&text) {
            hits.push((class.clone(), 0, text));
        }
    }
    for e in retrofit_endpoints(st) {
        let c = endpoint_callers(st, &e);
        let base = bases
            .first()
            .map(|(_, b)| b.trim_end_matches('/'))
            .unwrap_or("");
        let params = e
            .params
            .iter()
            .map(|(kind, name, ty)| match name {
                Some(n) => format!("{n} ({kind}, {ty})"),
                None => format!("({kind}: {ty})"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let model = c.models.iter().find_map(|m2| {
            model_fields(st, m2).map(|f| {
                format!(
                    "{}{{{}}}",
                    m2.rsplit('.').next().unwrap_or(m2),
                    f.join(", ")
                )
            })
        });
        let mut line = format!(
            "{} {}/{}    params: {}    -> {}",
            e.verb,
            base,
            e.path.trim_start_matches('/'),
            params,
            e.ret
        );
        if let Some(m2) = &model {
            line.push_str(&format!("    [model inferred: {m2}]"));
        }
        if !c.class.is_empty() && !c.args.is_empty() {
            line.push_str(&format!(
                "    [args inferred from {}]",
                c.class.rsplit('.').next().unwrap_or(&c.class)
            ));
        }
        if m.line(&line) {
            let curl = curl_line(&e, base, &c.args);
            hits.push((e.class.clone(), 0, line));
            hits.push((e.class.clone(), 0, format!("    {curl}")));
        }
    }
    hits.truncate(LIMIT);
    hits
}
