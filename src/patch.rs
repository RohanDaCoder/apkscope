//! XML patch bundles: one file that describes every edit an APK needs.
//!
//! A bundle is what `apkscope patch` applies. It says which app it is for, and holds named
//! patches, picked from a list, each a list of edits: `return` stubs a method to return a
//! constant, `replace` swaps smali text inside a method, and `string`, `manifest` and `entry`
//! edit resources, the manifest and the zip itself. The XML is read by the same parser the
//! text resource forms use ([`apkscope_core::markup`]); method edits are applied to the
//! class's own smali and re-assembled from it, so a bundle edits readable smali text, never
//! raw DEX hex patterns.

use apkscope_core::markup::{Element, parse_xml};

/// One patch bundle, [`parse`]d from its XML.
#[derive(Debug)]
pub struct Bundle {
    pub name: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    /// The app the bundle is for, when it says.
    pub target: Option<Target>,
    /// The named patches, in file order.
    pub patches: Vec<Patch>,
}

/// The app a bundle is for: what `<target>` says, and every part given must match. A bundle
/// for one app is not run on another, or on a version it was not written for.
#[derive(Debug)]
pub struct Target {
    pub package: String,
    pub version_name: Option<String>,
    pub min_version_code: Option<u32>,
    pub max_version_code: Option<u32>,
}

/// One named patch, picked or skipped by name, whose ops are applied together.
#[derive(Debug)]
pub struct Patch {
    pub name: String,
    pub description: Option<String>,
    pub ops: Vec<Op>,
}

/// One edit, as the XML says it. File paths stay as written: the command reads them relative
/// to the bundle's own folder.
#[derive(Debug)]
pub enum Op {
    /// `<return>`: stub the method's whole body to return a constant.
    Return {
        class: String,
        method: String,
        /// the exact signature `name(params)ret`; omitted: every method of that name
        sig: Option<String>,
        /// the constant, typed by the method's return type (absent: the type's zero, null for
        /// references, nothing for void)
        value: Option<String>,
        line: usize,
    },
    /// `<replace>`: swap smali text inside the method's body.
    Replace {
        class: String,
        method: String,
        sig: Option<String>,
        from: String,
        to: String,
        /// only the first occurrence in each matching method
        once: bool,
        line: usize,
    },
    /// `<string>`: set one string resource.
    Str {
        name: String,
        config: Option<String>,
        value: String,
    },
    /// `<manifest>`: replace AndroidManifest.xml with this text-XML file.
    Manifest { file: String },
    /// `<entry>`: add or replace a zip entry with a file, or delete it.
    Entry {
        name: String,
        src: Option<String>,
        delete: bool,
    },
}

/// Parse a bundle. Every element and attribute is checked against the format: a typo is an
/// error that names its line, not an edit that silently does nothing.
pub fn parse(text: &str) -> Result<Bundle, String> {
    let root = parse_xml(text)?;
    if root.name != "bundle" {
        return Err(format!(
            "line {}: the root element is <bundle>, not <{}>",
            root.line, root.name
        ));
    }
    only(&root, &["name", "author", "version", "description"])?;
    let mut bundle = Bundle {
        name: root.attr("name").map(str::to_string),
        author: root.attr("author").map(str::to_string),
        version: root.attr("version").map(str::to_string),
        description: root.attr("description").map(str::to_string),
        target: None,
        patches: Vec::new(),
    };
    for e in root.elements() {
        match e.name.as_str() {
            "target" => bundle.target = Some(Target::parse(e)?),
            "patch" => {
                let p = Patch::parse(e)?;
                // a name is how --only and --skip pick a patch, so two of one name is an error
                if bundle.patches.iter().any(|x| x.name == p.name) {
                    return Err(format!("line {}: two patches named {:?}", e.line, p.name));
                }
                bundle.patches.push(p);
            }
            _ => {
                return Err(format!(
                    "line {}: <bundle> has no <{}> inside",
                    e.line, e.name
                ));
            }
        }
    }
    Ok(bundle)
}

impl Target {
    fn parse(e: &Element) -> Result<Self, String> {
        only(
            e,
            &["package", "versionName", "minVersionCode", "maxVersionCode"],
        )?;
        no_children(e)?;
        let code = |k: &str| {
            e.attr(k)
                .map(|v| {
                    v.parse::<u32>().map_err(|_| {
                        format!("line {}: <target> {} is not a number: {v}", e.line, k)
                    })
                })
                .transpose()
        };
        Ok(Target {
            package: req(e, "package")?,
            version_name: e.attr("versionName").map(str::to_string),
            min_version_code: code("minVersionCode")?,
            max_version_code: code("maxVersionCode")?,
        })
    }

    /// Whether `(package, versionName, versionCode)` is the app this target names. A part the
    /// target checks that the APK does not say is a mismatch, not a pass.
    pub fn matches(
        &self,
        package: Option<&str>,
        name: Option<&str>,
        code: Option<u32>,
    ) -> Result<(), String> {
        match package {
            Some(p) if p == self.package => {}
            Some(p) => {
                return Err(format!(
                    "the bundle is for {}, the APK is {p}",
                    self.package
                ));
            }
            None => {
                return Err(
                    "the APK has no package name to check the bundle's <target> against".into(),
                );
            }
        }
        if let Some(want) = &self.version_name {
            match name {
                Some(got) if got == want => {}
                Some(got) => {
                    return Err(format!(
                        "the bundle is for versionName {want:?}, the APK has {got:?}"
                    ));
                }
                None => {
                    return Err(
                        "the APK has no android:versionName to check the bundle's <target> against"
                            .into(),
                    );
                }
            }
        }
        if let Some(want) = self.min_version_code {
            match code {
                Some(got) if got >= want => {}
                Some(got) => {
                    return Err(format!(
                        "the bundle is for versionCode at least {want}, the APK has {got}"
                    ));
                }
                None => {
                    return Err(
                        "the APK has no android:versionCode to check the bundle's <target> against"
                            .into(),
                    );
                }
            }
        }
        if let Some(want) = self.max_version_code {
            match code {
                Some(got) if got <= want => {}
                Some(got) => {
                    return Err(format!(
                        "the bundle is for versionCode at most {want}, the APK has {got}"
                    ));
                }
                None => {
                    return Err(
                        "the APK has no android:versionCode to check the bundle's <target> against"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
}

impl Patch {
    fn parse(e: &Element) -> Result<Self, String> {
        only(e, &["name", "description"])?;
        let mut p = Patch {
            name: req(e, "name")?,
            description: e.attr("description").map(str::to_string),
            ops: Vec::new(),
        };
        for c in e.elements() {
            p.ops.push(op(c)?);
        }
        Ok(p)
    }
}

fn op(e: &Element) -> Result<Op, String> {
    match e.name.as_str() {
        "return" => {
            only(e, &["class", "method", "sig", "value"])?;
            no_children(e)?;
            Ok(Op::Return {
                class: req(e, "class")?,
                method: req(e, "method")?,
                sig: opt(e, "sig")?,
                value: opt(e, "value")?,
                line: e.line,
            })
        }
        "replace" => {
            only(e, &["class", "method", "sig", "from", "to", "once"])?;
            no_children(e)?;
            Ok(Op::Replace {
                class: req(e, "class")?,
                method: req(e, "method")?,
                sig: opt(e, "sig")?,
                from: req(e, "from")?,
                to: req(e, "to")?,
                once: yes_no(e, "once")?,
                line: e.line,
            })
        }
        "string" => {
            only(e, &["name", "config"])?;
            Ok(Op::Str {
                name: req(e, "name")?,
                config: opt(e, "config")?,
                value: e.text(),
            })
        }
        "manifest" => {
            only(e, &["file"])?;
            no_children(e)?;
            Ok(Op::Manifest {
                file: req(e, "file")?,
            })
        }
        "entry" => {
            only(e, &["name", "src", "delete"])?;
            no_children(e)?;
            let delete = yes_no(e, "delete")?;
            let src = opt(e, "src")?;
            let name = req(e, "name")?;
            if src.is_some() == delete {
                return Err(format!(
                    "line {}: <entry> needs one of src=... or delete=yes",
                    e.line
                ));
            }
            // an empty name, or one ending in /, is not a file: it would add a broken entry
            // to the APK that nothing on Android can read
            if name.is_empty() || name.ends_with('/') {
                return Err(format!(
                    "line {}: <entry> name is a file path; {name:?} is not one",
                    e.line
                ));
            }
            Ok(Op::Entry { name, src, delete })
        }
        _ => Err(format!(
            "line {}: <patch> has no <{}> inside",
            e.line, e.name
        )),
    }
}

fn req(e: &Element, k: &str) -> Result<String, String> {
    e.attr(k)
        .map(str::to_string)
        .ok_or_else(|| format!("line {}: <{}> needs {}=...", e.line, e.name, k))
}

fn opt(e: &Element, k: &str) -> Result<Option<String>, String> {
    Ok(e.attr(k).map(str::to_string))
}

/// A yes / no attribute: absent is false, and anything else is an error.
fn yes_no(e: &Element, k: &str) -> Result<bool, String> {
    match e.attr(k) {
        None => Ok(false),
        Some("yes") | Some("true") => Ok(true),
        Some("no") | Some("false") => Ok(false),
        Some(v) => Err(format!("line {}: {} is yes or no, not {v:?}", e.line, k)),
    }
}

/// The XML only allows the attributes the format understands: a typo'd one is an error.
fn only(e: &Element, known: &[&str]) -> Result<(), String> {
    match e.attrs.iter().find(|(k, _)| !known.contains(&k.as_str())) {
        Some((k, _)) => Err(format!(
            "line {}: <{}> has no attribute {k}",
            e.line, e.name
        )),
        None => Ok(()),
    }
}

/// `<return>`, `<replace>`, `<manifest>` and `<entry>` are a single line; `<string>` holds
/// text, not markup.
fn no_children(e: &Element) -> Result<(), String> {
    match e.elements().next() {
        Some(c) => Err(format!(
            "line {}: <{}> has nothing inside, found <{}>",
            e.line, e.name, c.name
        )),
        None => Ok(()),
    }
}

// ─────────────────────────── the smali edits a bundle makes ───────────────────────────

/// Where a `<return>` or `<replace>` edit lands, and how errors name it.
#[derive(Debug)]
pub struct At {
    pub class: String,
    pub method: String,
    /// the exact signature `name(params)ret`; omitted: every method of that name
    pub sig: Option<String>,
    /// the patch name and the op, what errors start with
    pub note: String,
}

impl At {
    /// `name`, or `name(params)ret`: how an error names the method.
    fn want(&self) -> String {
        self.sig
            .as_ref()
            .map_or_else(|| self.method.clone(), |s| format!("{}{s}", self.method))
    }
}

/// The `.method` blocks of `method` in a class's smali, as (header line, `.end method` line)
/// pairs top to bottom. A `sig` of `name(params)ret` names one signature exactly; without it
/// every method of that name matches, overloads included.
pub fn methods(lines: &[&str], method: &str, sig: Option<&str>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let head = lines[i]
            .trim_start()
            .strip_prefix(".method ")
            .and_then(|rest| rest.split_whitespace().last());
        let hits = head.is_some_and(|h| match sig {
            // the name followed directly by `(` cannot be a longer name
            None => h.starts_with(method) && h[method.len()..].starts_with('('),
            Some(s) => h == format!("{method}{s}"),
        });
        if hits
            && let Some(end) = lines[i + 1..]
                .iter()
                .position(|l| l.trim() == ".end method")
        {
            out.push((i, i + 1 + end));
            i = end + i + 2;
        } else {
            i += 1;
        }
    }
    out
}

/// `abstract` and `native` methods have no body: a DEX that gives them one is invalid, and
/// Android refuses the whole APK. `<return>` and `<replace>` edit bodies, so a header that
/// says `abstract` or `native` is an error that names the method, not an edit that silently
/// bricks the app.
fn bodyless(lines: &[&str], hits: &[(usize, usize)], at: &At) -> Result<(), String> {
    for &(start, _) in hits {
        let flag = lines[start]
            .split_whitespace()
            .take_while(|t| !t.contains('('))
            .find(|t| *t == "abstract" || *t == "native");
        if let Some(flag) = flag {
            return Err(format!(
                "{}: {}#{} is {flag}: it has no body to edit",
                at.note,
                at.class,
                at.want()
            ));
        }
    }
    Ok(())
}

/// `<return>`: rewrite the body of every matching method to `return <value>`. Returns how
/// many methods were stubbed.
pub fn returns(text: &mut String, at: &At, value: Option<&str>) -> Result<usize, String> {
    let lines: Vec<&str> = text.lines().collect();
    let hits = methods(&lines, &at.method, at.sig.as_deref());
    let want = at.want();
    if hits.is_empty() {
        return Err(format!("{}: {} has no method {want}", at.note, at.class));
    }
    bodyless(&lines, &hits, at)?;
    let mut out = String::with_capacity(text.len());
    let mut at_line = 0;
    let n = hits.len();
    for &(start, end) in &hits {
        for l in &lines[at_line..=start] {
            out.push_str(l);
            out.push('\n');
        }
        let ret = ret_desc(lines[start]).ok_or_else(|| {
            format!(
                "{}: {}#{want}: no return type in the header",
                at.note, at.class
            )
        })?;
        out.push_str(&stub(ret, value, &at.note)?);
        out.push_str(lines[end]);
        out.push('\n');
        at_line = end + 1;
    }
    for l in &lines[at_line..] {
        out.push_str(l);
        out.push('\n');
    }
    *text = out;
    Ok(n)
}

/// `<replace>`: swap `from` for `to` in the bodies of the matching methods — every occurrence,
/// or with `once` only the first one in each method. `from` has to match within one line of
/// the smali, which is one instruction per line. Returns how many lines changed.
pub fn replaces(
    text: &mut String,
    at: &At,
    from: &str,
    to: &str,
    once: bool,
) -> Result<usize, String> {
    let lines: Vec<&str> = text.lines().collect();
    let hits = methods(&lines, &at.method, at.sig.as_deref());
    let want = at.want();
    if hits.is_empty() {
        return Err(format!("{}: {} has no method {want}", at.note, at.class));
    }
    bodyless(&lines, &hits, at)?;
    let mut out = String::with_capacity(text.len());
    let mut at_line = 0;
    let mut changed = 0;
    let mut found = 0;
    for (start, end) in hits {
        for l in &lines[at_line..=start] {
            out.push_str(l);
            out.push('\n');
        }
        let mut first = true;
        for l in &lines[start + 1..end] {
            found += count(l, from);
            let new = match once {
                // the first line the pattern is on takes the replacement; the rest stay
                true => match first {
                    true => {
                        let n = l.replacen(from, to, 1);
                        first = n == *l;
                        n
                    }
                    false => (*l).to_string(),
                },
                false => l.replace(from, to),
            };
            changed += usize::from(new != *l);
            out.push_str(&new);
            out.push('\n');
        }
        out.push_str(lines[end]);
        out.push('\n');
        at_line = end + 1;
    }
    for l in &lines[at_line..] {
        out.push_str(l);
        out.push('\n');
    }
    if found == 0 {
        return Err(format!(
            "{}: {from:?} is not in {}#{want}",
            at.note, at.class
        ));
    }
    *text = out;
    Ok(changed)
}

/// Occurrences of `from` in `l`, non-overlapping.
fn count(l: &str, from: &str) -> usize {
    if from.is_empty() {
        return 0;
    }
    l.match_indices(from).count()
}

/// The return descriptor of a `.method` header line: `Z` of `.method public isPurchased()Z`.
fn ret_desc(header: &str) -> Option<&str> {
    header
        .split_whitespace()
        .last()?
        .split_once(')')
        .map(|x| x.1)
}

/// The body of a method stubbed to `return <value>`, typed by the method's return descriptor:
/// the header line and `.end method` are the caller's, this is what goes between. `.locals`
/// counts what the stub needs — wide values take a register pair.
pub fn stub(ret: &str, value: Option<&str>, at: &str) -> Result<String, String> {
    let err = |m: String| Err(format!("{at}: {m}"));
    // (instructions, locals)
    let body: (Vec<String>, usize) = match ret {
        "V" => match value {
            None => (vec!["return-void".into()], 0),
            Some(v) => return err(format!("a void method returns nothing (value={v:?})")),
        },
        "Z" => {
            let lit = match value {
                None | Some("false") => 0,
                Some("true") => 1,
                Some(v) => return err(format!("a boolean returns true or false, not {v:?}")),
            };
            (vec![format!("const/4 v0, 0x{lit}"), "return v0".into()], 1)
        }
        "B" | "S" | "C" | "I" => {
            let (lo, hi, what) = match ret {
                "B" => (-128i64, 127, "a byte"),
                "S" => (-0x8000, 0x7fff, "a short"),
                "C" => (0, 0xffff, "a char"),
                _ => (i32::MIN as i64, i32::MAX as i64, "an int"),
            };
            // no value: the type's zero, like `return 0`
            let v = match value {
                None => 0,
                Some(v) => number(v).map_err(|e| format!("{at}: {e}"))?,
            };
            if v < lo || v > hi {
                return err(format!("{what} holds {lo} to {hi}, not {v}"));
            }
            (vec![i32_const(v as i32), "return v0".into()], 1)
        }
        "J" => {
            let v = match value {
                None => 0,
                Some(v) => number(v).map_err(|e| format!("{at}: {e}"))?,
            };
            (vec![wide_const(v), "return-wide v0".into()], 2)
        }
        "F" => {
            let bits = f32_bits(value)?;
            (
                vec![
                    match bits {
                        0 => "const/4 v0, 0x0".into(),
                        v => format!("const v0, 0x{v:08x}"),
                    },
                    "return v0".into(),
                ],
                1,
            )
        }
        "D" => {
            let bits = f64_bits(value)?;
            (
                vec![
                    match bits {
                        0 => "const-wide/16 v0, 0x0".into(),
                        v => format!("const-wide v0, 0x{v:016x}"),
                    },
                    "return-wide v0".into(),
                ],
                2,
            )
        }
        t if t.starts_with('L') || t.starts_with('[') => match value {
            None | Some("null") => (vec!["const/4 v0, 0x0".into(), "return-object v0".into()], 1),
            Some(v) => {
                return err(format!(
                    "{t} is a reference type: value can only be null, not {v:?}"
                ));
            }
        },
        t => return err(format!("{t} is not a return type")),
    };
    let (lines, locals) = body;
    let mut out = format!("    .locals {locals}\n");
    for l in lines {
        out.push_str("    ");
        out.push_str(&l);
        out.push('\n');
    }
    Ok(out)
}

/// `value` as the bits of a float: a decimal number, or 0x hex as raw bits. Absent: 0.0.
fn f32_bits(value: Option<&str>) -> Result<u32, String> {
    Ok(match value {
        None => 0,
        Some(v) => match v.starts_with("0x") {
            true => number(v)? as u32,
            false => v
                .parse::<f32>()
                .map_err(|e| format!("{v:?} is not a float: {e}"))?
                .to_bits(),
        },
    })
}

/// `value` as the bits of a double: a decimal number, or 0x hex as raw bits. Absent: 0.0.
fn f64_bits(value: Option<&str>) -> Result<u64, String> {
    Ok(match value {
        None => 0,
        Some(v) => match v.starts_with("0x") {
            true => number(v)? as u64,
            false => v
                .parse::<f64>()
                .map_err(|e| format!("{v:?} is not a double: {e}"))?
                .to_bits(),
        },
    })
}

/// A smali-style integer: decimal or `0x` hex, optional leading minus.
fn number(v: &str) -> Result<i64, String> {
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v),
    };
    let n = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).map_err(|_| format!("{v:?} is not a number"))?,
        None => v
            .parse::<u64>()
            .map_err(|_| format!("{v:?} is not a number"))?,
    };
    Ok(match neg {
        true => (n as i64).wrapping_neg(),
        false => n as i64,
    })
}

/// The narrowest `const` that holds an int.
fn i32_const(v: i32) -> String {
    match v {
        -8..=7 => format!("const/4 v0, {}", lit(v as i64)),
        -0x8000..=0x7fff => format!("const/16 v0, {}", lit(v as i64)),
        _ => format!("const v0, {}", lit(v as i64)),
    }
}

/// The narrowest `const-wide` that holds a long.
fn wide_const(v: i64) -> String {
    match v {
        -0x8000..=0x7fff => format!("const-wide/16 v0, {}", lit(v)),
        _ => format!("const-wide v0, {}", lit(v)),
    }
}

/// A literal as smali writes it: `0x2a`, or `-0x472f5021`.
fn lit(v: i64) -> String {
    match v {
        0.. => format!("0x{v:x}"),
        _ => format!("-0x{:x}", v.unsigned_abs()),
    }
}

/// `(package, versionName, versionCode)` of a decoded manifest's text.
pub fn manifest_id(text: &str) -> (Option<String>, Option<String>, Option<u32>) {
    let Some(e) = text.split('<').find(|e| e.starts_with("manifest")) else {
        return (None, None, None);
    };
    let e = &e[..e.find('>').unwrap_or(e.len())];
    (
        crate::search::attr(e, "package"),
        crate::search::attr(e, "versionName"),
        crate::search::attr(e, "versionCode").and_then(|v| v.parse().ok()),
    )
}

#[cfg(test)]
#[path = "../tests/apkscope/patch.rs"]
mod tests;
