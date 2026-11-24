//! Syntax colouring of Java and Smali, and find-next in a document.

use super::*;

pub(super) const KEYWORDS: &[&str] = &[
    "abstract",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "void",
    "volatile",
    "while",
    "true",
    "false",
    "null",
];

/// Java/Smali/XML colouring: comments, strings, keywords, numbers, smali directives.
/// `mark` (1-based) gets a background, for jumping to a search hit.
/// `sel` = identifier to mark everywhere (the word the user clicked).
pub(super) fn highlight(
    text: &str,
    p: &Palette,
    size: f32,
    mark: Option<usize>,
    sel: Option<&str>,
) -> LayoutJob {
    let font = FontId::monospace(size);
    let mut job = LayoutJob::default();
    let mut line = 1;
    let mut push = |job: &mut LayoutJob, s: &str, k: u8| {
        // split at newlines so only the marked line gets the background
        for piece in s.split_inclusive('\n') {
            let mut f = TextFormat::simple(font.clone(), p.code[(k % 6) as usize]);
            if Some(line) == mark {
                f.background = p.mark;
            }
            // k >= 6: the selected word
            if k >= 6 {
                f.background = p.accent.gamma_multiply(0.38);
            }
            job.append(piece, 0.0, f);
            if piece.ends_with('\n') {
                line += 1;
            }
        }
    };
    let b = text.as_bytes();
    let (mut i, mut start) = (0, 0);
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    while i < b.len() {
        let c = b[i];
        let tok_end = if text[i..].starts_with("/*") {
            let e = text[i + 2..].find("*/").map_or(b.len(), |p| i + 4 + p);
            Some((e, if text[i..e].contains("WARNING") { 5 } else { 1 }))
        } else if text[i..].starts_with("//") || (c == b'#' && (i == 0 || b[i - 1] == b'\n')) {
            Some((text[i..].find('\n').map_or(b.len(), |p| i + p), 1))
        } else if c == b'"' {
            let mut e = i + 1;
            while e < b.len() && b[e] != b'"' && b[e] != b'\n' {
                e += if b[e] == b'\\' { 2 } else { 1 };
            }
            Some(((e + 1).min(b.len()), 2))
        } else if c == b'.'
            && (i == 0 || !word(b[i - 1]))
            && b.get(i + 1).is_some_and(|c| c.is_ascii_lowercase())
        {
            let e = i
                + 1
                + b[i + 1..]
                    .iter()
                    .position(|&c| !word(c))
                    .unwrap_or(b.len() - i - 1);
            Some((e, 5))
        } else if word(c) && (i == 0 || !word(b[i - 1])) {
            let e = i + b[i..].iter().position(|&c| !word(c)).unwrap_or(b.len() - i);
            let w = &text[i..e];
            if Some(w) == sel {
                Some((e, 6))
            } else if KEYWORDS.contains(&w) {
                Some((e, 3))
            } else if c.is_ascii_digit() {
                Some((e, 4))
            } else {
                i = e;
                continue;
            }
        } else {
            None
        };
        match tok_end {
            Some((e, k)) => {
                if start < i {
                    push(&mut job, &text[start..i], 0);
                }
                push(&mut job, &text[i..e], k);
                i = e;
                start = e;
            }
            None => i += text[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    if start < b.len() {
        push(&mut job, &text[start..], 0);
    }
    job
}

/// Next line after `from` (1-based) containing `needle`, wrapping around.
pub(super) fn find_next(text: &str, needle: &str, from: usize) -> Option<usize> {
    let low = needle.to_lowercase();
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    (0..n)
        .map(|k| (from + k) % n)
        .find(|&k| lines[k].to_lowercase().contains(&low))
        .map(|k| k + 1)
}
