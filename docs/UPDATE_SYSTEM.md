# APKScope Stable / Canary Update System

**Status:** Proposed design (normative target for implementation)  
**Repository:** `RohanDaCoder/apkscope`  
**Last updated:** 2026-11-16

## 1. Purpose and principles

Implement a self-updater for APKScope with two channels:

- **Stable** — versioned releases for normal use.
- **Canary** — the newest successfully published build from `main`, for testing development changes.

The design is intentionally simple:

1. CI builds binaries.
2. CI publishes those binaries as GitHub Release assets.
3. The CLI discovers the requested GitHub Release and downloads its matching asset.
4. The CLI verifies the download and safely replaces the installed executable.

**GitHub Releases are the distribution interface.** GitHub Actions artifacts (or Buildkite artifacts) may be used internally to transfer build outputs between CI jobs, but the installed CLI never downloads workflow artifacts or Buildkite artifacts.

The updater must fail closed: a missing asset, incomplete publication, invalid metadata, missing checksum, checksum mismatch, or installation failure must not leave the user with a partially replaced executable.

## 2. Release channels

### 2.1 Stable

| Property | Contract |
| --- | --- |
| Git tag | Immutable version tag, for example `v0.10.0` |
| GitHub Release | One release per version |
| Prerelease | No, except explicitly versioned release candidates |
| Tag movement | Never force-move a published stable tag |
| Asset behavior | Publish versioned assets once; do not overwrite an existing stable release |
| Discovery | GitHub latest-release API |

Latest stable endpoint:

```http
GET https://api.github.com/repos/RohanDaCoder/apkscope/releases/latest
```

A specific stable version is resolved by its exact tag:

```http
GET https://api.github.com/repos/RohanDaCoder/apkscope/releases/tags/v0.10.0
```

### 2.2 Canary

| Property | Contract |
| --- | --- |
| Source | Successful builds of the `main` branch |
| Git tag | One permanent, movable tag named `canary` |
| GitHub Release | One persistent prerelease attached to `canary` |
| Prerelease | Yes |
| Latest stable | Must not be marked as GitHub's latest stable release |
| Tag movement | Force-update `canary` to the commit represented by the newly published build |
| Asset behavior | Replace assets on the existing Release, retaining the same filenames |
| Discovery | GitHub release-by-tag API |

Canary endpoint:

```http
GET https://api.github.com/repos/RohanDaCoder/apkscope/releases/tags/canary
```

The `canary` tag is a **moving pointer**, not a version number. It always points to the commit for the latest successfully published canary build. Do not create a unique Git tag or GitHub Release for each canary commit.

Example:

| Successful publication | `canary` points to | Assets in the canary Release |
| --- | --- | --- |
| First | Commit A | Binaries built from A |
| Second | Commit B | Binaries built from B |
| Third | Commit C | Binaries built from C |

The Release page and download URLs stay stable:

```text
https://github.com/RohanDaCoder/apkscope/releases/tag/canary
https://github.com/RohanDaCoder/apkscope/releases/download/canary/apkscope_linux_aarch64
```

**Do not delete and recreate the canary Release or delete and recreate its tag on every build.** Keep the same Release and move the existing tag ref after the new assets have been published.

## 3. CLI interface and channel selection

The command is named `upgrade`.

| Command | Behavior |
| --- | --- |
| `apkscope upgrade` | Upgrade within the channel embedded in the installed binary |
| `apkscope upgrade --canary` | Switch to and upgrade from canary |
| `apkscope upgrade --stable` | Switch to and upgrade from stable |
| `apkscope upgrade --check` | Check for an update without downloading or installing |
| `apkscope upgrade --version 0.10.0` | Install the specified stable version |

Rules:

- `--canary` and `--stable` are mutually exclusive.
- `--version` selects a stable version and cannot be combined with `--canary`.
- Plain `upgrade` preserves the installed binary's channel.
- A new installation defaults to stable.
- A legacy binary without channel metadata is treated as stable.
- There are no background or automatic updates; updating is user-initiated.

### Build identity and version output

Every release binary embeds:

- Package version from `Cargo.toml`.
- Build channel: `stable` or `canary`.
- Full Git commit SHA used to build the binary.

Suggested build-time values are `APKSCOPE_BUILD_CHANNEL` and `APKSCOPE_GIT_SHA`. Local development builds may use a `dev`/unknown fallback.

Example:

```text
apkscope 0.10.0
channel: canary
commit:  a1b2c3d4e5f6...
```

A canary's identity is its commit SHA, **not** a generated SemVer suffix. Multiple canary builds may share the same Cargo package version; compare their commit SHAs to determine whether an update exists.

The updater must be able to read the embedded channel and commit independently of the human-readable version string.

## 4. Published assets and platform mapping

Keep the existing release asset names. The updater downloads the raw executable; it does not need to extract an archive.

| Platform ID | Release asset | Contents |
| --- | --- | --- |
| `linux-x86_64-gnu` | `apkscope_linux_x86_64` | Viewer + CLI + MCP |
| `linux-aarch64-gnu` | `apkscope_linux_aarch64` | Viewer + CLI + MCP |
| `linux-x86_64-musl` | `apkscope_linux_musl_x86_64` | CLI + MCP; no viewer |
| `linux-aarch64-musl` | `apkscope_linux_musl_aarch64` | CLI + MCP; no viewer |
| `windows-x86_64` | `apkscope_windows_x86_64.exe` | Viewer + CLI + MCP |
| `macos-aarch64` | `apkscope_macos_aarch64` | CLI + MCP |
| `macos-x86_64` | `apkscope_macos_x86_64` | CLI + MCP |

The Linux updater must distinguish glibc from musl and select the corresponding asset. It must never substitute one for the other. The detection must be consistent with the official installer.

The existing `.deb` and AppImage packages remain available for their existing installation paths; they are not selected by this self-updater.

## 5. Release metadata and integrity

**Use the GitHub Releases API as the primary metadata source.** Public APKScope releases can be queried anonymously; the CLI must not embed a GitHub token or require users to sign in.

The API already supplies the information the updater needs:

- Release identity: `tag_name`, `draft`, `prerelease`.
- Per-asset download URL: `assets[].browser_download_url`.
- Per-asset size: `assets[].size`.
- Per-asset SHA-256: `assets[].digest` (formatted as `sha256:<hex>`).

Do not download `checksums.txt` just to obtain a digest when the API supplies a valid `assets[].digest`.

### Minimal fallback manifest

Ship one small `apkscope-update.json` asset with each stable and canary Release **only as a fallback**. It is not required on the normal API path.

Use it only when the Releases API cannot be used (for example, network failure, API rate limiting, or a transient GitHub API error), or when a selected API asset has no usable `digest`. The fallback manifest supplies only the metadata that cannot be obtained from the API. It must not introduce a second update service or a second set of release assets.

Example:

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
      "sha256": "lowercase-64-character-sha256"
    }
  }
}
```

The actual manifest lists all supported updater platform IDs. Keep the schema deliberately small: channel, version, full commit SHA, and per-platform asset name/size/SHA-256. Do not duplicate release notes, authors, timestamps, arbitrary download URLs, or other GitHub release metadata.

Fallback manifest URLs are deterministic:

```text
Stable latest:
https://github.com/RohanDaCoder/apkscope/releases/latest/download/apkscope-update.json

Canary:
https://github.com/RohanDaCoder/apkscope/releases/download/canary/apkscope-update.json

Specific stable version:
https://github.com/RohanDaCoder/apkscope/releases/download/v0.10.0/apkscope-update.json
```

The updater constructs the executable download URL from the trusted repository, release tag/channel and the known platform asset name. It must not follow arbitrary URLs supplied by the manifest.

### Validation and fallback rules

- Reject unknown `schema_version` values.
- Validate the requested channel and release tag.
- Validate the selected platform entry, exact expected asset filename, byte size and 64-character SHA-256.
- When using API metadata, validate the `digest` prefix and hex value before use.
- If API metadata is usable and contains a valid digest, use it directly.
- If the API request fails transiently or the asset digest is absent/unsupported, try the fallback manifest.
- If both API metadata and the fallback manifest are unavailable or invalid, stop with a useful error. Never install without a verified digest.
- Do not use the fallback to mask a valid API response that identifies the wrong tag, channel, draft/prerelease state, or platform asset.

Keep the existing `checksums.txt` for the shell/PowerShell installers and manual downloads. The CLI updater does not need to fetch it on the normal API path.

A SHA-256 digest obtained from GitHub or a manifest hosted in the same GitHub Release detects corruption and inconsistent downloads; it is not independent authenticity protection if the GitHub account or release credentials are compromised. Signing releases is optional future hardening, not a requirement for the initial updater.

## 6. CI and publication lifecycle

Reuse the platform build matrix and artifact collection in the existing release workflow. Do not maintain a second, divergent platform matrix just for canary.

### 6.1 Stable publication

Trigger on version tags matching `v*`.

1. Run formatting, checks, lint and every required platform build.
2. Collect all platform outputs.
3. Generate `checksums.txt` and the small fallback `apkscope-update.json` from those exact outputs.
4. Publish the versioned GitHub Release only after every required build succeeds.
5. Do not force-move the stable tag or overwrite an already published stable release's binaries.

A failed platform build prevents stable publication.

### 6.2 Canary publication

Trigger on pushes to `main`, **not** on pushes to the `canary` tag.

1. Run the same validation and platform build jobs as stable.
2. Collect all platform outputs.
3. Generate `checksums.txt` and the fallback manifest from those exact outputs.
4. Serialize the publication job. Do not cancel a publication halfway through.
5. Before publishing, confirm the build commit is still the current `main` HEAD. Skip stale runs.
6. Reuse the existing GitHub Release tagged `canary`; create it on the first successful publication if it does not exist.
7. Replace the executable assets and `checksums.txt` on the existing Release, retaining the exact filenames.
8. Upload/replace `apkscope-update.json` last, after all binaries and checksums have uploaded successfully.
9. Move the remote lightweight Git tag `canary` to the exact commit used to build the binaries.
10. Recheck that the build is not stale immediately before moving the tag.
11. Keep the Release marked as a prerelease and do not mark it as GitHub's latest stable release.

The workflow requires `contents: write` permission. The moving tag is updated as a Git ref; do not delete the Release or recreate the tag for every build.

### 6.3 Canary publication consistency

GitHub Release asset replacement is not an atomic multi-file transaction. The updater must not assume that all fixed-name assets change simultaneously.

The API path must retrieve the current `canary` tag's commit SHA through the public Git references API. After downloading and verifying the selected asset, run the staged executable's version command **before replacing the installed executable**. Require its embedded channel to be `canary` and its full embedded commit SHA to match the current `canary` tag SHA. If they differ, the Release is being updated or is inconsistent; abort without replacing the current executable.

The fallback manifest carries the same full commit SHA and is checked against the canary tag when the Git references API is available. If the API is unavailable and the manifest fallback is used, the downloaded binary's embedded commit must still match the manifest commit.

This avoids adding a readiness-marker protocol or release-deletion step. The existing Release remains in place, assets retain stable names, and a client encountering mixed-generation assets rejects the downloaded binary before installation.

## 7. Update discovery and download algorithm

The updater uses the unauthenticated GitHub REST API first. No token, OAuth flow or user login is needed for public releases.

### Stable

- Latest: `GET /repos/RohanDaCoder/apkscope/releases/latest`
- Specific version: `GET /repos/RohanDaCoder/apkscope/releases/tags/v<version>`

Require a non-draft release. For latest stable, require `prerelease == false`. Find the exact asset by the known platform filename and use its `browser_download_url`, `size` and `digest`.

### Canary

- Release: `GET /repos/RohanDaCoder/apkscope/releases/tags/canary`
- Tag identity: `GET /repos/RohanDaCoder/apkscope/git/ref/tags/canary`

Require a non-draft prerelease whose tag is exactly `canary`. Resolve the lightweight tag to its full commit SHA. Find the exact platform asset and use its API-provided download URL, size and digest.

### Fallback behavior

If the release API cannot be used because of a network error, rate limit, or transient server error, retrieve `apkscope-update.json` directly from the corresponding public Release download URL:

- Stable latest: `/releases/latest/download/apkscope-update.json`
- Canary: `/releases/download/canary/apkscope-update.json`
- Specific stable version: `/releases/download/v<version>/apkscope-update.json`

Use the manifest's version, commit and exact platform asset name/size/SHA-256. Construct the binary URL from the known repository, requested tag and known filename; do not trust arbitrary URLs from JSON.

If the API succeeds but an asset's `digest` is absent or unusable, the manifest may be fetched to supply the missing digest and commit metadata. A valid API response that says a release is draft, the wrong channel, or the wrong tag is not an API outage and must not be bypassed using the fallback.

If neither source can provide a valid digest, abort. There is no unverified-download mode.

### Common flow

1. Resolve the requested channel (explicit option or embedded build channel).
2. Query GitHub Releases API anonymously.
3. Validate release state and locate the exact platform asset.
4. Use API-provided `browser_download_url`, `size` and `digest`; use the fallback manifest only when the API/asset metadata is unavailable as described above.
5. Determine OS, architecture and Linux libc family.
6. Compare local and remote identities:
   - Stable: compare release/package versions.
   - Canary: compare full commit SHAs.
7. For `--check`, report the result without downloading or installing.
8. Download the raw executable to a temporary file beside the installed executable.
9. Enforce a reasonable download size limit and verify exact byte size and SHA-256.
10. Validate the downloaded file as a plausible executable for the current platform.
11. Execute the staged binary's version command and verify its channel, version and commit against the expected release identity.
12. Install using the staged replacement procedure.

Handle HTTP errors, GitHub rate limits, redirects, timeouts, interrupted downloads, missing assets and malformed responses with useful errors. Every failure before replacement must leave the installed binary untouched.

A 404 means the requested public Release/tag is not currently published (for example, a fork with no releases yet); report that clearly. It does not mean authentication is required. Do not silently switch to another repository or channel.

The updater uses the canonical repository `RohanDaCoder/apkscope`. Fork/mirror overrides, if retained, must be explicit configuration and must preserve the same validation rules.

## 8. Safe executable replacement

The self-updater targets standalone APKScope installations. It must not silently overwrite a binary owned by apt, Homebrew, Scoop, Cargo or another package manager. Package-managed installations should be updated through their package manager.

Installation procedure:

1. Resolve the actual running executable path.
2. Check that it is a supported standalone installation and the destination is writable.
3. Download and verify the new executable before changing the current one.
4. Stage the verified file beside the existing executable, on the same filesystem.
5. Preserve Unix executable permissions.
6. Preserve the old executable as a rollback backup.
7. Replace using a same-filesystem atomic rename where supported.
8. On Windows, use a small helper process to wait for the original process to exit before replacing the running executable.
9. Run the new executable with `--version`; verify expected channel, version and commit.
10. Remove the backup only after post-install verification succeeds.
11. Restore the backup if replacement or post-install verification fails.

Do not overwrite the current executable with a partial download, unverified file or unsupported platform binary. Do not request elevation automatically; explain permission problems and let the user decide how to proceed.

## 9. Repository alignment

The canonical release repository is `RohanDaCoder/apkscope`.

Align the defaults and public links in:

- `Cargo.toml` package repository metadata.
- `scripts/install.sh` and `scripts/install.ps1`.
- README installation/release links.
- The updater's repository configuration.

The current code on this feature branch contains some `suffixing/apkscope` defaults. Those must be changed before the implementation is considered ready for this repository's releases.

The installers should use the same stable/canary release contract and strict checksum behavior. An optional `APKSCOPE_CHANNEL=canary` installer setting may select the canary Release; stable remains the default.

## 10. Implementation boundaries

The existing CLI uses a hand-written argument dispatcher and is synchronous. Integrate the updater with that architecture; do not introduce a new CLI framework or async runtime solely for updating.

Suggested module layout:

```text
src/
└── upgrade/
    ├── mod.rs
    ├── channel.rs
    ├── release.rs
    ├── manifest.rs
    ├── platform.rs
    ├── download.rs
    ├── verify.rs
    └── install.rs
```

Responsibilities:

- `channel.rs`: current/requested channel and command option validation.
- `release.rs`: GitHub Release and tag discovery.
- `manifest.rs`: manifest schema and validation.
- `platform.rs`: OS, architecture and Linux libc selection.
- `download.rs`: bounded download to a temporary file.
- `verify.rs`: size, checksum and executable validation.
- `install.rs`: staged replacement, Windows helper and rollback.

A blocking HTTP client is appropriate for the current synchronous command architecture. The exact HTTP/temp-file crates are implementation choices, not part of the release contract.

## 11. Acceptance criteria

### Channel and discovery

- [ ] Stable discovery selects the latest non-prerelease release and never selects canary.
- [ ] Canary discovery resolves only the Release tagged `canary`.
- [ ] Specific stable versions resolve by exact `vX.Y.Z` tag.
- [ ] Public API requests work without authentication; no token is embedded in the binary.
- [ ] Plain `upgrade` follows the installed binary's embedded channel.
- [ ] `--stable` and `--canary` switch channels and are mutually exclusive.
- [ ] Canary comparison uses the full commit SHA, even when package versions are identical.

### Metadata and fallback

- [ ] API-provided asset URL, size and SHA-256 digest are used when available.
- [ ] `apkscope-update.json` is used only when API discovery fails transiently or API asset digest/commit metadata is unavailable.
- [ ] The fallback manifest contains only schema version, channel, version, full commit SHA and platform asset name/size/SHA-256.
- [ ] Missing/invalid metadata from both sources aborts safely; no unverified download path exists.
- [ ] The updater does not fetch `checksums.txt` just to obtain a digest when the API provides one.

### Publishing

- [ ] Canary publication reuses the existing Release; it does not delete/recreate it.
- [ ] Canary assets retain the exact same filenames between builds.
- [ ] The fallback manifest is uploaded after the binary assets and checksums.
- [ ] The `canary` tag points to the exact commit used to build the published binaries.
- [ ] A canary binary whose embedded commit does not match the tag/manifest is rejected before installation.
- [ ] A failed platform build or stale run cannot publish/move the canary tag.
- [ ] Canary remains a prerelease and never becomes latest stable.
- [ ] Stable tags are never force-moved and stable assets are never overwritten.

### Platform, integrity and installation

- [ ] Every supported target maps to its exact asset filename.
- [ ] Linux glibc and musl select different assets.
- [ ] Missing asset/digest, invalid JSON, size mismatch and checksum mismatch all abort safely.
- [ ] A failed download or verification leaves the current executable unchanged.
- [ ] Unix replacement preserves executable permissions.
- [ ] Windows replacement waits for the running process to exit.
- [ ] Failed post-install verification rolls back to the previous executable.
- [ ] Package-manager-owned installations are refused with useful guidance.

## 12. Explicit non-goals

The initial implementation does not include:

- Client downloads from GitHub Actions artifacts or Buildkite artifacts.
- A custom update server or separate CDN.
- Background or automatic updates.
- In-place updates of package-manager-owned installations.
- Archive-based self-update bundles.
- Mandatory release signing infrastructure (recommended as later hardening).

---

## 13. Migration notes for the current feature-branch draft

This document is the target design. Existing code on `feat/update-command` is an earlier draft and should be reconciled with it before merging.

Material differences to address:

- The draft exposes `apkscope update`; the target command is `apkscope upgrade`.
- The draft identifies canaries using a generated `<next-version>-canary.<sha6>` version and `version.txt`; the target uses the embedded channel plus full Git commit SHA.
- The draft deletes and recreates the canary tag and Release; the target preserves the Release, overwrites its assets, and moves the existing tag.
- The draft uses `cancel-in-progress: true`; publication must be serialized without cancelling a partially completed publish.
- The draft treats missing checksums as a warning; the target requires a valid API digest or fallback-manifest digest and fails closed.
- The draft does not perform the full staged backup, pre-install identity check, post-install verification and rollback contract.
- The target uses GitHub Releases API metadata first and keeps `apkscope-update.json` only as a minimal fallback, not as the primary release metadata source.

The implementation should be reconciled with this document before merging.

---

**Summary:** CI builds APKScope; GitHub Releases distribute it. Stable releases are immutable and versioned. Canary uses one persistent prerelease and one moving `canary` tag. The updater uses the unauthenticated GitHub Releases API as its primary metadata source, including each asset's download URL, size and SHA-256 digest. A small `apkscope-update.json` is shipped only as a fallback for API/rate-limit failures or missing digest metadata. The updater verifies platform, channel, commit, size and SHA-256 before safely replacing the local executable.
