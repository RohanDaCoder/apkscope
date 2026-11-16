# APKScope Stable / Canary Update System

**Status:** Implemented on `feat/update-command` (PR #32)
**Repository:** `suffixing/apkscope`
**Last updated:** 2026-11-16

## 1. Purpose and principles

`apkscope upgrade` replaces the running executable with a newer build of the
same tool, from one of two channels:

- **Stable** — versioned releases for normal use.
- **Canary** — the newest successfully published build from `main`, for testing
  development changes.

The design is intentionally simple:

1. CI builds binaries (one shared matrix for both channels).
2. CI publishes those binaries as GitHub Release assets.
3. The CLI downloads a small manifest (`apkscope-update.json`) from the
   release through the ordinary public release-asset URL.
4. The CLI downloads the one asset matching its platform, verifies it, and
   safely replaces the installed executable.

**GitHub Releases are the distribution interface.** GitHub Actions artifacts
move build outputs between CI jobs, but the installed CLI never downloads
workflow artifacts and never calls the GitHub REST or GraphQL API. No token,
OAuth flow or login is involved; everything is a plain HTTPS release download,
the same URL a browser would use.

The updater fails closed: a missing manifest, incomplete publication, invalid
metadata, size or checksum mismatch, or an identity mismatch must never leave
the user with a partially replaced executable.

## 2. Release channels

### 2.1 Stable

| Property | Contract |
| --- | --- |
| Git tag | Immutable version tag, for example `v0.10.0` |
| GitHub Release | One release per version |
| Prerelease | No — except explicitly versioned candidates like `v0.11.0-rc.1` |
| Tag movement | Never force-move a published stable tag |
| Asset behavior | Publish once; an existing stable release is never overwritten |
| Discovery | `releases/latest/download` (GitHub's newest-release redirect) |

Latest stable manifest:

```text
https://github.com/suffixing/apkscope/releases/latest/download/apkscope-update.json
```

A specific stable version uses its versioned Release:

```text
https://github.com/suffixing/apkscope/releases/download/v0.10.0/apkscope-update.json
```

The client follows GitHub's ordinary HTTPS release-download redirect (to the
CDN). It does not call the GitHub REST API or require authentication.

### 2.2 Canary

| Property | Contract |
| --- | --- |
| Source | Successful builds of the `main` branch |
| Git tag | One persistent, movable tag named `canary` |
| GitHub Release | One persistent prerelease attached to `canary` |
| Prerelease | Yes — never GitHub's "latest" stable release |
| Tag movement | Force-updated to the commit of the newly published build |
| Asset behavior | Same filenames every publish; assets overwritten in place |
| Discovery | `releases/download/canary` (the fixed tag URL) |

Canary manifest:

```text
https://github.com/suffixing/apkscope/releases/download/canary/apkscope-update.json
```

The `canary` tag is a **moving pointer**, not a version number. It always
points to the commit of the latest successfully published canary build. There
is one Release and one tag, ever; publication updates both in place.

Example:

| Successful publication | `canary` points to | Assets in the canary Release |
| --- | --- | --- |
| First | Commit A | Binaries built from A |
| Second | Commit B | Binaries built from B (same filenames) |
| Third | Commit C | Binaries built from C (same filenames) |

The Release page and download URLs stay stable:

```text
https://github.com/suffixing/apkscope/releases/tag/canary
https://github.com/suffixing/apkscope/releases/download/canary/apkscope_linux_aarch64
```

The canary Release and its tag are **never deleted and recreated**. The
existing Release is updated and the existing tag ref is force-moved as part of
publishing (see §6.2).

## 3. CLI interface

The command is `upgrade` (there is deliberately no `update` command).

| Command | Behavior |
| --- | --- |
| `apkscope upgrade` | Upgrade within the channel embedded in the installed binary |
| `apkscope upgrade --canary` | Switch to, and upgrade from, canary |
| `apkscope upgrade --stable` | Switch to, and upgrade from, stable |
| `apkscope upgrade --check` | Check for an update without downloading or installing |
| `apkscope upgrade --version 0.10.0` | Install that exact stable release |

Rules:

- `--canary` and `--stable` are mutually exclusive.
- `--version` selects a stable release and cannot be combined with either
  channel flag.
- Plain `upgrade` preserves the installed binary's channel.
- A new installation defaults to stable.
- A binary without embedded channel metadata (a local or pre-upgrade build)
  is treated as stable — the documented fallback; the tool never claims an
  unknown channel was detected.
- There are no background or automatic updates; upgrading is user-initiated.

### Build identity

Every official binary embeds three facts at compile time
(`APKSCOPE_BUILD_CHANNEL`, `APKSCOPE_GIT_SHA`, set by the release workflow for
every matrix target):

- The Cargo package version.
- The channel it was released on: `stable` or `canary`.
- The full 40-character commit SHA it was built from.

A local `cargo build` gets neither variable, has no channel, and identifies as
a dev build — it can never pose as an official release.

`apkscope -V` prints the bare version (`0.10.0`). `apkscope --revision`
prints the version with its revision — `0.10.0-canary-4c3eeb` for a canary
(6-char commit), `0.10.0-dev` for a local build. The machine-readable form is:

```console
$ apkscope --version --json
{"version":"0.10.0","channel":"canary","commit":"0123..."}
```

The updater uses exactly this interface to verify a staged binary before
replacing anything, and again from the installed path afterwards.

A canary's identity is its commit SHA, **not** an invented SemVer suffix:
multiple canary builds share the same Cargo package version, and the updater
compares full commit SHAs to decide whether an update exists.

## 4. Platform mapping

The updater downloads the raw executable; no archive is extracted. Platform
IDs are the keys of the manifest's `assets` map:

| Platform ID | Release asset | Contents |
| --- | --- | --- |
| `linux-x86_64-gnu` | `apkscope_linux_x86_64` | Viewer + CLI + MCP |
| `linux-aarch64-gnu` | `apkscope_linux_aarch64` | Viewer + CLI + MCP |
| `linux-x86_64-musl` | `apkscope_linux_musl_x86_64` | CLI + MCP; no viewer |
| `linux-aarch64-musl` | `apkscope_linux_musl_aarch64` | CLI + MCP; no viewer |
| `windows-x86_64` | `apkscope_windows_x86_64.exe` | Viewer + CLI + MCP |
| `macos-aarch64` | `apkscope_macos_aarch64` | CLI + MCP |
| `macos-x86_64` | `apkscope_macos_x86_64` | CLI + MCP |

The musl builds know themselves at compile time (`target_env`), so the musl
binary selects its musl asset directly; a glibc build's gnu/musl choice uses
the same detection as the shell installer. The two are never substituted for
each other, and the manifest's asset name must match the platform's canonical
filename exactly — a manifest cannot redirect a platform to some other file.

The `.deb` and AppImage packages remain release assets for their existing
installation paths; the self-updater never selects them.

## 5. The update manifest

**`apkscope-update.json` is the single source of truth for update discovery
and asset verification.** There is no `version.txt`, no release-page parsing,
and `checksums.txt` (still published, for the installers and manual
verification) is not consulted by the self-updater.

Schema:

```json
{
  "schema_version": 1,
  "channel": "canary",
  "version": "0.10.0",
  "commit": "0123456789abcdef0123456789abcdef01234567",
  "assets": {
    "linux-aarch64-gnu": {
      "name": "apkscope_linux_aarch64",
      "size": 12345678,
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  }
}
```

The real manifest lists every platform in §4. The client validates, and fails
closed on:

- Unknown or missing `schema_version` (this build understands 1).
- A channel that does not match the requested one.
- A version that is not `X.Y.Z` (with optional `-suffix`).
- A commit that is not a full 40-character hex SHA.
- No entry for the current platform.
- An asset name that does not exactly match the platform mapping (including
  path traversal or directory components).
- A missing, zero or invalid size.
- A `sha256` that is not 64 hex characters.
- A `--version` request whose manifest carries a different version.
- A downloaded asset whose size or SHA-256 differs from the manifest.
- A staged binary whose `--version --json` identity (channel, version, full
  commit) differs from the manifest.

The updater constructs the binary URL itself from the repository, the selected
release tag and the validated asset name; the manifest supplies no URLs.

## 6. CI and publication lifecycle

Both channels are published by `.github/workflows/release.yml` — one workflow,
one shared build matrix, two publishers. `ci.yml` stays the pull-request and
branch CI and never publishes anything.

Triggers: pushes of `v*` tags publish stable; pushes to `main` publish canary.
Pull requests run neither.

### 6.1 Stable publication (version tag)

1. `lint` gates the matrix (fmt, both feature configs' check, clippy).
2. Every platform builds (linux-x86_64, linux-aarch64, macos, windows) and
   uploads its `dist/` as a workflow artifact — GitHub Actions artifacts move
   outputs between jobs; they are never a download source for users.
3. The `publish` job downloads all artifacts, generates `checksums.txt` and
   `apkscope-update.json` from the exact files in the room, and publishes the
   versioned Release with all executables, packages, checksums and manifest.
4. A "-" in the tag (`v0.11.0-rc.1`) publishes as a prerelease, never as the
   latest stable release.
5. Stable releases are never overwritten; the tag is never moved.

### 6.2 Canary publication (push to main)

The same lint + matrix + collect pipeline runs, then:

1. **Stale check, immediately before publishing:** the run's commit must still
   be `main`'s HEAD; otherwise the run skips. An older run queued behind a
   publish can never overwrite a newer canary.
2. **Take the old manifest offline:** `gh release delete-asset canary
   apkscope-update.json` (a no-op on the first publish). While the manifest is
   absent, clients fail closed — `upgrade --canary` reports that the canary
   release is being published and changes nothing.
3. **Move the tag:** `git tag --force canary $GITHUB_SHA && git push --force
   origin refs/tags/canary`. The existing tag ref is moved; it is never
   deleted and recreated, and `target_commitish` is not used for this (it
   does not move an existing tag).
4. **Publish** with `softprops/action-gh-release@v3.0.4` against `tag_name:
   canary`: the action reuses the existing Release (creating it on the very
   first publish), overwrites the same-named asset files
   (`overwrite_files: true`, `preserve_order: true`), and — because the
   manifest is listed last — uploads `apkscope-update.json` **after** every
   executable and the checksums. Once the manifest is back, it describes the
   complete new build: its commit, and every asset's exact size and SHA-256.

The tag, the manifest, and the binaries' embedded commit all refer to the
same full SHA.

### 6.3 Concurrency

The workflow's concurrency group uses `cancel-in-progress: false`: a canary
publish must never be cancelled halfway (a cancelled run could leave the
persistent release without a manifest). New runs queue behind the current
publish instead of replacing it, and the stale-commit check (§6.2.1) makes a
queued, now-outdated run skip.

## 7. The upgrade flow, step by step

1. Resolve the requested channel: an explicit flag, or the binary's embedded
   channel (stable for a dev/legacy build).
2. Resolve the manifest URL: `releases/latest/download/...` (stable),
   `releases/download/canary/...` (canary), or
   `releases/download/vX.Y.Z/...` (`--version`).
3. Download the manifest (curl or wget, HTTPS-only, timeouts, bounded
   retries) and validate it per §5.
4. Compare identities: stable by version, canary by full commit SHA. Already
   current → done. `--check` → report and exit.
5. Download the platform's asset to a staging file **beside** the installed
   executable (same filesystem, so the final move is a rename).
6. Verify the exact byte size, the SHA-256, and a plausible executable
   header (ELF / Mach-O / MZ) for this platform.
7. Make the staged file executable and run `staged --version --json`;
   require the manifest's channel, version and full commit.
8. Back up the current executable; rename the staged file into place
   (atomic on Unix). On Windows the running exe is renamed aside first —
   a running exe can be renamed but not replaced or deleted.
9. Verify again from the installed path; on failure restore the backup.
10. Remove the backup only after that succeeds. Report the new version,
    channel and commit.

If the installation appears to be owned by a package manager (dpkg/rpm
database entry on Linux, a Homebrew prefix on macOS), the upgrade refuses and
points at the package manager. This detection is best-effort and documented
as such: a standalone binary copied into `/usr/local/bin` by hand upgrades
normally; an apt-owned one does not.

## 8. What the updater never does

- No GitHub REST or GraphQL API calls, no Actions or Buildkite artifact
  downloads, no tokens or login.
- No release-page HTML scraping; no `version.txt`.
- No download over the installed executable; nothing replaces it before
  size, digest and identity checks pass.
- No fallback to an unverified download on any failure — a failed upgrade
  leaves the installation exactly as it was.
- No background updates; no HTTPS→HTTP downgrade (`curl --proto =https`,
  `wget` defaults).

## 9. Testing the updater

The upgrade module's tests live in `tests/apkscope/upgrade*.rs`, wired in
with `#[path]` module stubs like the rest of the suite (see `tests/README.md`)
— `cargo test` runs them all. They cover manifest validation (every rejection
in §5), channel selection and option conflicts, size/digest verification,
staged-identity mismatches, and the staged replacement: install, permissions,
identity-mismatch rejection and post-install rollback, using a real fixture
binary compiled from a generated source. Unit tests never touch the network
or the real installed executable.

End-to-end against real releases: install a released build
(`scripts/install.sh`), then `apkscope upgrade --check` and
`apkscope upgrade`. Contributors with a fork can point `APKSCOPE_REPO` and
`APKSCOPE_BASE_URL` at their own releases to test the whole pipeline.

## 10. Installer compatibility

`scripts/install.sh` and `scripts/install.ps1` keep their behavior: stable by
default, an exact version on request. An optional `APKSCOPE_CHANNEL=canary`
env (shell) selects the canary release; the label it prints reads the release
manifest — the same single source of truth — so both sides of an update agree
on what "canary (1a2b3c)" means. The installers introduce no second update
protocol.

---

**Summary:** CI builds APKScope; GitHub Releases distribute it. Stable
releases are immutable and versioned; canary is one persistent prerelease on
one moving `canary` tag. Every release carries one `apkscope-update.json`
with the channel, package version, full commit SHA and each platform asset's
exact name, size and SHA-256. The CLI downloads the manifest and the binary
through ordinary public release URLs — no API, no token — verifies
everything, and replaces the local executable safely, with a rollback path.
