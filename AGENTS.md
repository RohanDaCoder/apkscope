# AGENTS.md

Guide for AI coding agents working **on** this repository. To **use** apkscope from an agent,
see [docs/agents.md](docs/agents.md). Humans: see [CONTRIBUTING.md](CONTRIBUTING.md);
the rules are the same.

## What this is

apkscope is an Android reverse-engineering tool written from scratch in Rust: its own DEX parser,
decompiler, deobfuscator, resource decoder, APK signer, CLI, MCP server and egui viewer.
It is not a JADX or apktool wrapper; do not suggest adding them.

## Commands

```
cargo build --release                                      # with the viewer (default feature `gui`)
cargo fmt --all -- --check
cargo clippy --release --no-default-features -- -D warnings  # what CI lints
cargo test --release --no-default-features
```

## Layout

- `src/cli.rs`, `src/commands/`: CLI (clap), one file per command family
- `src/mcp.rs`: MCP server; each tool maps to CLI arguments
- `src/gui.rs`, `src/gui/`: the viewer (behind the `gui` feature)
- `src/session.rs`: opened APK, reference index, scoped searches (being split into `session/` + `search/`, see #50)
- `src/quick.rs`: stateless queries (no index, no cache)
- `src/java.rs`, `src/ir.rs`, `src/opt.rs`, `src/cfg.rs`: the decompiler
- `src/re.rs`: xrefs, call graphs, Frida scripts, scan
- `src/lookup.rs`: answers from the DEX tables without decompiling (`consts`, `returns`, `--in`/`--impl`)
- `src/retrofit_map.rs`: Retrofit's annotation names in obfuscated apps (for `search api`)
- `src/examples.rs`: the examples under every `--help`; a new command or flag gets one here
- `core/src/keystore/`: JKS / PKCS#12 reading for `build --ks` (its ciphers are written out, no dependency)
- `src/upgrade/`: self-update
- `core/`: the `apkscope-core` library (DEX, smali, resources, signing, rebuild)
- `tests/`: unit tests, wired in with `#[cfg(test)] #[path = "..."] mod tests;` (see `tests/README.md`)
- `bench/`: benchmark and regression scripts

## Rules

- **Wire every feature into all three surfaces:** CLI, MCP (`tools()` + `call()`), viewer.
- **Measure speed** on a large real APK against `main` and report it. No caching or extra RAM to
  win speed; make the algorithm better.
- **Check heuristics on a real app** and report the false positives, not only fixtures.
- **Offline only.** No network code outside `upgrade`. **No new dependencies** without asking.
- **README numbers must be measured.** Limits go in "Honest limits".
- **Small PRs:** one feature or one refactor, rebased on `main`, CI green.
- When moving code, update the `#[path]` test link and its file in `tests/` together.

## Gotchas

- `--version` is apkscope's own flag, not clap's: `upgrade` checks new binaries with `--version --json`.
- CI runs clippy without the `gui` feature; viewer code still has old lints there, so don't add new ones.
- Progress and summary lines on stderr go through `note!` (silenced by `-q`); only errors use `eprintln!`.
- Keep source files free of literal NUL or other control bytes (write `\0`); grep treats such files as binary.
