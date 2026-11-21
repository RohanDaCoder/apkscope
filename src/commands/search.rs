//! Indexed + cached commands: search, info, resource.
//!
//! `SearchOpts` / `search_args` / `search_on` are shared with the MCP server, which
//! parses its own tool-call argument vectors the same way the CLI used to.

use std::fmt::Write as _;
use std::io::Write as _;

use crate::cli::{self, InfoArgs, ResourceArgs, SearchArgs};
use crate::session;

/// Parsed `search` options; `pos` keeps the positional words (apk, scope, query...).
pub struct SearchOpts {
    pub pos: Vec<String>,
    pub regex: bool,
    pub case: bool,
    pub method: bool,
    pub app_only: bool,
    /// include library code even in the scopes that default to app-only
    pub all: bool,
    pub ctx: usize,
    pub fmt: session::Format,
    pub out: Option<String>,
}

pub fn search_args(args: &[String]) -> Result<SearchOpts, String> {
    let mut s = SearchOpts {
        pos: Vec::new(),
        regex: false,
        case: false,
        method: false,
        app_only: false,
        all: false,
        ctx: 0,
        fmt: session::Format::Text,
        out: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--regex" => s.regex = true,
            "--case" => s.case = true,
            "--method" => s.method = true,
            "--app-only" => s.app_only = true,
            "--all" => s.all = true,
            "--context" => {
                s.ctx = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--context needs a number")?
            }
            "--format" => {
                s.fmt = match it.next().map(String::as_str) {
                    Some("txt") => session::Format::Text,
                    Some("csv") => session::Format::Csv,
                    Some("json") => session::Format::Json,
                    _ => return Err("--format is txt, csv or json".into()),
                }
            }
            "--out" | "-o" => s.out = Some(it.next().ok_or("--out needs a file")?.clone()),
            _ => s.pos.push(a.clone()),
        }
    }
    Ok(s)
}

/// Run one search on a loaded session; `words` = [scope, query...]. Returns (text, hits, note).
pub fn search_on(
    st: &session::State,
    a: &SearchOpts,
    words: &[String],
) -> Result<(String, usize, String), String> {
    let [scope, rest @ ..] = words else {
        return Err("search needs a scope".into());
    };
    let scope = session::Scope::parse(scope).ok_or_else(|| {
        format!("unknown scope {scope}: code string usages class res urls http secrets api")
    })?;
    let mut m = session::Matcher::new(&rest.join(" "), scope, a.regex, a.case)?;
    // the API scopes default to app code only; --all opts back out, --app-only still works
    m.app_only = a.app_only || (scope.query_optional() && !a.all);
    if scope.decompiles() && st.index.get().is_none() {
        st.build_index();
    }
    let (hits, hidden) = session::search_stats(st, &m, &session::Progress::default());
    let mut text = session::export(&hits, a.ctx, a.method, a.fmt, |x| st.target_text(x));
    if a.fmt == session::Format::Text {
        let head = session::search_summary(st, scope, &hits);
        if !head.is_empty() {
            text = head.join("\n") + "\n\n" + &text;
        }
    }
    let mut n = hits.len();
    if scope == session::Scope::Secrets {
        n += secrets_entropy_tier(st, a, &mut text);
    }
    let mut note = scope.note().to_string();
    if hidden > 0 {
        let _ = write!(
            note,
            "\napp-only: skipped {hidden} library class{} (--all to include them)",
            if hidden == 1 { "" } else { "es" }
        );
    }
    Ok((text, n, note))
}

/// Append the labelled entropy tier of `search secrets` to the exported output. Kept out of
/// the pattern hits on purpose: its own section in text output, trailing rows in csv/json.
/// Returns how many entries it added.
fn secrets_entropy_tier(st: &session::State, a: &SearchOpts, text: &mut String) -> usize {
    let found = session::entropy_secrets(st);
    if found.is_empty() {
        return 0;
    }
    let line = |c: &session::Const| {
        format!(
            "possible secret (entropy): {}#{} = \"{}\"",
            c.class, c.field, c.value
        )
    };
    match a.fmt {
        session::Format::Text => {
            let _ = write!(
                text,
                "\n// possible secrets (entropy, app code only, {} shown) — lower confidence:\n// token-shaped, mixed, high-entropy constants of the app that no known pattern matched. A hit is not proof.\n",
                found.len()
            );
            for c in &found {
                let _ = writeln!(text, "{}#{} = \"{}\"", c.class, c.field, c.value);
            }
        }
        session::Format::Csv => {
            for c in &found {
                let (t, m) = (format!("{}#{}", c.class, c.field), line(c));
                let _ = writeln!(
                    text,
                    "{},{},{},{}",
                    session::csv_field(&t),
                    0,
                    session::csv_field(&m),
                    session::csv_field(&m)
                );
            }
        }
        session::Format::Json => {
            let rows: Vec<String> = found
                .iter()
                .map(|c| {
                    let (t, m) = (format!("{}#{}", c.class, c.field), line(c));
                    format!(
                        "  {{\"target\":{},\"line\":0,\"match\":{},\"excerpt\":{}}}",
                        session::json_str(&t),
                        session::json_str(&m),
                        session::json_str(&m)
                    )
                })
                .collect();
            session::json_extend(text, &rows);
        }
    }
    found.len()
}

/// `apkscope search`: same engine as the viewer; results go to stdout or `--out`.
pub fn search(a: &SearchArgs, deobf: bool) -> Result<(), String> {
    let fmt = match a.format.as_str() {
        "txt" => session::Format::Text,
        "csv" => session::Format::Csv,
        "json" => session::Format::Json,
        _ => return Err("--format is txt, csv or json".into()),
    };
    let words: Vec<String> = std::iter::once(a.scope.clone())
        .chain(a.query.iter().cloned())
        .collect();
    let sa = SearchOpts {
        pos: Vec::new(),
        regex: a.regex,
        case: a.case,
        method: a.method,
        app_only: a.app_only,
        all: a.all,
        ctx: a.context,
        fmt,
        out: a.out.clone(),
    };
    let t = std::time::Instant::now();
    let st = cli::load(&a.apk, deobf)?;
    let loaded = t.elapsed();
    let (text, n, note) = search_on(&st, &sa, &words)?;
    match &sa.out {
        Some(f) => std::fs::write(f, text).map_err(|e| format!("{f}: {e}"))?,
        None => {
            let _ = std::io::stdout().lock().write_all(text.as_bytes());
        }
    }
    eprintln!(
        "{n} hits | load {:.2}s, index + search {:.2}s{}",
        loaded.as_secs_f64(),
        (t.elapsed() - loaded).as_secs_f64(),
        if st.cached() {
            ", code cache present"
        } else {
            ""
        }
    );
    eprintln!("note: {note}");
    Ok(())
}

pub fn info(a: &InfoArgs, deobf: bool) -> Result<(), String> {
    cli::load(&a.apk, deobf).and_then(|st| cli::emit(&session::overview(&st)))
}

pub fn resource(a: &ResourceArgs, deobf: bool) -> Result<(), String> {
    cli::load(&a.apk, deobf).and_then(|st| {
        cli::emit(
            &st.target_text(&a.res)
                .ok_or_else(|| format!("{} not found", a.res))?,
        )
    })
}
