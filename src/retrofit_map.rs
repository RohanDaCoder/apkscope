//! Retrofit's annotation types in an obfuscated app. R8 renames `retrofit2.http.GET` to
//! something like `tba`, so `search api` can't find endpoints by name. Retrofit's own parser
//! still says which is which: it tests each annotation with `instance-of` and then passes the
//! verb string ("GET", "POST", ...) on, and its parameter branches carry error messages that
//! name the annotation ("@Path parameter name must match ..."). Read from that bytecode, the
//! map is exact where those strings are, and by Retrofit's fixed check order in between.

use std::collections::HashMap;

use crate::dex::Dex;
use crate::index;
use crate::session::State;

const VERBS: [&str; 7] = ["DELETE", "GET", "HEAD", "PATCH", "POST", "PUT", "OPTIONS"];
/// The order Retrofit tests parameter annotations in (RequestFactory.parseParameterAnnotation).
const PARAMS: [&str; 13] = [
    "Url",
    "Path",
    "Query",
    "QueryName",
    "QueryMap",
    "Header",
    "HeaderMap",
    "Field",
    "FieldMap",
    "Part",
    "PartMap",
    "Body",
    "Tag",
];

/// Obfuscated annotation descriptor -> its Retrofit name (`Lretrofit2/http/GET;`). Empty when
/// Retrofit's names are intact or there is no Retrofit.
pub fn retrofit_names(st: &State) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(idx) = st.index.get() else {
        return map;
    };
    for (d, dx) in st.dexes.iter().enumerate() {
        if dx.find_string(b"Lretrofit2/http/GET;").is_some() {
            return HashMap::new();
        }
        let Some(sid) = dx.find_string(b"OPTIONS") else {
            continue;
        };
        for &k in idx.users(d, index::STRING, sid as usize) {
            let (_, kd, kc) = &st.classes[k as usize];
            let kdx = &st.dexes[*kd];
            let Ok(top) = kdx.class_descriptor(*kc) else {
                continue;
            };
            let nested = format!("{}$", top.trim_end_matches(';'));
            for c in 0..kdx.class_count {
                if kdx
                    .class_descriptor(c)
                    .is_ok_and(|n| n == top || n.starts_with(&nested))
                {
                    let _ = kdx.each_method(c, |_, code| {
                        if let Some(code) = code {
                            read_parser(kdx, code, &mut map);
                        }
                    });
                }
            }
        }
    }
    map
}

/// An annotation interface of the app (not a JDK / Android / Kotlin type).
fn is_annotation(dx: &Dex, desc: &str) -> bool {
    if ["Ljava/", "Ljavax/", "Landroid/", "Lkotlin/"]
        .iter()
        .any(|p| desc.starts_with(p))
    {
        return false;
    }
    // defined in this DEX: must be an annotation (ACC_ANNOTATION); defined elsewhere: allowed
    (0..dx.class_count)
        .find(|&c| dx.class_descriptor(c).is_ok_and(|n| n == desc))
        .is_none_or(|c| dx.class_def(c).is_ok_and(|d| d.access & 0x2000 != 0))
}

fn read_parser(dx: &Dex, code: &[u8], map: &mut HashMap<String, String>) {
    let Ok(insns) = crate::bytecode::decode(code) else {
        return;
    };
    let mut regs: HashMap<u16, String> = HashMap::new();
    let mut cur: Option<String> = None;
    // parameter-annotation candidates in test order, with the strings their branch loads
    let mut seq: Vec<(String, Vec<String>)> = Vec::new();
    let mut verbs_here = false;
    for i in &insns {
        match i.op {
            0x1a | 0x1b => {
                let Ok(s) = dx.string(i.idx) else { continue };
                if let Some((_, strs)) = seq.last_mut() {
                    strs.push(s.clone());
                }
                regs.insert(i.regs[0], s);
            }
            // check-cast / instance-of on an annotation type
            0x1f | 0x20 => {
                let Ok(t) = dx.type_name(i.idx) else { continue };
                if !is_annotation(dx, &t) {
                    continue;
                }
                if !seq.iter().any(|(s, _)| *s == t) {
                    seq.push((t.clone(), vec![]));
                }
                cur = Some(t);
            }
            // an invoke right after the test that passes a verb string: that type is the verb
            0x6e..=0x72 | 0x74..=0x78 => {
                let Some(t) = &cur else { continue };
                if map.contains_key(t) {
                    continue;
                }
                let args: Vec<u16> = if i.range {
                    (i.regs[0]..i.regs[0] + i.nregs as u16).collect()
                } else {
                    i.regs[..i.nregs as usize].to_vec()
                };
                if let Some(v) = args
                    .iter()
                    .filter_map(|r| regs.get(r))
                    .find(|s| VERBS.contains(&s.as_str()))
                {
                    let real = format!("Lretrofit2/http/{v};");
                    if !map.values().any(|x| *x == real) {
                        map.insert(t.clone(), real);
                        verbs_here = true;
                    }
                }
            }
            _ => {}
        }
    }
    let _ = verbs_here;
    // parameter annotations: anchors named by their own messages, the gaps by Retrofit's order
    let seq: Vec<&(String, Vec<String>)> =
        seq.iter().filter(|(t, _)| !map.contains_key(t)).collect();
    let anchor = |strs: &[String]| -> Option<&'static str> {
        let has = |p: &str| strs.iter().any(|s| s.starts_with(p));
        if strs.iter().any(|s| s.contains("Multiple @Url")) || has("@Url must be") {
            return Some("Url");
        }
        if has("@Path parameter name must match") {
            return Some("Path");
        }
        for x in ["QueryMap", "HeaderMap", "FieldMap", "PartMap"] {
            if has(&format!("@{x} keys must")) || has(&format!("@{x} parameter type must")) {
                return Some(x);
            }
        }
        if has("@Field parameters can only") {
            return Some("Field");
        }
        if has("@Part parameters can only") || has("@Part annotation must supply") {
            return Some("Part");
        }
        if has("@Body parameters cannot") {
            return Some("Body");
        }
        None
    };
    let found: Vec<(usize, usize)> = seq
        .iter()
        .enumerate()
        .filter_map(|(i, (_, strs))| {
            let name = anchor(strs)?;
            Some((i, PARAMS.iter().position(|p| *p == name)?))
        })
        .collect();
    let mut names: Vec<Option<&str>> = vec![None; seq.len()];
    for &(i, o) in &found {
        names[i] = Some(PARAMS[o]);
    }
    for w in found.windows(2) {
        let ((i, oi), (j, oj)) = (w[0], w[1]);
        if j > i && oj > oi && j - i == oj - oi {
            for g in 1..j - i {
                names[i + g] = Some(PARAMS[oi + g]);
            }
        }
    }
    for (k, n) in names.iter().enumerate() {
        if let Some(n) = n {
            let real = format!("Lretrofit2/http/{n};");
            if !map.values().any(|x| *x == real) {
                map.insert(seq[k].0.clone(), real);
            }
        }
    }
}
