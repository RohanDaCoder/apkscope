//! Indexed + cached commands: search, info, resource.
//!
//! `SearchOpts` / `search_args` / `search_on` are shared with the MCP server, which
//! parses its own tool-call argument vectors the same way the CLI used to.

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
) -> Result<(String, usize, &'static str), String> {
    let [scope, rest @ ..] = words else {
        return Err("search needs a scope".into());
    };
    let scope = session::Scope::parse(scope).ok_or_else(|| {
        format!("unknown scope {scope}: code string usages class res urls http secrets")
    })?;
    let mut m = session::Matcher::new(&rest.join(" "), scope, a.regex, a.case)?;
    m.app_only = a.app_only;
    if scope.decompiles() && st.index.get().is_none() {
        st.build_index();
    }
    let hits = session::search(st, &m, &session::Progress::default());
    Ok((
        session::export(&hits, a.ctx, a.method, a.fmt, |x| st.target_text(x)),
        hits.len(),
        scope.note(),
    ))
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
