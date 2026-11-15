# tests/

Unit-test modules for the `apkscope` and `apkscope-core` crates, kept out of the
implementation files so those files read as implementation only. Each file here is the body
of a `#[cfg(test)] mod <name> { … }` block that used to live inline at the bottom of a source
file.

## Layout

The tree mirrors the crates, using `_` to flatten module paths:

| source | tests live in |
|---|---|
| `src/<a>.rs` | `tests/apkscope/<a>.rs` |
| `src/<a>/<b>.rs` | `tests/apkscope/<a>_<b>.rs` |
| `core/src/<a>.rs` | `tests/apkscope-core/<a>.rs` |
| `core/src/<a>/mod.rs` | `tests/apkscope-core/<a>.rs` |

Files with more than one test module get one extracted file per module (e.g. `values.rs` had
`mod tests` and `mod edits`, now `tests/apkscope-core/values.rs` and `values_edits.rs`).

## How a module's tests are wired in

The implementation file keeps a stub that points here:

```rust
// core/src/axml.rs
#[cfg(test)]
#[path = "../../tests/apkscope-core/axml.rs"]
mod tests;
```

The file here is the *body* of that module — it starts with `use super::*;` (and any other
imports), then the `#[test]` functions, exactly as it did when it was inline. Nothing about
visibility, `pub(crate)` items, or `super::` paths changes: the tests compile against the same
private API they always did. Fixture modules shared across tests work the same way —
`src/dex.rs`'s `pub(crate) mod tests` (home of `tiny_dex()`) is one stub, and
`crate::dex::tests::tiny_dex()` still resolves.

## Why subdirectories, not Cargo integration tests

Two reasons:

- Cargo integration tests (the `tests/*.rs` files Cargo compiles automatically) link against
  the crate's *public* API only, and as a *separate binary*. Many of these tests exercise
  private internals (`pub(crate)` items, `super::*` imports), so an integration-test move would
  force visibility changes across the workspace. The `#[path]` attribute keeps each test
  module a private submodule of the module it tests — a pure file move, no behavior or API
  change.
- Cargo auto-discovers `tests/*.rs` and `tests/<dir>/main.rs` as integration tests, but files
  in subdirectories are ignored. Keeping the crates in `tests/apkscope/` and
  `tests/apkscope-core/` means Cargo never tries to compile these files standalone — no
  `autotests = false` or manifest change is needed, and the two crates' `dex.rs` files don't
  collide.

If a real integration test (public-API level) is added later, it can live at `tests/<name>.rs`
top level or be declared with an explicit `[[test]]` table.

## The path depth

`#[path]` is relative to the file that contains it, so:

| source file | `#[path]` |
|---|---|
| `src/java.rs` | `../tests/apkscope/java.rs` |
| `src/gui/live_ui.rs` | `../../tests/apkscope/gui_live_ui.rs` |
| `core/src/axml.rs` | `../../tests/apkscope-core/axml.rs` |
| `core/src/smali/mod.rs` | `../../tests/apkscope-core/smali.rs` |
| `core/src/table/edit.rs` | `../../../tests/apkscope-core/table_edit.rs` |

## Running them

Nothing changes from the inline layout — `cargo test --release --no-default-features` runs all
of these, exactly as CI does. `cargo test --release --no-default-features axml` runs just one
module's tests. `cargo fmt --all` follows the `#[path]` includes, so the test files stay
formatted like the rest of the workspace.
