//! Answers read straight from the DEX tables, no decompiling and no index: the class
//! hierarchy (the `--impl` filters), static constants (`consts`) and methods whose body only
//! returns one constant (`returns`, e.g. every message class's `uri()` id).

use std::collections::{HashMap, HashSet};

use crate::dex::{self, Dex, Value};

/// `a.b.C` or `La/b/C;` -> `La/b/C;`
pub fn descriptor(name: &str) -> String {
    if name.starts_with('L') && name.ends_with(';') {
        name.to_string()
    } else {
        format!("L{};", name.replace('.', "/"))
    }
}

/// Descriptors of every class that extends or implements `target` (a class or interface,
/// `a.b.C` or descriptor), directly or through other classes and interfaces of the APK.
/// `target` itself is not included.
pub fn implementors(dexes: &[Dex], target: &str) -> HashSet<String> {
    let asked = descriptor(target);
    // supertype -> its direct subtypes
    let mut down: HashMap<String, Vec<String>> = HashMap::new();
    // the class the user named by its deobfuscated name, when that differs from the DEX name
    let mut by_shown = None;
    for dx in dexes {
        for c in 0..dx.class_count {
            let Ok(def) = dx.class_def(c) else { continue };
            if by_shown.is_none() && def.class != asked && dex::java_name(&def.class) == target {
                by_shown = Some(def.class.clone());
            }
            for s in def.superclass.iter().chain(def.interfaces.iter()) {
                down.entry(s.clone()).or_default().push(def.class.clone());
            }
        }
    }
    let target = if down.contains_key(&asked) {
        asked
    } else {
        by_shown.unwrap_or(asked)
    };
    let mut out = HashSet::new();
    let mut todo = vec![target];
    while let Some(t) = todo.pop() {
        for sub in down.get(&t).into_iter().flatten() {
            if out.insert(sub.clone()) {
                todo.push(sub.clone());
            }
        }
    }
    out
}

/// A `--in` pattern: `a.b.*` / `a.b.` (a package and everything under it), `*` wildcards
/// anywhere, otherwise an exact class name (nested `a.b.C$D` included with `a.b.C`).
pub struct NameFilter(Vec<regex::Regex>);

impl NameFilter {
    pub fn new(patterns: &[String]) -> Result<NameFilter, String> {
        let mut out = Vec::new();
        for p in patterns.iter().flat_map(|p| p.split(',')) {
            let p = p.trim();
            if p.is_empty() {
                continue;
            }
            let p = p
                .strip_suffix('.')
                .map_or(p.to_string(), |s| format!("{s}.*"));
            let body = p
                .split('*')
                .map(regex::escape)
                .collect::<Vec<_>>()
                .join(".*");
            // an exact class also covers its nested classes
            let re = if p.contains('*') {
                format!("^{body}$")
            } else {
                format!("^{body}(\\$.*)?$")
            };
            out.push(regex::Regex::new(&re).map_err(|e| e.to_string())?);
        }
        Ok(NameFilter(out))
    }

    /// `name` is a Java name (`a.b.C`).
    pub fn matches(&self, name: &str) -> bool {
        self.0.is_empty() || self.0.iter().any(|r| r.is_match(name))
    }

    /// A class by its descriptor and shown (possibly deobfuscated) name: either one may match.
    pub fn accepts(&self, desc: &str, shown: &str) -> bool {
        self.matches(shown) || self.matches(&raw_name(desc))
    }
}

/// `La/b/C;` -> `a.b.C`, the name in the DEX (no deobfuscator).
pub fn raw_name(desc: &str) -> String {
    desc.strip_prefix('L')
        .and_then(|s| s.strip_suffix(';'))
        .unwrap_or(desc)
        .replace('/', ".")
}

/// One answer row: `class#member = value  (kind)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub class: String,
    pub member: String,
    pub value: String,
    /// int, long, byte, short, char, float, double, boolean, String
    pub kind: &'static str,
}

impl Row {
    pub fn line(&self) -> String {
        format!(
            "{}#{} = {}    ({})",
            self.class, self.member, self.value, self.kind
        )
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"class": self.class, "member": self.member, "value": self.value, "kind": self.kind})
    }
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

fn int_kind(k: char) -> &'static str {
    match k {
        'b' => "byte",
        's' => "short",
        'c' => "char",
        'j' => "long",
        _ => "int",
    }
}

/// `value` written the way Java source would, plus its kind; None for values that are not
/// constants a user would look up (types, methods, nested arrays).
fn shown(dx: &Dex, v: &Value) -> Option<(String, &'static str)> {
    Some(match v {
        Value::Int(n, 'c') => match char::from_u32(*n as u32) {
            Some(c) if !c.is_control() => (format!("'{c}' ({n})"), "char"),
            _ => (n.to_string(), "char"),
        },
        Value::Int(n, k) => (n.to_string(), int_kind(*k)),
        Value::Float(f) => (format!("{f}f"), "float"),
        Value::Double(d) => (d.to_string(), "double"),
        Value::Bool(b) => (b.to_string(), "boolean"),
        Value::Str(s) => (quoted(&dx.string(*s).ok()?), "String"),
        _ => return None,
    })
}

/// A `--value` test: numbers compare as numbers (`736131`, `0xB3B83`, `-1`), anything else
/// as the exact string value.
pub struct ValueFilter(Option<String>, Option<i128>);

impl ValueFilter {
    pub fn new(v: Option<&str>) -> ValueFilter {
        let num = v.and_then(|v| {
            let v = v.trim();
            let (neg, body) = v.strip_prefix('-').map_or((false, v), |b| (true, b));
            let n = match body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
                Some(h) => i128::from_str_radix(h, 16).ok(),
                None => body.parse::<i128>().ok(),
            }?;
            Some(if neg { -n } else { n })
        });
        ValueFilter(v.map(str::to_string), num)
    }

    fn matches(&self, r: &Row) -> bool {
        let Some(want) = &self.0 else { return true };
        match self.1 {
            // the leading number of the shown value (chars show `'x' (120)`)
            Some(n) => {
                // 0x9cbd4ec4 also means the int it wraps to (-1665445180)
                let wrapped = (n > i32::MAX as i128 && n <= u32::MAX as i128)
                    .then_some(n as u32 as i32 as i128);
                let eq = |v: i128| v == n || Some(v) == wrapped;
                let digits = r
                    .value
                    .rsplit('(')
                    .next()
                    .unwrap_or(&r.value)
                    .trim_end_matches(')');
                digits.parse::<i128>().is_ok_and(eq) || r.value.parse::<i128>().is_ok_and(eq)
            }
            None => r.value == quoted(want) || r.value == *want,
        }
    }
}

/// Static final fields with a constant initial value, of the classes `names` keeps.
pub fn consts(
    dexes: &[Dex],
    names: &NameFilter,
    value: &ValueFilter,
    member: Option<&regex::Regex>,
) -> Vec<Row> {
    let mut out = Vec::new();
    for dx in dexes {
        for c in 0..dx.class_count {
            let Ok(desc) = dx.class_descriptor(c) else {
                continue;
            };
            let class = dex::java_name(&desc);
            if !names.accepts(&desc, &class) {
                continue;
            }
            let Ok(data) = dx.class_data(c) else { continue };
            let vals = dx.static_values(c).unwrap_or_default();
            let assigned = clinit_writes(dx, c);
            for (k, (f, acc)) in data.fields.iter().enumerate() {
                // static fields come first; the stored values line up with that prefix
                if acc & 0x8 == 0 {
                    break;
                }
                // final only, and not set by the static initializer: the DEX then stores
                // only the type's default (0, null), not the value the app runs with
                if acc & 0x10 == 0 || assigned.contains(f) {
                    continue;
                }
                let Some((v, kind)) = vals.get(k).and_then(|v| shown(dx, v)) else {
                    continue;
                };
                let Ok((owner, ty, name)) = dx.field_parts(*f) else {
                    continue;
                };
                let shown_name = crate::deobf::field(&owner, &name, &ty).unwrap_or(name);
                if member.is_some_and(|m| !m.is_match(&shown_name)) {
                    continue;
                }
                let r = Row {
                    class: class.clone(),
                    member: shown_name,
                    value: v,
                    kind,
                };
                if value.matches(&r) {
                    out.push(r);
                }
            }
        }
    }
    out.sort_by(|a, b| a.class.cmp(&b.class));
    out
}

/// Field ids the class's static initializer (`<clinit>`) writes with `sput*`.
fn clinit_writes(dx: &Dex, c: usize) -> HashSet<u32> {
    let mut out = HashSet::new();
    let _ = dx.each_method(c, |idx, code| {
        let Some(code) = code else { return };
        if dx.method_parts(idx).is_ok_and(|(_, n, _)| n == "<clinit>")
            && let Ok(insns) = crate::bytecode::decode(code)
        {
            // sput, sput-wide, -object, -boolean, -byte, -char, -short
            out.extend(
                insns
                    .iter()
                    .filter(|i| (0x67..=0x6d).contains(&i.op))
                    .map(|i| i.idx),
            );
        }
    });
    out
}

/// The constant a method body returns when the body is only `const* vX; return vX` (one
/// or two instructions, the shape of `int uri() { return 736131; }`).
fn returned(dx: &Dex, code: &[u8]) -> Option<(String, &'static str)> {
    let u = |k: usize| -> Option<u16> {
        code.get(2 * k..2 * k + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let first = u(0)?;
    let (op, a) = ((first & 0xff) as u8, (first >> 8) as u8);
    // (value, kind, code units of the const instruction, its register)
    let (val, kind, len, reg): (String, &'static str, usize, u8) = match op {
        0x12 => ((((a as i8) >> 4) as i64).to_string(), "int", 1, a & 0xf),
        0x13 => ((u(1)? as i16 as i64).to_string(), "int", 2, a),
        0x14 => (
            ((u(1)? as u32 | (u(2)? as u32) << 16) as i32 as i64).to_string(),
            "int",
            3,
            a,
        ),
        0x15 => (
            (((u(1)? as u32) << 16) as i32 as i64).to_string(),
            "int",
            2,
            a,
        ),
        0x16 => ((u(1)? as i16 as i64).to_string(), "long", 2, a),
        0x17 => (
            ((u(1)? as u32 | (u(2)? as u32) << 16) as i32 as i64).to_string(),
            "long",
            3,
            a,
        ),
        0x18 => {
            let v = (0..4).try_fold(0u64, |acc, k| Some(acc | (u(1 + k)? as u64) << (16 * k)))?;
            ((v as i64).to_string(), "long", 5, a)
        }
        0x19 => ((((u(1)? as u64) << 48) as i64).to_string(), "long", 2, a),
        0x1a => (quoted(&dx.string(u(1)? as u32).ok()?), "String", 2, a),
        0x1b => (
            quoted(&dx.string(u(1)? as u32 | (u(2)? as u32) << 16).ok()?),
            "String",
            3,
            a,
        ),
        _ => return None,
    };
    let ret = u(len)?;
    let ok = match (ret & 0xff) as u8 {
        0x0f => kind == "int",
        0x10 => kind == "long",
        0x11 => kind == "String",
        _ => false,
    } && (ret >> 8) as u8 == reg;
    ok.then_some((val, kind))
}

/// Methods named `method` (any, when None) whose body returns one constant, in the classes `keep` accepts
/// (by descriptor and Java name).
pub fn returns(
    dexes: &[Dex],
    method: Option<&str>,
    keep: impl Fn(&str, &str) -> bool,
    value: &ValueFilter,
) -> Vec<Row> {
    let mut out = Vec::new();
    for dx in dexes {
        for c in 0..dx.class_count {
            let Ok(desc) = dx.class_descriptor(c) else {
                continue;
            };
            let class = dex::java_name(&desc);
            if !keep(&desc, &class) {
                continue;
            }
            let _ = dx.each_method(c, |idx, code| {
                let Some(code) = code else { return };
                let Ok((owner, name, proto)) = dx.method_parts(idx) else {
                    return;
                };
                let shown_name = crate::deobf::method(&owner, &name).unwrap_or(name.clone());
                if method.is_some_and(|m| name != m && shown_name != m) {
                    return;
                }
                if let Some((v, kind)) = returned(dx, code) {
                    // the bytecode only knows int / long / object: the declared type says more
                    let ret = dx
                        .proto(proto)
                        .ok()
                        .and_then(|p| p.rsplit(')').next().map(str::to_string));
                    let (v, kind) = match ret.as_deref() {
                        Some("Z") => ((v != "0").to_string(), "boolean"),
                        Some("B") => (v, "byte"),
                        Some("S") => (v, "short"),
                        Some("C") => (v, "char"),
                        _ => (v, kind),
                    };
                    let r = Row {
                        class: class.clone(),
                        member: format!("{shown_name}()"),
                        value: v,
                        kind,
                    };
                    if value.matches(&r) {
                        out.push(r);
                    }
                }
            });
        }
    }
    out.sort_by(|a, b| a.class.cmp(&b.class));
    out
}

/// Id of the top-level class a hit target names (`a.b.C`, `a.b.C$D`, `a.b.C#m`); None for
/// resources and native libraries.
pub fn class_id(st: &crate::session::State, target: &str) -> Option<usize> {
    let t = target.split(['#', '$']).next().unwrap_or(target);
    st.find(t)
}

/// The `--in` / `--impl` mask over a session's classes (search filters); None when neither is
/// given.
pub fn class_mask(
    st: &crate::session::State,
    within: &[String],
    implements: Option<&str>,
) -> Result<Option<Vec<bool>>, String> {
    if within.is_empty() && implements.is_none() {
        return Ok(None);
    }
    let names = NameFilter::new(within)?;
    // nested subtypes (`a.b.C$Msg`) live in their top-level class's decompiled text
    let subtypes = implements.map(|t| {
        implementors(&st.dexes, t)
            .into_iter()
            .map(|d| match d.find('$') {
                Some(i) => format!("{};", &d[..i]),
                None => d,
            })
            .collect::<std::collections::HashSet<_>>()
    });
    if subtypes.as_ref().is_some_and(|s| s.is_empty()) {
        return Err(format!(
            "--impl {}: no class in this APK extends or implements it",
            implements.unwrap_or_default()
        ));
    }
    Ok(Some(
        st.classes
            .iter()
            .map(|(shown, d, c)| {
                let desc = st.dexes[*d].class_descriptor(*c).unwrap_or_default();
                names.accepts(&desc, shown) && subtypes.as_ref().is_none_or(|s| s.contains(&desc))
            })
            .collect(),
    ))
}

#[cfg(test)]
#[path = "../tests/apkscope/lookup.rs"]
mod tests;
