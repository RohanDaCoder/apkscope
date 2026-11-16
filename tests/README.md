# tests/

Unit-test modules for the `apkscope` crate, kept out of the implementation files so those
files read as implementation only — the same layout and wiring PR #28 introduces for the
whole tree (see issue #27). Each file here is the *body* of a `#[cfg(test)] mod tests { … }`
block that would otherwise live inline at the bottom of a source file.

## Layout

| source | tests live in |
|---|---|
| `src/<a>.rs` | `tests/apkscope/<a>.rs` |

## How a module's tests are wired in

The implementation file keeps a stub that points here:

```rust
// src/re.rs
#[cfg(test)]
#[path = "../tests/apkscope/re.rs"]
mod tests;
```

The file here is the body of that module — it starts with `use super::*;` (and any other
imports), then the `#[test]` functions. Nothing about visibility or `super::` paths changes:
the tests compile against the same private API they did when inline.

## Why subdirectories, not Cargo integration tests

Cargo auto-discovers `tests/*.rs` as integration tests, which link against the crate's
*public* API only and as a separate binary. These tests exercise private internals
(`use super::*`, `pub(crate)` items), so keeping them in `tests/apkscope/` — where Cargo
never compiles them standalone — makes the move a pure file move with no API change.
