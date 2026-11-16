# Contributing to apkscope

Thanks for your interest in improving apkscope. This document covers the workflow for bug reports,
code, benchmarks and docs. The repo is small and everything is in Rust: the faster and more readable
it gets, the more useful it is to researchers and agents.

## Ways to help

- **Decompiler output** — a class that renders wrong (a `/* WARNING */` header, a stray `goto`,
  wrong types) is the most valuable bug report of all: include the class name and, if you can, what
  JADX shows for the same method.
- **Performance** — apkscope's selling points are speed and memory. If you touch anything on the
  hot path (DEX reading, the index, the printers), measure before and after (see
  [Benchmarks](#benchmarks) below).
- **Core editing** — `apkscope-core` (`core/`): the DEX reader and writer, the Smali round trip,
  resources, rebuild and signing. Its promise is *byte-for-byte*: writing back what was read must
  not change a byte. Tests assert that; keep them passing.
- **The MCP server** — new tools, better paging, anything that makes agent sessions cheaper.
- **The viewer** — the egui GUI in `src/gui.rs`.
- **Packaging and installers** — `.deb` / AppImage in `packaging/`, the installers in `scripts/`.
- **Docs and benchmarks** — clearer README, honest limits, new measured results.

## Setting up

You need a recent stable Rust (the workspace uses edition 2024). Nothing else: no JVM, no Android
SDK, no system GUI libraries — the viewer loads the desktop's X11/Wayland/GL at runtime, so the
whole workspace builds on a bare Linux/macOS/Windows box.

```bash
git clone https://github.com/<your-fork>/apkscope
cd apkscope
git remote add upstream https://github.com/suffixing/apkscope.git

cargo build --release                          # viewer + CLI + MCP server
cargo build --release --no-default-features    # CLI + MCP server only (fastest loop)
```

## Branching: never work on `main`

Always branch. In your fork, keep `main` as a clean mirror of `upstream/main` and do every change
on a topic branch:

```bash
git fetch upstream
git checkout -b feat/short-description upstream/main
```

There is a practical reason beyond tidiness: **pull requests are opened *from* a branch, and GitHub
shares one branch across every PR that uses it.** If you open PRs from your fork's `main` and then
start a second, unrelated PR, the first PR's branch silently gains the second change — reviews get
confused, and force-pushing to unshare the history rewrites a PR that may already be under review.
One branch per change, from the start, avoids all of it. It also lets you keep `main` synced with
`upstream` via the GitHub UI without disturbing open PRs.

## Building and testing

CI (`.github/workflows/ci.yml`) is the gate; run the same things locally before pushing:

```bash
cargo fmt --all -- --check                     # formatting (rustfmt defaults)
cargo check --release --no-default-features    # CLI + MCP server
cargo check --release                          # ...and the viewer tree
cargo clippy --release --no-default-features -- -D warnings
cargo test --release --no-default-features     # what CI runs on all three OSes
```

Notes:

- Tests are inline `#[cfg(test)] mod tests` blocks at the end of each file, in both crates — there
  is no separate `tests/` directory. Put new tests next to the code they cover.
- `cargo test` builds both crates (the workspace's `default-members`), so core changes are tested
  from either directory.
- To run one module's tests: `cargo test --release --no-default-features dex::`.
- GUI code is compiled but not unit-tested headlessly; keep `cargo check --release` (default
  features) clean for it.

## Code style

- `rustfmt` defaults, no custom config. Comments in code are lowercase and explain *why*.
- Every module starts with a `//!` doc comment saying what it is (see any file in `core/src/`).
- In `apkscope-core`, every fallible function returns `apkscope_core::Error` — the `thiserror` enum
  in `core/src/error.rs` — never a `String`, and never a panic on bad input: malformed APKs are the
  normal case, not the exception.
- No new runtime dependencies without a strong case: "one small binary, no JVM" is a feature the
  README promises.

## Commit messages

Conventional Commits, with the crate or area as the scope, matching the history:

```
feat(core): look up, set, add and remove resource table values
fix(smali): keep hiddenapi flags through the Smali round trip
docs: add CONTRIBUTING.md and AGENTS.md
```

Types in use: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`.

## Pull requests

- One logical change per PR. Small PRs get reviewed; broad rewrites rot.
- Open the PR **from your topic branch** against `main` — never from your fork's `main`.
- If an issue exists for the change, reference it with `Closes #N` so it closes on merge.
- Describe user-visible changes in the PR body (they feed the release notes).
- The CI matrix (Linux, macOS, Windows, plus a musl static build) must pass. If your change only
  makes sense on one OS, say so in the PR.
- For anything on a hot path, include before/after numbers from the relevant `bench/` script.

## Benchmarks

Every number the README and `BENCHMARKS.md` quote comes from a script in `bench/`; keep it that way.

- `bench/regress.py` decompiles sample APKs and compares quality counters (failed methods, gotos,
  warnings) against `bench/baseline.json`. Run it when you touch the decompiler:
  `python bench/regress.py <apkscope.exe> <app.apk>...` — it fails when output quality regresses
  past the baseline, and `--update` re-records it (say so in the PR when you do).
- `bench/results.json` holds the measured numbers; `bench/charts.py` redraws `docs/bench-*.svg`
  from it. Only measured numbers go in `results.json` — never hand-edit the SVGs.
- The wall-time harnesses (`bench/asc.ps1`, `bench/open.ps1`, `bench/search.ps1`,
  `bench/measure.ps1`) are PowerShell and expect the tools' Windows paths; the agent-session
  harness `bench/mcp.py` runs anywhere.

## Security, ethics and licensing

- apkscope is a reverse-engineering tool: only analyze apps you own or are authorized to. Don't
  commit APKs you don't have the rights to, decompiled output of copyrighted apps, or anything
  containing real secrets — findings in issues and PRs should show trimmed, relevant lines only.
- For a vulnerability in apkscope itself, don't open a public issue with exploit details; use
  GitHub's private reporting on the Security tab if enabled, or contact the maintainer directly.
- Code is Apache-2.0 (see `LICENSE`). "apkscope" and its logo are trademarks of Ahmad (see
  `NOTICE`): modified versions shipped under a different name must use a different name and logo
  and may say they are "based on apkscope". `core/src/data/android_public.txt` is generated from
  AOSP (Apache-2.0).

## Releasing (maintainers)

Tags `v*` trigger `.github/workflows/release.yml`, which builds all five targets, packages the
`.deb`/AppImage, attaches checksums, and publishes (a `-` in the tag, e.g. `v0.10.0-rc.1`, marks a
pre-release). User-visible changes go into `docs/RELEASE_NOTES.md`.
