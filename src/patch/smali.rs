//! The method bodies of a class's smali text: where a `<return>` or `<replace>` edit lands
//! ([`At`], [`methods`]) and the edits themselves ([`returns`], [`replaces`]). Edits are
//! line-based on the text `apkscope smali` prints — one instruction per line — and the class
//! is re-assembled from that text, so no raw DEX bytes are touched.

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
/// constructor keeps the call and stays allowed.
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

#[cfg(test)]
#[path = "../../tests/apkscope/patch_smali.rs"]
mod tests;
