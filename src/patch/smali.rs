//! The method bodies of a class's smali text: where a `<return>`, `<replace>` or `<body>`
//! edit lands ([`At`], [`methods`]) and the edits themselves ([`returns`], [`replaces`],
//! [`bodies`]). Edits are line-based on the text `apkscope smali` prints — one instruction
//! per line — and the class is re-assembled from that text, so no raw DEX bytes are
//! touched: the assembler is the check on everything a body now claims.

use super::stub::stub;

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

/// A stubbed `<init>` cannot call `super()` or `this(...)`, and a constructor that returns
/// without one is a verify error: Android refuses the class at load, and the app dies where
/// it is first used. The whole body — the `super()` call included — is what `<return>`
/// replaces, so it is an error that says what to do instead; `<replace>` inside a
/// constructor keeps the call and stays allowed, and so does `<body>` — [`init_calls`]
/// checks the new body for the call.
fn constructors(lines: &[&str], hits: &[(usize, usize)], at: &At) -> Result<(), String> {
    for &(start, _) in hits {
        if is_init(lines[start]) {
            return Err(format!(
                "{}: {}#{} is a constructor: a stub drops the super() call and Android refuses \
                 the class; <replace> inside it instead",
                at.note,
                at.class,
                at.want()
            ));
        }
    }
    Ok(())
}

/// Whether a `.method` header line is a constructor `<init>`: not `<clinit>`, whose stub is
/// only an empty static initializer and fine.
fn is_init(header: &str) -> bool {
    header
        .split_whitespace()
        .last()
        .is_some_and(|h| h.starts_with("<init>("))
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
    constructors(&lines, &hits, at)?;
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

/// `<body>`: replace the body of every matching method with the given smali. The class is
/// re-assembled from the edited text, so the assembler is the check: a body that does not
/// parse, or a register it never declared, is an error before anything is written, and so is
/// a constructor body without its `super()` call. Returns how many methods were rewritten.
pub fn bodies(text: &mut String, at: &At, body: &str) -> Result<usize, String> {
    let lines: Vec<&str> = text.lines().collect();
    let hits = methods(&lines, &at.method, at.sig.as_deref());
    let want = at.want();
    if hits.is_empty() {
        return Err(format!("{}: {} has no method {want}", at.note, at.class));
    }
    bodyless(&lines, &hits, at)?;
    // the lines as the XML wrote them, without its indentation and blank lines: they are
    // not part of the smali
    let new: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if new.is_empty() {
        return Err(format!(
            "{}: the body of {}#{want} is empty: <return> stubs a method to a constant, \
             and a method with no code at all is <return> without a value",
            at.note, at.class
        ));
    }
    // a body says how many registers it uses, like the smali apkscope prints; without the
    // line the registers are whatever the parameters left, which is not what a hand-written
    // body means
    match new[0].split_once(' ') {
        Some((".locals", _)) | Some((".registers", _)) => {}
        _ => {
            return Err(format!(
                "{}: the body of {}#{want} must start with `.locals N` (or `.registers N`), \
                 which says how many registers it uses",
                at.note, at.class
            ));
        }
    }
    init_calls(&new, &lines, &hits, at)?;
    let mut out = String::with_capacity(text.len());
    let mut at_line = 0;
    let n = hits.len();
    for &(start, end) in &hits {
        for l in &lines[at_line..=start] {
            out.push_str(l);
            out.push('\n');
        }
        // the printer's bodies are four spaces in; the new code is indented to match, so
        // the text a body lands in reads like the text around it
        for l in &new {
            out.push_str("    ");
            out.push_str(l);
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
    *text = out;
    Ok(n)
}

/// A new constructor body must call `super()` or `this(...)`: an `<init>` that returns
/// without one is a verify error and Android refuses the class. The call is an
/// `invoke-direct` on `p0` to an `<init>`, so that is what the body is checked for — an
/// `invoke-direct <init>` on another register only builds some other object, and does not
/// count.
fn init_calls(
    new: &[&str],
    lines: &[&str],
    hits: &[(usize, usize)],
    at: &At,
) -> Result<(), String> {
    for &(start, _) in hits {
        if is_init(lines[start])
            && !new.iter().any(|l| {
                let l = l.trim_start();
                l.starts_with("invoke-direct") && l.contains("{p0") && l.contains("-><init>(")
            })
        {
            return Err(format!(
                "{}: the new body of {}#{} never calls super() or this(...): a constructor \
                 needs `invoke-direct {{p0}}, ...-><init>()` before it returns, or Android \
                 refuses the class",
                at.note,
                at.class,
                at.want()
            ));
        }
    }
    Ok(())
}

/// `<replace>`: swap `from` for `to` in the bodies of the matching methods — every
/// occurrence, or with `once` only the first one in each method. A `from` of one line
/// matches within a line of the smali, which is one instruction per line; a `from` of
/// several lines names the lines exactly (leading and trailing spaces aside — the lines as
/// `apkscope smali` prints them), and the consecutive block it names is replaced by `to`'s
/// lines, as many or few as the new code needs, an empty `to` deleting the block. Returns
/// how many lines changed.
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
    // the lines of `from` and `to` as the XML wrote them, without its indentation; the XML
    // cannot be the indentation of the smali it lands in
    let from_l: Vec<&str> = from
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let to_l: Vec<&str> = to
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if from_l.is_empty() {
        return Err(format!("{}: <replace> from is empty", at.note));
    }
    // a piece of one line cannot become several: where the rest of the line would go is not
    // a thing the text says, and guessing would edit code nobody asked for
    if from_l.len() == 1 && to_l.len() > 1 {
        return Err(format!(
            "{}: a from of one line needs a to of one line: {} lines cannot replace a piece \
             of one; a from of several lines is what a to of several replaces",
            at.note,
            to_l.len()
        ));
    }
    let mut out = String::with_capacity(text.len());
    let mut at_line = 0;
    let mut changed = 0;
    let mut found = 0;
    for (start, end) in hits {
        for l in &lines[at_line..=start] {
            out.push_str(l);
            out.push('\n');
        }
        let body = &lines[start + 1..end];
        let mut i = 0;
        let mut first = true;
        while i < body.len() {
            // one line: a piece of the line; several: the lines exactly
            if from_l.len() == 1 {
                let l = body[i];
                let to_one = to_l.first().copied().unwrap_or("");
                found += count(l, from_l[0]);
                let new = match (once, first) {
                    // the first line the pattern is on takes the replacement; the rest stay
                    (true, true) => {
                        let n = l.replacen(from_l[0], to_one, 1);
                        first = n == *l;
                        n
                    }
                    (true, false) => (*l).to_string(),
                    (false, _) => l.replace(from_l[0], to_one),
                };
                changed += usize::from(new != *l);
                out.push_str(&new);
                out.push('\n');
                i += 1;
            } else if i + from_l.len() <= body.len()
                && body[i..i + from_l.len()]
                    .iter()
                    .zip(&from_l)
                    .all(|(l, f)| l.trim() == *f)
                && !(once && !first)
            {
                // the block, replaced whole: `to`'s lines under the indent of the line the
                // block starts at
                found += 1;
                let indent = &body[i][..body[i].len() - body[i].trim_start().len()];
                for l in &to_l {
                    out.push_str(indent);
                    out.push_str(l);
                    out.push('\n');
                }
                changed += from_l.len();
                i += from_l.len();
                first = false;
            } else {
                out.push_str(body[i]);
                out.push('\n');
                i += 1;
            }
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

#[cfg(test)]
#[path = "../../tests/apkscope/patch_smali.rs"]
mod tests;
