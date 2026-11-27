//! The bundle as the XML says it: [`Bundle`], its named patches and their ops, and [`parse`],
//! which reads the format strictly — every element and attribute is checked against what the
//! format understands, so a typo is an error that names its line, not an edit that silently
//! does nothing.

use apkscope_core::markup::{Element, parse_xml};

use super::target::Target;

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

pub(super) fn req(e: &Element, k: &str) -> Result<String, String> {
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
pub(super) fn only(e: &Element, known: &[&str]) -> Result<(), String> {
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
pub(super) fn no_children(e: &Element) -> Result<(), String> {
    match e.elements().next() {
        Some(c) => Err(format!(
            "line {}: <{}> has nothing inside, found <{}>",
            e.line, e.name, c.name
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "../../tests/apkscope/patch_bundle.rs"]
mod tests;
