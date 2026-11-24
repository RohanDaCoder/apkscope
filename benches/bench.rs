//! The `cargo bench` target (#64): build apkscope at one or more git refs — or take
//! ready-made binaries — run its subcommands on a set of APKs as fresh processes, and
//! print the Markdown comparison table that PR bodies paste by hand — replacing what
//! the retired `bench/*.ps1` and `bench/*.py` scripts measured, without Python,
//! PowerShell or a JDK on the benching machine.
//!
//! ```text
//! cargo bench -- --compare main --compare HEAD --apk bench/apps/TikTok.apk
//! cargo bench -- --pr 79 --apk Telegram.apk
//! cargo bench -- --bin ~/bin/apkscope --bin ~/.local/bin/apkscope-pr-79 --apk Telegram.apk
//! ```
//!
//! The `--` matters: `--compare` is this harness's flag, not cargo's, so it goes after
//! cargo's own separator. Plain `cargo bench` also works and measures the working tree
//! alone. Columns are the refs (built under `target/bench/`, worktrees under
//! `target/bench/wt/`), the pull requests `--pr` fetches (GitHub's merge ref, kept
//! under `refs/bench/`), the ready-made binaries `--bin` points at (measured as given,
//! no build — two apkscope builds to race, e.g. an `apkscope-pr-<N>` preview from
//! `scripts/preview-pr.sh`), and any `--tool` binaries found on PATH. Rows are the case
//! set `BENCHMARKS.md` reports: `info`, `classes`, `strings`, `decompile` (one class),
//! `smali` (one class), `build`, and the whole-APK row. Every cell is `time, peak RSS`,
//! the median of `--runs` cold runs (`-q --no-cache`); times below a second print in
//! milliseconds, above it in seconds.
//!
//! `--hash` adds an output-equivalence check: each run's stdout — for `build`, the
//! rebuilt APK's entry list and CRCs — is digested with SHA-256, and when several refs
//! are compared, differing digests are reported and the bench exits 3, so a PR that
//! changes output while chasing speed is caught next to the numbers.
//!
//! `--format json` dumps the same measurements machine-readably; `--format svg` writes
//! bar charts to `docs/bench-<apk>-{time,ram}.svg`.
//!
//! Exit codes: 0 = every run succeeded; 1 = a run failed; 2 = setup error; 3 = `--hash`
//! found differing outputs (cargo reports that as a failed bench — that is the point).

use std::{
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Instant,
};

use clap::{Parser, ValueEnum};
use sha2::{Digest as _, Sha256};

// ───────────────────────────── command line ─────────────────────────────

#[derive(Parser)]
#[command(
    name = "apkscope-bench",
    bin_name = "cargo bench --",
    about = "build apkscope at git refs or take given binaries, measure its subcommands on APKs, print the PR table",
    after_help = "run as: cargo bench -- [OPTIONS]  (the -- passes OPTIONS to this harness)"
)]
struct Opts {
    /// git ref to build and measure; repeatable — one column each, the first is the
    /// baseline. `HEAD` (or `.`) builds the working tree, uncommitted changes included.
    /// Default: the working tree alone — unless `--bin` or `--pr` name other columns.
    #[arg(long, value_name = "REF")]
    compare: Vec<String>,

    /// ready-made apkscope binary to measure as-is, no build; repeatable — two local
    /// builds to race, or a real install next to an `apkscope-pr-<N>` preview from
    /// `scripts/preview-pr.sh`
    #[arg(long, value_name = "PATH")]
    bin: Vec<PathBuf>,

    /// pull request to compare against its base: fetch GitHub's merge ref for PR N —
    /// the same merge CI builds — into `refs/bench/pr-N` and bench it as a `pr-N`
    /// column; repeatable. Without `--compare`, the base ref (`--base`) comes first
    #[arg(long = "pr", value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pr: Vec<u64>,

    /// the ref the `--pr` columns are judged against — the first column when `--pr`
    /// runs without `--compare`; default `origin/main`, the tip GitHub merges them into
    #[arg(long, value_name = "REF", default_value = "origin/main")]
    base: String,

    /// APK to measure; repeatable. Default: every app in bench/apps/ (APKs are not
    /// committed, so usually this is passed explicitly).
    #[arg(long, value_name = "PATH")]
    apk: Vec<PathBuf>,

    /// external tool to race apkscope against, found on PATH (or an absolute path);
    /// repeatable. Runs `<TOOL> <APK>` in a scratch directory — the whole-APK row only.
    #[arg(long, value_name = "NAME")]
    tool: Vec<String>,

    /// output format: the Markdown table (default), JSON on stdout, or SVG bar charts
    /// into docs/
    #[arg(long, value_name = "FMT", default_value = "markdown")]
    format: Format,

    /// digest every run's output with SHA-256 and, across the apkscope columns
    /// (--compare, --pr, --bin alike), report (and exit 3 on) any case whose output
    /// differs between builds. The `build` case hashes the rebuilt APK's entry list
    /// and CRCs instead of stdout.
    #[arg(long)]
    hash: bool,

    /// runs per case and column; the median lands in the table
    #[arg(long, value_name = "N", default_value_t = 3)]
    runs: usize,

    /// only these cases (repeatable): info, classes, strings, decompile, smali, build,
    /// whole
    #[arg(long = "case", value_name = "NAME")]
    only_cases: Vec<String>,

    /// cargo's own bench-mode marker, injected when `cargo bench` runs this binary:
    /// accepted and ignored so plain `cargo bench -- <flags>` works
    #[arg(long, hide = true)]
    bench: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Markdown,
    Json,
    Svg,
}

// ───────────────────────────── the case set ─────────────────────────────

struct CaseDef {
    id: &'static str,
    /// row label, in the PR-table shape: `apkscope info`, `apkscope decompile` (one class)…
    label: &'static str,
    /// the case needs a class name picked from `classes` output
    needs_class: bool,
    /// only apkscope does it; `--tool` columns show —
    apkscope_only: bool,
}

const CASES: &[CaseDef] = &[
    CaseDef {
        id: "info",
        label: "`apkscope info`",
        needs_class: false,
        apkscope_only: true,
    },
    CaseDef {
        id: "classes",
        label: "`apkscope classes`",
        needs_class: false,
        apkscope_only: true,
    },
    CaseDef {
        id: "strings",
        label: "`apkscope strings`",
        needs_class: false,
        apkscope_only: true,
    },
    CaseDef {
        id: "decompile",
        label: "`apkscope decompile` (one class)",
        needs_class: true,
        apkscope_only: true,
    },
    CaseDef {
        id: "smali",
        label: "`apkscope smali` (one class)",
        needs_class: true,
        apkscope_only: true,
    },
    CaseDef {
        id: "build",
        label: "`apkscope build`",
        needs_class: false,
        apkscope_only: true,
    },
    // the whole-APK row: `apkscope decompile <apk>` for apkscope, `<tool> <apk>` for tools
    CaseDef {
        id: "whole",
        label: "the whole APK",
        needs_class: false,
        apkscope_only: false,
    },
];

fn case_def(id: &str) -> Option<&'static CaseDef> {
    CASES.iter().find(|c| c.id == id)
}

/// The argv (after the binary) that runs one case. `class` is the class picked for this
/// APK, `out` the APK path the `build` case writes.
fn apkscope_args(id: &str, apk: &Path, class: Option<&str>, out: Option<&Path>) -> Vec<String> {
    let mut a = vec!["-q".to_string(), "--no-cache".to_string()];
    match id {
        "info" => a.extend(["info".into(), apk.display().to_string()]),
        "classes" => a.extend(["classes".into(), apk.display().to_string()]),
        "strings" => a.extend(["strings".into(), apk.display().to_string()]),
        "decompile" => {
            a.extend([
                "decompile".into(),
                apk.display().to_string(),
                class.unwrap_or_default().into(),
            ]);
        }
        "smali" => a.extend([
            "smali".into(),
            apk.display().to_string(),
            class.unwrap_or_default().into(),
        ]),
        "build" => a.extend([
            "build".into(),
            apk.display().to_string(),
            out.expect("build case output path").display().to_string(),
        ]),
        "whole" => a.extend(["decompile".into(), apk.display().to_string()]),
        _ => unreachable!("case ids come from CASES"),
    }
    a
}

// ───────────────────────────── columns: build refs, find tools ─────────────────────────────

enum ColKind {
    /// an apkscope binary built at a git ref
    Apkscope { sha: String },
    /// an apkscope binary measured exactly as given (`--bin`), its `--version` probed
    /// up front
    Bin { version: String },
    /// an external tool raced in the whole-APK row
    Tool,
}

struct Column {
    name: String,
    kind: ColKind,
    exe: PathBuf,
}

impl Column {
    fn is_apkscope(&self) -> bool {
        matches!(self.kind, ColKind::Apkscope { .. } | ColKind::Bin { .. })
    }
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "apkscope.exe"
    } else {
        "apkscope"
    }
}

fn build_columns(repo: &Path, opts: &Opts) -> Result<Vec<Column>, String> {
    // (column name, git ref, is the working tree?) — the name is what the table shows,
    // the ref what gets resolved and built, so `--pr` can display `pr-79` while building
    // the fetched `refs/bench/pr-79`
    let mut specs: Vec<(String, String, bool)> = Vec::new();
    for name in &opts.compare {
        let working_tree = name.eq_ignore_ascii_case("HEAD") || name == "." || name == "@";
        specs.push((name.clone(), name.clone(), working_tree));
    }
    if !opts.pr.is_empty() {
        fetch_prs(repo, &opts.pr)?;
        // with no --compare column to judge against, the base comes first — origin's
        // tip, the same tip GitHub merges the PRs into
        if specs.is_empty() {
            specs.push((opts.base.clone(), opts.base.clone(), false));
        }
        for n in &opts.pr {
            specs.push((format!("pr-{n}"), format!("refs/bench/pr-{n}"), false));
        }
    }
    // plain `cargo bench`: the working tree alone — but with `--bin` columns given, only
    // those are measured (no surprise build of the checkout next to them)
    if specs.is_empty() && opts.bin.is_empty() {
        specs.push(("HEAD".to_string(), "HEAD".to_string(), true));
    }
    let mut columns = Vec::new();
    // sha → already-built binary, so two refs at the same commit build once
    let mut built: Vec<(String, PathBuf)> = Vec::new();
    for (name, refname, working_tree) in specs {
        let sha = if working_tree {
            git(repo, &["rev-parse", "HEAD"])?
        } else {
            git(repo, &["rev-parse", &format!("{refname}^{{commit}}")])?
        };
        let exe = match built.iter().find(|(s, _)| s == &sha) {
            Some((_, exe)) if !working_tree => exe.clone(),
            _ => {
                let exe = build_ref(repo, &name, &sha, working_tree)?;
                if !working_tree {
                    built.push((sha.clone(), exe.clone()));
                }
                exe
            }
        };
        columns.push(Column {
            name,
            kind: ColKind::Apkscope { sha },
            exe,
        });
    }
    for bin in &opts.bin {
        if !bin.is_file() {
            return Err(format!("--bin {}: no such file", bin.display()));
        }
        let version = bin_version(bin)?;
        let stem = bin
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut name = if stem.is_empty() {
            "bin".to_string()
        } else {
            stem.clone()
        };
        // same stem twice (./old/apkscope, ./new/apkscope) still needs distinct columns
        // and distinct scratch dirs
        let mut n = 2;
        while columns.iter().any(|c| c.name == name) {
            name = format!("{stem}-{n}");
            n += 1;
        }
        columns.push(Column {
            name,
            kind: ColKind::Bin { version },
            exe: bin.clone(),
        });
    }
    for tool in &opts.tool {
        let exe = find_tool(tool)
            .ok_or_else(|| format!("tool {tool}: not found on PATH (or pass an absolute path)"))?;
        columns.push(Column {
            name: tool.clone(),
            kind: ColKind::Tool,
            exe,
        });
    }
    Ok(columns)
}

/// Build apkscope at one ref into `target/bench/bin/<ref>-<sha>/`. `HEAD` (and `.`)
/// compiles the working tree in place, uncommitted changes included, on every run —
/// cargo's own fingerprinting makes that a no-op when nothing changed; a sha-keyed
/// cache would go stale exactly when a PR is being written. Every other ref is checked
/// out into a worktree under `target/bench/wt/` and cached by sha (a worktree at a
/// given sha always holds the same source, so the cache cannot go stale). Each ref
/// gets its own `CARGO_TARGET_DIR` under `target/bench/build/` — one shared directory
/// would let one ref's binary overwrite another's between the build and the copy.
fn build_ref(repo: &Path, name: &str, sha: &str, working_tree: bool) -> Result<PathBuf, String> {
    let dir =
        repo.join("target/bench/bin")
            .join(format!("{}-{}", slug(name), &sha[..sha.len().min(7)]));
    let exe = dir.join(exe_name());
    if !working_tree && exe.is_file() {
        eprintln!("{name}: apkscope {} (cached)", &sha[..sha.len().min(12)]);
        return Ok(exe);
    }
    let build_dir = repo.join("target/bench/build").join(slug(name));
    let cwd = if working_tree {
        repo.to_path_buf()
    } else {
        let wt = repo.join("target/bench/wt").join(slug(name));
        let at = if wt.is_dir() {
            git(&wt, &["rev-parse", "HEAD"]).ok()
        } else {
            None
        };
        if at.as_deref() != Some(sha) {
            if wt.is_dir() {
                git(
                    repo,
                    &["worktree", "remove", "--force", &wt.display().to_string()],
                )?;
            }
            git(
                repo,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    &wt.display().to_string(),
                    sha,
                ],
            )?;
        }
        wt
    };
    eprintln!(
        "{name}: building apkscope at {}…",
        &sha[..sha.len().min(12)]
    );
    let t0 = Instant::now();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    // the CLI + MCP server the cases exercise; the viewer is not benched
    let out = Command::new(cargo)
        .current_dir(&cwd)
        .env("CARGO_TARGET_DIR", &build_dir)
        .args(["build", "--release", "--no-default-features"])
        .output()
        .map_err(|e| format!("cargo build for {name}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo build for {name} failed:\n{}",
            tail(&String::from_utf8_lossy(&out.stderr), 25)
        ));
    }
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    fs::copy(build_dir.join("release").join(exe_name()), &exe)
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    eprintln!("{name}: built in {:.1} s", t0.elapsed().as_secs_f64());
    Ok(exe)
}

/// Fetch the `--pr` pull requests from `origin` into `refs/bench/pr-<N>`. The remote's
/// branches refresh first: GitHub builds each merge ref against the base branch's
/// current tip, so that tip — not a possibly stale local `main` — is the fair baseline.
/// The merge ref is the PR exactly as CI sees it; a PR with conflicts (or a closed one)
/// has none, and falls back to the plain, unmerged head.
fn fetch_prs(repo: &Path, prs: &[u64]) -> Result<(), String> {
    git(repo, &["fetch", "--quiet", "origin"])
        .map_err(|e| format!("fetching origin (needed by --pr): {e}"))?;
    for n in prs {
        let dst = format!("refs/bench/pr-{n}");
        let merge = format!("+pull/{n}/merge:{dst}");
        if git(repo, &["fetch", "--quiet", "origin", &merge]).is_ok() {
            continue;
        }
        let head = format!("+pull/{n}/head:{dst}");
        git(repo, &["fetch", "--quiet", "origin", &head])
            .map_err(|e| format!("fetching pr-{n} from origin: {e} — is PR #{n} open there?"))?;
        eprintln!(
            "pr-{n}: origin holds no merge ref (conflicts? closed?); benching the unmerged head"
        );
    }
    Ok(())
}

/// `--version` of a `--bin` column: proves the file is an apkscope binary before
/// minutes are spent on it, and lands in the table's footer as its build id.
fn bin_version(exe: &Path) -> Result<String, String> {
    let out = Command::new(exe)
        .arg("--version")
        .output()
        .map_err(|e| format!("--bin {}: {e}", exe.display()))?;
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if !out.status.success() || !line.starts_with("apkscope") {
        return Err(format!(
            "--bin {}: not an apkscope binary (`--version` said: {})",
            exe.display(),
            if line.is_empty() { "nothing" } else { &line }
        ));
    }
    Ok(line.strip_prefix("apkscope ").unwrap_or(&line).to_string())
}

/// Resolve a tool name against PATH the way the shell would (absolute paths pass through).
fn find_tool(name: &str) -> Option<PathBuf> {
    if Path::new(name).is_file() {
        return Some(PathBuf::from(name));
    }
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `a/b-C@1` → `a-b-c-1`: safe as a directory name under target/bench/
fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in s.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "ref".to_string()
    } else {
        trimmed.to_string()
    }
}

// ───────────────────────────── run one subprocess ─────────────────────────────

#[cfg(unix)]
mod child_usage {
    //! `wait4(2)` returns the exit status *and* `ru_maxrss` — the child's peak RSS — in
    //! one call: no polling, no race with a short-lived child, and no new dependency,
    //! because the C library is already linked by std. 64-bit layout: two `struct
    //! timeval` (16 B each), then `ru_maxrss` at offset 32 — the same field order on Linux
    //! and macOS; the unit differs (KiB on Linux, bytes on macOS).

    use std::{io, process::Child};

    #[repr(C)]
    struct RUsage([u8; 144]);

    unsafe extern "C" {
        fn wait4(pid: i32, status: *mut i32, options: i32, usage: *mut RUsage) -> i32;
    }

    /// Wait for the child; returns `(exit, peak RSS in bytes)`. A death by signal is a
    /// negative exit. If `wait4` fails, fall back to `Child::wait` with the RSS unknown.
    pub fn wait(child: &mut Child) -> io::Result<(i32, Option<u64>)> {
        let mut status: i32 = 0;
        let mut usage = RUsage([0; 144]);
        let pid = child.id() as i32;
        // SAFETY: `status`/`usage` are valid out-parameters, `pid` is our own child, and
        // nothing reaps it first — std's `Child::wait` is never called on the success path.
        let r = unsafe { wait4(pid, &mut status, 0, &mut usage) };
        if r < 0 {
            let e = io::Error::last_os_error();
            let st = child.wait().map_err(|_| e)?;
            return Ok((st.code().unwrap_or(-1), None));
        }
        // WIFEXITED/WEXITSTATUS/WTERMSIG by hand — the libc macros, without the crate
        let code = (status >> 8) & 0xff;
        let sig = status & 0x7f;
        let exit = if sig == 0 { code } else { -sig };
        let peak = if cfg!(target_pointer_width = "64") {
            // ru_maxrss sits right after the two timevals on every 64-bit unix
            let raw = i64::from_ne_bytes(usage.0[32..40].try_into().expect("8 bytes"));
            let scale = if cfg!(target_os = "macos") { 1 } else { 1024 };
            u64::try_from(raw).ok().map(|v| v * scale)
        } else {
            None
        };
        Ok((exit, peak))
    }
}

#[cfg(windows)]
mod child_usage {
    //! Peak RSS of a child via `OpenProcess` + `GetProcessMemoryInfo`; kernel32 is
    //! already linked by std, so this adds no dependency.

    use std::{ffi::c_void, process::Child};

    #[repr(C)]
    struct MemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_non_paged: usize,
        quota_non_paged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut MemoryCounters,
            cb: u32,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// A process handle kept open so the peak RSS stays queryable after `Child::wait`.
    pub struct Handle(*mut c_void);

    pub fn open(pid: u32) -> Option<Handle> {
        // SAFETY: plain Win32 calls with an integer pid.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() { None } else { Some(Handle(h)) }
    }

    impl Handle {
        pub fn peak_rss(&self) -> Option<u64> {
            // SAFETY: `m` is a valid out-parameter of the size its `cb` declares.
            let mut m: MemoryCounters = unsafe { std::mem::zeroed() };
            m.cb = std::mem::size_of::<MemoryCounters>() as u32;
            let ok = unsafe { GetProcessMemoryInfo(self.0, &mut m, m.cb) };
            (ok != 0).then(|| m.peak_working_set as u64)
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from OpenProcess and is closed exactly once.
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// Wait for the child; returns `(exit, peak RSS in bytes)` from the kept handle.
    pub fn wait(child: &mut Child, handle: &Option<Handle>) -> std::io::Result<(i32, Option<u64>)> {
        let st = child.wait()?;
        let peak = handle.as_ref().and_then(|h| h.peak_rss());
        Ok((st.code().unwrap_or(-1), peak))
    }
}

struct RunOut {
    seconds: f64,
    peak_rss: Option<u64>,
    exit: i32,
    stdout: Vec<u8>,
    stderr: String,
}

/// Spawn `exe args` (in `cwd`, when given), time it, and take its peak RSS and stdout.
/// stdout is read to EOF before waiting; stderr is drained on a thread so a chatty child
/// cannot block on a full pipe.
fn run_once(exe: &Path, args: &[String], cwd: Option<&Path>) -> Result<RunOut, String> {
    let mut cmd = Command::new(exe);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let start = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))?;

    #[cfg(windows)]
    let handle = child_usage::open(child.id());

    let err_reader = {
        let pipe = child.stderr.take();
        thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut v);
            }
            v
        })
    };
    let mut stdout = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_end(&mut stdout);
    }
    let stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).to_string();

    #[cfg(unix)]
    let (exit, peak_rss) =
        child_usage::wait(&mut child).map_err(|e| format!("{}: {e}", exe.display()))?;
    #[cfg(windows)]
    let (exit, peak_rss) =
        child_usage::wait(&mut child, &handle).map_err(|e| format!("{}: {e}", exe.display()))?;

    Ok(RunOut {
        seconds: start.elapsed().as_secs_f64(),
        peak_rss,
        exit,
        stdout,
        stderr,
    })
}

// ───────────────────────────── measure ─────────────────────────────

struct Cell {
    /// median wall time over the successful runs
    median_s: Option<f64>,
    /// median peak RSS over the successful runs, in bytes
    peak_rss: Option<u64>,
    /// SHA-256 of the first successful run's output (with `--hash`)
    hash: Option<String>,
    /// set when every run failed: what to print in the cell
    err: Option<String>,
    /// the case does not apply to this column (`--tool` on an apkscope-only case)
    absent: bool,
}

struct CaseBench {
    def: &'static CaseDef,
    cells: Vec<Cell>,
}

struct ApkBench {
    apk: PathBuf,
    /// the class the `decompile`/`smali` cases use, picked from `classes` output
    class: Option<String>,
    cases: Vec<CaseBench>,
}

fn measure(
    opts: &Opts,
    repo: &Path,
    columns: &[Column],
    apks: &[PathBuf],
) -> Result<Vec<ApkBench>, String> {
    let cases: Vec<&'static CaseDef> = if opts.only_cases.is_empty() {
        CASES.iter().collect()
    } else {
        let mut v = Vec::new();
        for name in &opts.only_cases {
            v.push(case_def(name).ok_or_else(|| {
                format!(
                    "unknown case {name}; valid: {}",
                    CASES.iter().map(|c| c.id).collect::<Vec<_>>().join(", ")
                )
            })?);
        }
        v
    };
    if opts.runs == 0 {
        return Err("--runs must be at least 1".into());
    }
    let first_ref = columns.iter().position(|c| c.is_apkscope());
    let mut results = Vec::new();
    for apk in apks {
        // the class every column decompiles: picked once, from the first ref, so the
        // outputs (and their --hash digests) are comparable across columns
        let class = if cases.iter().any(|c| c.needs_class) {
            match first_ref.map(|i| pick_class(&columns[i].exe, apk)) {
                Some(Some(c)) => Some(c),
                Some(None) => {
                    eprintln!(
                        "{}: no class found; the decompile/smali cases will read n/a",
                        apk.display()
                    );
                    None
                }
                None => None,
            }
        } else {
            None
        };
        eprintln!(
            "{}: {} cases × {} columns × {} runs",
            apk.display(),
            cases.len(),
            columns.len(),
            opts.runs
        );
        let mut case_benches = Vec::new();
        for def in &cases {
            let mut cells = Vec::new();
            for col in columns {
                cells.push(measure_cell(opts, repo, def, col, apk, class.as_deref()));
            }
            case_benches.push(CaseBench { def, cells });
        }
        results.push(ApkBench {
            apk: apk.clone(),
            class,
            cases: case_benches,
        });
    }
    Ok(results)
}

fn measure_cell(
    opts: &Opts,
    repo: &Path,
    def: &CaseDef,
    col: &Column,
    apk: &Path,
    class: Option<&str>,
) -> Cell {
    if !col.is_apkscope() && def.apkscope_only {
        return Cell {
            median_s: None,
            peak_rss: None,
            hash: None,
            err: None,
            absent: true,
        };
    }
    if def.needs_class && class.is_none() {
        return Cell {
            median_s: None,
            peak_rss: None,
            hash: None,
            err: None,
            absent: false,
        };
    }
    // every column gets its own scratch dir: tools may drop files next to their output
    // (`jadx app.apk` writes an `app/` folder), and `build` writes out.apk per column
    let apk_slug = slug(
        &apk.file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
    );
    let out_dir = repo
        .join("target/bench/out")
        .join(apk_slug)
        .join(slug(&col.name));
    let out_apk = out_dir.join("out.apk");
    let _ = fs::create_dir_all(&out_dir);
    let (args, cwd) = if col.is_apkscope() {
        (apkscope_args(def.id, apk, class, Some(&out_apk)), None)
    } else {
        (vec![apk.display().to_string()], Some(out_dir.as_path()))
    };
    let mut times: Vec<f64> = Vec::new();
    let mut rss: Vec<u64> = Vec::new();
    let mut hash = None;
    let mut fail = None;
    let mut failed_runs = 0;
    for _ in 0..opts.runs {
        match run_once(&col.exe, &args, cwd) {
            Ok(ro) if ro.exit == 0 => {
                times.push(ro.seconds);
                if let Some(b) = ro.peak_rss {
                    rss.push(b);
                }
                if opts.hash && hash.is_none() {
                    // a failed digest disables the comparison for this cell (hash stays
                    // None) instead of failing the run that produced the output
                    let digest = if def.id == "build" {
                        apk_digest(&out_apk)
                    } else {
                        Ok(digest_hex(&ro.stdout))
                    };
                    hash = match digest {
                        Ok(h) => Some(h),
                        Err(e) => {
                            eprintln!("warning: {} on {}: {e}", def.label, col.name);
                            None
                        }
                    };
                }
            }
            Ok(ro) => {
                failed_runs += 1;
                fail = Some(fail_msg(ro.exit, &ro.stderr));
            }
            Err(e) => {
                failed_runs += 1;
                fail = Some(e);
            }
        }
    }
    if failed_runs > 0 {
        // a partially failed cell still reports the median of its successful runs, but
        // never silently
        eprintln!(
            "warning: {failed_runs}/{} runs failed for {} — {} on {}: {}",
            opts.runs,
            col.name,
            def.label,
            apk.display(),
            fail.as_deref().unwrap_or("?")
        );
    }
    if times.is_empty() {
        return Cell {
            median_s: None,
            peak_rss: None,
            hash,
            err: fail,
            absent: false,
        };
    }
    Cell {
        median_s: median_f64(&mut times),
        peak_rss: median_u64(&mut rss),
        hash,
        err: None,
        absent: false,
    }
}

fn pick_class(exe: &Path, apk: &Path) -> Option<String> {
    let out = run_once(exe, &apkscope_args("classes", apk, None, None), None).ok()?;
    if out.exit != 0 {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    (!line.is_empty()).then_some(line)
}

fn fail_msg(exit: i32, stderr: &str) -> String {
    let first = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim();
    let mut m = format!("exit {exit}");
    if !first.is_empty() {
        m.push_str(": ");
        m.push_str(&first.chars().take(60).collect::<String>());
    }
    m
}

fn median_f64(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

fn median_u64(v: &mut [u64]) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

/// The `build` case's digest: SHA-256 over the rebuilt APK's entry list and CRCs — the
/// parts that say "same content" — so re-signing or fresh zip timestamps cannot make two
/// identical rebuilds look different.
fn apk_digest(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut entries: Vec<(Vec<u8>, u32)> = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let e = zip
            .by_index(i)
            .map_err(|e| format!("{}: entry {i}: {e}", path.display()))?;
        // raw name bytes: a stable identity even for the rare non-UTF-8 entry name
        entries.push((e.name_raw().to_vec(), e.crc32()));
    }
    entries.sort();
    let mut h = Sha256::new();
    for (name, crc) in entries {
        h.update(&name);
        h.update(crc.to_le_bytes());
    }
    Ok(hex(&h.finalize()))
}

fn digest_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

// ───────────────────────────── emit: markdown ─────────────────────────────

fn emit_markdown(results: &[ApkBench], columns: &[Column], opts: &Opts, diffs: &[HashDiff]) {
    for r in results {
        println!();
        println!(
            "### {}",
            r.apk
                .file_name()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default()
        );
        println!();
        table(&r.cases, columns, cell_text);
        if opts.hash {
            println!();
            println!("#### Output hashes (`--hash`)");
            println!();
            table(&r.cases, columns, hash_cell_text);
        }
    }
    println!();
    println!(
        "Columns: {}",
        columns
            .iter()
            .map(column_desc)
            .collect::<Vec<_>>()
            .join(" · ")
    );
    println!(
        "Median of {} runs; apkscope runs are cold (`-q --no-cache`); cells are wall time, peak \
         RSS. The whole-APK row: `apkscope decompile <apk>` for apkscope, `<tool> <apk>` for tools.",
        opts.runs
    );
    if opts.hash {
        println!();
        let n_refs = columns.iter().filter(|c| c.is_apkscope()).count();
        if n_refs == 0 {
            println!(
                "`--hash`: no apkscope columns given; digests are shown but there is nothing to compare."
            );
        } else if n_refs == 1 {
            println!(
                "`--hash`: only one apkscope column ({}); add a second --compare, --pr or --bin to detect output changes between builds.",
                columns
                    .iter()
                    .find(|c| c.is_apkscope())
                    .map(|c| c.name.as_str())
                    .unwrap_or_default()
            );
        } else if diffs.is_empty() {
            println!("`--hash`: outputs identical across all compared refs.");
        } else {
            println!(
                "**`--hash`: {} case(s) produce different output across the compared refs**",
                diffs.len()
            );
            for d in diffs {
                println!();
                println!(
                    "- {} — {}: {}",
                    d.label,
                    d.apk,
                    d.cols
                        .iter()
                        .map(|(c, h)| format!("{c} = {}", &h[..h.len().min(12)]))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
}

/// The PR-table shape: first cell empty, columns as columns, cases as rows.
fn table(cases: &[CaseBench], columns: &[Column], cell: fn(&Cell) -> String) {
    let heads: Vec<String> = columns.iter().map(|c| format!(" {} ", c.name)).collect();
    println!("|  |{}|", heads.join("|"));
    let dashes: Vec<&str> = std::iter::repeat_n(" --- ", columns.len() + 1).collect();
    println!("|{}|", dashes.join("|"));
    for case in cases {
        if case.cells.iter().all(|c| c.absent) {
            continue;
        }
        let cells: Vec<String> = case
            .cells
            .iter()
            .map(|c| format!(" {} ", cell(c)))
            .collect();
        println!("| {} |{}|", case.def.label, cells.join("|"));
    }
}

fn cell_text(c: &Cell) -> String {
    if c.absent {
        return "—".to_string();
    }
    if let Some(e) = &c.err {
        return one_line(&format!("FAILED ({e})"));
    }
    match (c.median_s, c.peak_rss) {
        (Some(t), Some(r)) => format!("{}, {}", fmt_time(t), fmt_rss(r)),
        (Some(t), None) => fmt_time(t),
        (None, _) => "n/a".to_string(),
    }
}

fn hash_cell_text(c: &Cell) -> String {
    match (&c.hash, c.absent) {
        (Some(h), _) => format!("`{}`", &h[..h.len().min(12)]),
        (None, true) => "—".to_string(),
        (None, _) => "n/a".to_string(),
    }
}

fn column_desc(c: &Column) -> String {
    match &c.kind {
        ColKind::Apkscope { sha } => format!(
            "{} = apkscope {} ({})",
            c.name,
            &sha[..sha.len().min(7)],
            c.exe.display()
        ),
        ColKind::Bin { version } => format!(
            "{} = {} (apkscope {}, given with --bin)",
            c.name,
            c.exe.display(),
            version
        ),
        ColKind::Tool => format!("{} = {} (external tool)", c.name, c.exe.display()),
    }
}

fn fmt_time(s: f64) -> String {
    // the unit follows the value: milliseconds under a second — where cold-query
    // differences live — seconds above it, minutes past a minute
    if s >= 60.0 {
        format!(
            "{} min {} s",
            s as u64 / 60,
            s.rem_euclid(60.0).round() as u64
        )
    } else if s >= 10.0 {
        format!("{s:.1} s")
    } else if s >= 1.0 {
        format!("{s:.2} s")
    } else if s >= 0.1 {
        format!("{:.0} ms", s * 1000.0)
    } else {
        format!("{:.1} ms", s * 1000.0)
    }
}

fn fmt_rss(b: u64) -> String {
    format!("{} MB", (b as f64 / 1_048_576.0).round() as u64)
}

/// keep the table intact: no pipes, no line breaks
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '|' || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

// ───────────────────────────── emit: json ─────────────────────────────

fn emit_json(results: &[ApkBench], columns: &[Column], opts: &Opts) -> Result<(), String> {
    let v = serde_json::json!({
        "runs": opts.runs,
        "columns": columns
            .iter()
            .map(|c| serde_json::json!({
                "name": c.name,
                "kind": if c.is_apkscope() { "apkscope" } else { "tool" },
                "exe": c.exe.display().to_string(),
                "sha": match &c.kind {
                    ColKind::Apkscope { sha } => serde_json::json!(sha),
                    ColKind::Bin { .. } | ColKind::Tool => serde_json::json!(null),
                },
                "version": match &c.kind {
                    // the `--version` line of a --bin column; null for built refs and tools
                    ColKind::Bin { version } => serde_json::json!(version),
                    ColKind::Apkscope { .. } | ColKind::Tool => serde_json::json!(null),
                },
            }))
            .collect::<Vec<_>>(),
        "apks": results
            .iter()
            .map(|r| serde_json::json!({
                "apk": r.apk.display().to_string(),
                "class": r.class,
                "cases": r.cases
                    .iter()
                    .map(|case| serde_json::json!({
                        "case": case.def.id,
                        "label": case.def.label,
                        "cells": case.cells
                            .iter()
                            .map(|c| serde_json::json!({
                                "median_s": c.median_s,
                                "peak_rss_bytes": c.peak_rss,
                                "hash": c.hash,
                                "error": c.err,
                                "absent": c.absent,
                            }))
                            .collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
    );
    Ok(())
}

// ───────────────────────────── emit: svg ─────────────────────────────

const COLORS: &[&str] = &[
    "#2563eb", "#dc2626", "#16a34a", "#d97706", "#7c3aed", "#0e7490", "#be185d", "#4d7c0f",
];

/// One bar chart per APK and metric, written to `docs/bench-<apk>-{time,ram}.svg`.
fn emit_svg(results: &[ApkBench], columns: &[Column]) -> Result<(), String> {
    for r in results {
        let stem = r
            .apk
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()
            .into_owned();
        let base = slug(&stem);
        for (metric, file, pick) in [
            ("wall time", "time", pick_time as fn(&Cell) -> Option<f64>),
            ("peak RSS", "ram", pick_rss as fn(&Cell) -> Option<f64>),
        ] {
            let path = Path::new("docs").join(format!("bench-{base}-{file}.svg"));
            let rows: Vec<(&str, Vec<(usize, f64)>)> = r
                .cases
                .iter()
                .filter(|case| case.cells.iter().any(|c| !c.absent && pick(c).is_some()))
                .map(|case| {
                    (
                        case.def.label,
                        case.cells
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| !c.absent)
                            .filter_map(|(i, c)| pick(c).map(|v| (i, v)))
                            .collect(),
                    )
                })
                .collect();
            if rows.iter().all(|(_, v)| v.is_empty()) {
                continue;
            }
            let title = format!("apkscope bench — {stem}: {metric} (median)");
            let svg = chart(&rows, columns, &title, metric);
            fs::create_dir_all("docs").map_err(|e| format!("docs/: {e}"))?;
            fs::write(&path, svg).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}

fn pick_time(c: &Cell) -> Option<f64> {
    c.median_s
}

fn pick_rss(c: &Cell) -> Option<f64> {
    c.peak_rss.map(|b| b as f64 / 1_048_576.0)
}

/// Grouped horizontal bars: one row per case, one bar per column, all bars scaled by the
/// chart's maximum. Hand-written SVG — no rendering dependency.
fn chart(
    rows: &[(&str, Vec<(usize, f64)>)],
    columns: &[Column],
    title: &str,
    metric: &str,
) -> String {
    const W: f64 = 940.0;
    const MARGIN_L: f64 = 250.0;
    const MARGIN_R: f64 = 130.0;
    const MARGIN_T: f64 = 70.0;
    const BAR_H: f64 = 12.0;
    const BAR_DY: f64 = 17.0;
    const ROW_GAP: f64 = 12.0;
    let vmax = rows
        .iter()
        .flat_map(|(_, v)| v.iter().map(|(_, x)| *x))
        .fold(0.0_f64, f64::max);
    let plot_w = W - MARGIN_L - MARGIN_R;
    let row_h = |bars: usize| bars as f64 * BAR_DY + ROW_GAP;
    let height = MARGIN_T + rows.iter().map(|(_, v)| row_h(v.len())).sum::<f64>() + 14.0;
    let mut s = String::with_capacity(4096);
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{W:.0}\" height=\"{height:.0}\" \
         font-family=\"ui-sans-serif, system-ui, sans-serif\">\n"
    ));
    s.push_str("<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n");
    s.push_str(&format!(
        "<text x=\"16\" y=\"24\" font-size=\"15\" font-weight=\"bold\" fill=\"#111827\">{}</text>\n",
        esc(title)
    ));
    for (i, col) in columns.iter().enumerate() {
        let x = 16.0 + i as f64 * 190.0;
        s.push_str(&format!(
            "<rect x=\"{x:.0}\" y=\"34\" width=\"10\" height=\"10\" fill=\"{}\"/>\n",
            color(i)
        ));
        s.push_str(&format!(
            "<text x=\"{:.0}\" y=\"43\" font-size=\"12\" fill=\"#374151\">{}</text>\n",
            x + 14.0,
            esc(&col.name)
        ));
    }
    let mut y = MARGIN_T;
    for (label, bars) in rows {
        let label: String = label.trim_matches('`').to_string();
        let block = row_h(bars.len());
        s.push_str(&format!(
            "<text x=\"{}\" y=\"{:.0}\" font-size=\"12\" fill=\"#111827\" \
             text-anchor=\"end\">{}</text>\n",
            MARGIN_L - 8.0,
            y + BAR_DY * 0.75,
            esc(&label)
        ));
        for (j, (col, v)) in bars.iter().enumerate() {
            let by = y + j as f64 * BAR_DY;
            let w = (v / vmax * plot_w).max(1.0);
            s.push_str(&format!(
                "<rect x=\"{:.0}\" y=\"{by:.0}\" width=\"{w:.0}\" height=\"{BAR_H:.0}\" \
                 fill=\"{}\"/>\n",
                MARGIN_L,
                color(*col)
            ));
            s.push_str(&format!(
                "<text x=\"{:.0}\" y=\"{:.0}\" font-size=\"11\" fill=\"#374151\">{}</text>\n",
                MARGIN_L + w + 6.0,
                by + BAR_H - 1.0,
                esc(&fmt_value(*v, metric))
            ));
        }
        y += block;
        s.push_str(&format!(
            "<line x1=\"16\" y1=\"{y:.0}\" x2=\"{}\" y2=\"{y:.0}\" stroke=\"#e5e7eb\"/>\n",
            W - 16.0
        ));
    }
    s.push_str("</svg>\n");
    s
}

fn color(i: usize) -> &'static str {
    COLORS[i % COLORS.len()]
}

fn fmt_value(v: f64, metric: &str) -> String {
    if metric == "peak RSS" {
        format!("{v:.0} MB")
    } else {
        fmt_time(v)
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ───────────────────────────── the --hash comparison ─────────────────────────────

/// One case whose output differs across the compared refs. `cols` holds every ref
/// column's digest, so the report shows which side changed.
struct HashDiff {
    apk: String,
    label: &'static str,
    cols: Vec<(String, String)>,
}

fn hash_diffs(results: &[ApkBench], columns: &[Column]) -> Vec<HashDiff> {
    let ref_idx: Vec<usize> = columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_apkscope())
        .map(|(i, _)| i)
        .collect();
    if ref_idx.len() < 2 {
        return Vec::new();
    }
    let mut diffs = Vec::new();
    for r in results {
        for case in &r.cases {
            let cols: Vec<(String, String)> = ref_idx
                .iter()
                .filter_map(|&i| {
                    case.cells[i]
                        .hash
                        .clone()
                        .map(|h| (columns[i].name.clone(), h))
                })
                .collect();
            let distinct: std::collections::HashSet<&str> =
                cols.iter().map(|(_, h)| h.as_str()).collect();
            if cols.len() == ref_idx.len() && distinct.len() > 1 {
                diffs.push(HashDiff {
                    apk: r
                        .apk
                        .file_name()
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default()
                        .into_owned(),
                    label: case.def.label,
                    cols,
                });
            }
        }
    }
    diffs
}

// ───────────────────────────── main ─────────────────────────────

fn main() -> std::process::ExitCode {
    let opts = Opts::parse();
    match run(&opts) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::from(2)
        }
    }
}

fn run(opts: &Opts) -> Result<std::process::ExitCode, String> {
    let repo = repo_root()?;
    let apks = resolve_apks(&repo, opts)?;
    let columns = build_columns(&repo, opts)?;
    let results = measure(opts, &repo, &columns, &apks)?;
    let failed = results
        .iter()
        .flat_map(|r| r.cases.iter())
        .flat_map(|c| c.cells.iter())
        .any(|c| c.err.is_some());
    let diffs = if opts.hash {
        hash_diffs(&results, &columns)
    } else {
        Vec::new()
    };
    match opts.format {
        Format::Markdown => emit_markdown(&results, &columns, opts, &diffs),
        Format::Json => emit_json(&results, &columns, opts)?,
        Format::Svg => emit_svg(&results, &columns)?,
    }
    if !diffs.is_empty() {
        for d in &diffs {
            eprintln!(
                "output differs: {} on {}: {}",
                d.label,
                d.apk,
                d.cols
                    .iter()
                    .map(|(c, h)| format!("{c} {}", &h[..h.len().min(12)]))
                    .collect::<Vec<_>>()
                    .join(" vs ")
            );
        }
        eprintln!("--hash: {} case(s) differ; exit 3", diffs.len());
        return Ok(std::process::ExitCode::from(3));
    }
    if failed {
        eprintln!("a run failed; exit 1");
        return Ok(std::process::ExitCode::from(1));
    }
    Ok(std::process::ExitCode::SUCCESS)
}

fn repo_root() -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err("not inside a git repository — run from the apkscope checkout".into());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

fn resolve_apks(repo: &Path, opts: &Opts) -> Result<Vec<PathBuf>, String> {
    if !opts.apk.is_empty() {
        for p in &opts.apk {
            if !p.is_file() {
                return Err(format!("{}: no such file", p.display()));
            }
        }
        return Ok(opts.apk.clone());
    }
    let dir = repo.join("bench/apps");
    let mut apks: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .as_deref(),
                Some("apk" | "xapk" | "apks" | "apkm" | "jar" | "dex")
            )
        })
        .collect();
    apks.sort();
    if apks.is_empty() {
        return Err(format!(
            "no --apk given and {} holds no APKs; APKs are not committed, so pass --apk \
             <path> (repeatable)",
            dir.display()
        ));
    }
    Ok(apks)
}

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}
