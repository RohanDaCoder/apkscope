//! Reading decompiled Java as text: declarations, the word or member under the cursor, and jumping to a definition.

use super::*;

pub(super) fn java_res() -> &'static [regex::Regex; 3] {
    use std::sync::OnceLock;
    static RE: OnceLock<[regex::Regex; 3]> = OnceLock::new();
    RE.get_or_init(|| {
        [
            regex::Regex::new(
                r"^(\s*)(?:[\w@]+\s+)*(?:class|interface|enum|@interface)\s+([\w$]+)",
            )
            .unwrap(),
            regex::Regex::new(
                r"^(\s+)(?:[\w$<>\[\],.?@]+\s+)*?([\w$]+)\((.*)\)\s*(?:throws [^{;]+)?\s*[{;]\s*$",
            )
            .unwrap(),
            regex::Regex::new(r"^(\s+)(?:[\w$<>\[\],.?]+\s+)+([\w$]+)(?: = .*)?;(?:\s*//.*)?$")
                .unwrap(),
        ]
    })
}

pub(super) const NOT_DECL: [&str; 9] = [
    "if",
    "while",
    "for",
    "switch",
    "catch",
    "synchronized",
    "return",
    "new",
    "super",
];

/// A method_re match is a declaration, not a call statement: a type or modifier comes first.
pub(super) fn is_decl(l: &str, c: &regex::Captures) -> bool {
    let pre = l[..c.get(2).map_or(0, |m| m.start())].trim();
    let first = pre.split_whitespace().next().unwrap_or("");
    // a constructor without modifiers: `Name(args) {`
    let ctor = pre.is_empty()
        && c[2].starts_with(|ch: char| ch.is_uppercase())
        && l.trim_end().ends_with('{');
    (ctor || !pre.is_empty())
        && !pre.ends_with('.')
        && !NOT_DECL.contains(&&c[2])
        && !NOT_DECL.contains(&first)
        && first != "throw"
        && first != "else"
        && !l.contains(" = ")
}

pub(super) fn indent(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

/// What sits on line `at` of decompiled Java: the chain of enclosing classes (as shown) and the
/// member declared there, as (is_method, shown name). A comment above a declaration (such as a
/// WARNING) points at that declaration.
/// (class chain, member: (is a method, name)) under a position in Java text
pub(super) type MemberAt = (Vec<String>, Option<(bool, String)>);

pub(super) fn member_at(text: &str, at: usize) -> Option<MemberAt> {
    let [class_re, method_re, field_re] = java_res();
    let lines: Vec<&str> = text.lines().collect();
    let mut at = at.min(lines.len().checked_sub(1)?);
    while at + 1 < lines.len() && {
        let t = lines[at].trim_start();
        t.starts_with("/*") || t.starts_with('*') || t.starts_with("//") || t.starts_with('@')
    } {
        at += 1;
    }
    let l = lines[at];
    let (member, own_indent) = if let Some(c) = class_re.captures(l) {
        (None, c[1].len() + 4)
    } else if let Some(c) = method_re.captures(l).filter(|c| is_decl(l, c)) {
        (Some((true, c[2].to_string())), c[1].len())
    } else {
        let c = field_re
            .captures(l)
            .filter(|c| !NOT_DECL.contains(&&c[2]))?;
        (Some((false, c[2].to_string())), c[1].len())
    };
    // enclosing classes, innermost last
    let mut chain = Vec::new();
    let mut want = own_indent.checked_sub(4)?;
    for k in (0..=at).rev() {
        if let Some(c) = class_re
            .captures(lines[k])
            .filter(|_| indent(lines[k]) == want)
        {
            chain.push(c[2].to_string());
            if want == 0 {
                break;
            }
            want -= 4;
        }
    }
    chain.reverse();
    (!chain.is_empty() && want == 0).then_some((chain, member))
}

/// Classes enclosing any line (a statement inside a method too), innermost last.
pub(super) fn classes_around(text: &str, at: usize) -> Vec<String> {
    let class_re = &java_res()[0];
    let lines: Vec<&str> = text.lines().collect();
    let mut cur = lines.get(at).map_or(0, |l| indent(l)) + 1;
    let mut chain = Vec::new();
    for k in (0..=at.min(lines.len().saturating_sub(1))).rev() {
        if let Some(c) = class_re
            .captures(lines[k])
            .filter(|_| indent(lines[k]) < cur)
        {
            chain.push(c[2].to_string());
            cur = indent(lines[k]);
            if cur == 0 {
                break;
            }
        }
    }
    chain.reverse();
    chain
}

/// 1-based line of the declaration of method `name` (with `nargs` parameters, if given).
pub(super) fn find_decl(text: &str, name: &str, nargs: Option<usize>) -> Option<usize> {
    let re = &java_res()[1];
    let mut first = None;
    for (k, l) in text.lines().enumerate() {
        let Some(c) = re.captures(l) else { continue };
        if &c[2] != name || !is_decl(l, &c) {
            continue;
        }
        let n = if c[3].trim().is_empty() {
            0
        } else {
            c[3].matches(',').count() + 1
        };
        if nargs.is_none_or(|w| w == n) {
            return Some(k + 1);
        }
        first.get_or_insert(k + 1);
    }
    first
}

/// Line to show for a target: the declaration, then the first line after it with `then`.
pub(super) fn locate(text: &str, t: &Target) -> Option<usize> {
    let decl = t.decl.as_ref().and_then(|(n, a)| find_decl(text, n, *a));
    match (&t.then, decl) {
        (Some(w), Some(d)) => {
            // stay inside the method: stop at the next line indented like its declaration
            let lines: Vec<&str> = text.lines().collect();
            let ind = indent(lines[d - 1]);
            let hit = lines
                .iter()
                .enumerate()
                .skip(d)
                .take_while(|(_, l)| l.trim().is_empty() || indent(l) > ind || l.trim() == "}")
                .find(|(_, l)| l.contains(w.as_str()));
            Some(hit.map_or(d, |(k, _)| k + 1))
        }
        (Some(w), None) => find_next(text, w, 0),
        (None, d) => d,
    }
}

/// Identifier at (line, column) and what surrounds it: (word, followed by `(`, after `new `,
/// receiver before a `.`).
pub(super) fn word_at(
    text: &str,
    line: usize,
    col: usize,
) -> Option<(String, bool, bool, Option<String>)> {
    let l: Vec<char> = text.lines().nth(line)?.chars().collect();
    let w = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    if !l.get(col).is_some_and(|&c| w(c)) {
        return None;
    }
    let (mut s, mut e) = (col, col);
    while s > 0 && w(l[s - 1]) {
        s -= 1;
    }
    while e < l.len() && w(l[e]) {
        e += 1;
    }
    let word: String = l[s..e].iter().collect();
    if word.chars().next()?.is_ascii_digit() {
        return None;
    }
    let call = l.get(e) == Some(&'(');
    let before: String = l[..s].iter().collect();
    let is_new = before.ends_with("new ");
    let recv = before
        .strip_suffix('.')
        .map(|b| {
            b.chars()
                .rev()
                .take_while(|&c| w(c))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>()
        })
        .filter(|r| !r.is_empty());
    Some((word, call, is_new, recv))
}

/// Top-level class (as listed) a simple name in `text` refers to: by its import, the tab's own
/// package, or a unique name in the APK.
pub(super) fn resolve_class(st: &State, text: &str, top: &str, simple: &str) -> Option<String> {
    let has = |n: &str| st.classes.binary_search_by(|c| c.0.as_str().cmp(n)).is_ok();
    let suffix = format!(".{simple};");
    if let Some(imp) = text
        .lines()
        .take_while(|l| !l.contains(" class ") && !l.contains("interface "))
        .find(|l| l.starts_with("import ") && l.ends_with(&suffix))
    {
        let n = imp.trim_start_matches("import ").trim_end_matches(';');
        if has(n) {
            return Some(n.to_string());
        }
    }
    let same = format!("{}.{simple}", top.rsplit_once('.').map_or("", |x| x.0));
    if has(&same) {
        return Some(same);
    }
    let mut it = st
        .classes
        .iter()
        .filter(|c| c.0.rsplit('.').next() == Some(simple));
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c.0.clone()),
        _ => None,
    }
}

/// Real descriptor of a class shown as the last of `chain` in tab `top`.
pub(super) fn owner_desc(st: &State, top: &str, chain: &[String]) -> Option<String> {
    let &(_, d, c) = st.classes.iter().find(|x| x.0 == top)?;
    let t = st.dexes[d].class_descriptor(c).ok()?;
    Some(if chain.len() > 1 {
        crate::re::nested_class(&st.dexes, &t, chain.last()?)
    } else {
        t
    })
}

pub(super) fn desc_of(st: &State, java: &str) -> Option<String> {
    let &(_, d, c) = st.classes.iter().find(|x| x.0 == java)?;
    st.dexes[d].class_descriptor(c).ok()
}

/// Ctrl+click: go to what the word names (class, or method declared in this file / its class).
pub(super) fn definition(
    st: &State,
    top: &str,
    text: &str,
    line: usize,
    col: usize,
) -> Option<Act> {
    let (word, call, is_new, recv) = word_at(text, line, col)?;
    if call && !is_new {
        let in_file = recv.as_deref().is_none_or(|r| r == "this" || r == "super");
        if in_file {
            return find_decl(text, &word, None).map(Act::Line);
        }
        let cls = recv
            .as_deref()
            .and_then(|r| resolve_class(st, text, top, r))?;
        return Some(Act::OpenClass(
            cls,
            Some(Target {
                decl: Some((word, None)),
                then: None,
            }),
        ));
    }
    if word.starts_with(|c: char| c.is_uppercase()) {
        if let Some(c) = resolve_class(st, text, top, &word) {
            return Some(Act::OpenClass(c, None));
        }
        // a nested class of this file
        return text
            .lines()
            .position(|l| java_res()[0].captures(l).is_some_and(|c| c[2] == word))
            .map(|k| Act::Line(k + 1));
    }
    None
}
