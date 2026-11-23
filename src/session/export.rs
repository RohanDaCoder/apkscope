//! Search-hit export: text / csv / json rendering of hits with their code excerpts,
//! and the helper that appends rows to a JSON array.

use std::fmt::Write as _;

use crate::search::{Format, Hit, csv_field, excerpt, json_str};

/// Write hits with their surrounding code. `text_of` gives a target's full text.
pub fn export(
    hits: &[Hit],
    ctx: usize,
    method: bool,
    fmt: Format,
    mut text_of: impl FnMut(&str) -> Option<String>,
) -> String {
    let mut out = String::new();
    match fmt {
        Format::Csv => out += "target,line,match,excerpt\n",
        Format::Json => out += "[\n",
        Format::Text => {}
    }
    let mut cur: Option<(String, Option<String>)> = None;
    for (n, (t, ln, text)) in hits.iter().enumerate() {
        if cur.as_ref().is_none_or(|c| &c.0 != t) {
            cur = Some((t.clone(), if ctx > 0 || method { text_of(t) } else { None }));
        }
        let body = cur.as_ref().and_then(|c| c.1.as_deref());
        let snippet = match (body, *ln) {
            (Some(b), l) if l > 0 => {
                let lines: Vec<&str> = b.lines().collect();
                let (a, z) = excerpt(&lines, l - 1, ctx, method);
                lines[a..z].join("\n")
            }
            _ => text.clone(),
        };
        match fmt {
            Format::Text => {
                let _ = writeln!(out, "== {t}:{ln}\n{snippet}\n");
            }
            Format::Csv => {
                let _ = writeln!(
                    out,
                    "{},{ln},{},{}",
                    csv_field(t),
                    csv_field(text),
                    csv_field(&snippet)
                );
            }
            Format::Json => {
                let sep = if n + 1 < hits.len() { "," } else { "" };
                let _ = writeln!(
                    out,
                    "  {{\"target\":{},\"line\":{ln},\"match\":{},\"excerpt\":{}}}{sep}",
                    json_str(t),
                    json_str(text),
                    json_str(&snippet)
                );
            }
        }
    }
    if fmt == Format::Json {
        out += "]\n";
    }
    out
}

/// Append `rows` (already-rendered, indented JSON objects) to an array produced by
/// `export(.., Format::Json, ..)`; the array stays a valid JSON array.
pub fn json_extend(array: &mut String, rows: &[String]) {
    if rows.is_empty() {
        return;
    }
    let body = array.trim_end();
    let had_rows = body.ends_with('}');
    let open = body.strip_suffix(']').unwrap_or(body);
    let mut out = String::from(open);
    for (k, r) in rows.iter().enumerate() {
        if k > 0 || had_rows {
            out.push(',');
            out.push('\n');
        }
        out.push_str(r);
    }
    out.push_str("\n]\n");
    *array = out;
}
