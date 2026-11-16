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
| Discovery | Download the manifest from the latest-release asset URL |

Latest stable manifest:

```text
https://github.com/RohanDaCoder/apkscope/releases/latest/download/apkscope-update.json
```

A specific stable version uses its versioned Release:

```text
https://github.com/RohanDaCoder/apkscope/releases/download/v0.10.0/apkscope-update.json
```

The client follows GitHub's ordinary HTTPS release-download redirect. It does not call the GitHub REST API or require authentication.

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
| Discovery | Download the manifest from the fixed canary Release asset URL |

Canary manifest:

```text
https://github.com/RohanDaCoder/apkscope/releases/download/canary/apkscope-update.json
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

**Do not delete and recreate the canary Release or delete and recreate its tag on every build.** Keep the same Release and move the existing tag ref as part of publishing the new build.

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

**`apkscope-update.json` is the single source of truth for update discovery and asset verification.**

The CLI does not call the GitHub REST API, GraphQL API, Git references API, or GitHub Actions artifacts endpoints. It downloads one small JSON file from the public GitHub Release using an ordinary HTTPS release-download URL. No GitHub token, OAuth flow, login, or API request quota is involved.

Each stable and canary Release must contain:

- The platform executable assets.
- `apkscope-update.json` (required by the self-updater).
- `checksums.txt` (retained for the existing shell/PowerShell installers and manual verification; the self-updater does not need to download it).

### Manifest schema

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
    },
    "linux-aarch64-musl": {
      "name": "apkscope_linux_musl_aarch64",
      "size": 5389472,
      "sha256": "lowercase-64-character-sha256"
    }
  }
}
```

The actual manifest lists every supported updater platform ID. Keep the schema deliberately small:

- `schema_version`: integer schema version.
- `channel`: `stable` or `canary`.
- `version`: Cargo/package version for this build (without inventing a canary SemVer suffix).
- `commit`: full 40-character Git commit SHA used to build the binaries.
- `assets`: map from the platform IDs in section 4 to the exact asset name, byte size and lowercase SHA-256 digest.

Do not include arbitrary download URLs. The updater constructs the binary URL from the trusted repository, selected release tag/channel, and the known asset name. Do not duplicate release notes, authors, timestamps, GitHub asset IDs, or other unnecessary metadata.

### Manifest URLs

Stable latest:

```text
https://github.com/RohanDaCoder/apkscope/releases/latest/download/apkscope-update.json
```

Canary:

```text
https://github.com/RohanDaCoder/apkscope/releases/download/canary/apkscope-update.json
```

Specific stable version:

```text
https://github.com/RohanDaCoder/apkscope/releases/download/v0.10.0/apkscope-update.json
```

The client downloads the manifest and binary through the normal public Release asset download URLs. These requests do not consume the GitHub REST API's unauthenticated request quota. GitHub may still apply general download/abuse protections, so the updater should use timeouts, bounded retries and clear errors.

### Validation

- Reject unknown `schema_version` values.
- The manifest channel must match the requested channel.
- For stable, the manifest version must match the version encoded in the selected versioned release tag (or latest release URL).
- For canary, compare the manifest's full commit SHA with the commit embedded in the downloaded binary.
- The manifest must contain exactly one entry for the current platform.
- The asset name must exactly match the expected platform mapping in section 4.
- The selected asset must have a positive, reasonable byte size and a valid 64-character SHA-256 digest.
- Verify the downloaded file's exact byte size and SHA-256 before installation.
- Reject malformed JSON, missing fields, unknown platform entries, path separators in asset names, malformed digests, size mismatches and checksum mismatches.
- Never install if the manifest is missing or invalid. There is no API or unverified-download fallback.

The same manifest is generated from the final build outputs. `checksums.txt` may be generated from those files too, but it is not a second source of truth for the self-updater.

A checksum published beside the binary detects accidental corruption and mismatched assets. It is not independent authenticity protection if the GitHub account or release credentials are compromised. Signing the manifest and verifying it with a public key embedded in APKScope is optional future hardening, not a requirement for the initial updater.

## 6. CI and publication lifecycle

Reuse the platform build matrix and artifact collection in the existing release workflow. Do not maintain a second, divergent platform matrix just for canary.

### 6.1 Stable publication

Trigger on version tags matching `v*`.

1. Run formatting, checks, lint and every required platform build.
2. Collect all platform outputs.
3. Generate `apkscope-update.json` and `checksums.txt` from those exact outputs.
4. Publish the versioned GitHub Release only after every required build succeeds.
5. Include `apkscope-update.json` as a required release asset.
6. Do not force-move the stable tag or overwrite an already published stable release's binaries.

A failed platform build prevents stable publication.

### 6.2 Canary publication

Trigger on pushes to `main`, **not** on pushes to the `canary` tag.

1. Run the same validation and platform build jobs as stable.
2. Collect all platform outputs.
3. Generate `checksums.txt` and `apkscope-update.json` from those exact outputs.
4. Serialize the publication job. Do not cancel a publication halfway through.
5. Before publishing, confirm the build commit is still the current `main` HEAD. Skip stale runs.
6. Reuse the existing GitHub Release tagged `canary`; create it on the first successful canary publication if it does not exist.
7. Remove/invalidate the old `apkscope-update.json` asset before replacing the canary binaries. This makes clients fail closed while the release is being updated.
8. Replace the executable assets and `checksums.txt` on the existing Release, retaining the exact filenames.
9. Move the remote lightweight Git tag `canary` to the exact commit used to build the binaries.
10. Upload the new `apkscope-update.json` **last**, after all binaries, checksums and the tag have been updated successfully.
11. Keep the Release marked as a prerelease and do not mark it as GitHub's latest stable release.

The workflow requires `contents: write` permission. The moving tag is updated as part of the CI publisher; the installed CLI does not inspect Git refs or call any GitHub API.

### 6.3 Canary publication consistency

GitHub Release asset replacement is not an atomic multi-file transaction. The manifest is therefore also the readiness marker.

The publication sequence is:

1. Delete the previous manifest asset.
2. Replace all fixed-name executable assets and `checksums.txt`.
3. Move the `canary` tag to the commit used for those binaries.
4. Upload the new manifest last.

While the manifest is absent, `apkscope upgrade --canary` reports that the canary release is being published and makes no changes. Once the manifest is available, its commit and hashes describe the new build.

After downloading and verifying a candidate executable, the updater runs the staged binary's version command **before replacing the installed executable**. It requires the embedded channel and full commit SHA to match the manifest. If they differ, the release is inconsistent or a stale CDN copy was returned; abort without replacing the current executable.

This design needs no client-side REST calls, release ID lookups, tag queries, or per-asset API metadata. The manifest is the only update metadata the client needs.

## 7. Update discovery and download algorithm

The updater uses **ordinary HTTPS downloads of GitHub Release assets only**. It must not call the GitHub REST API or GraphQL API, and must not require authentication.

### Resolve the manifest URL

- Latest stable: `https://github.com/RohanDaCoder/apkscope/releases/latest/download/apkscope-update.json`
- Canary: `https://github.com/RohanDaCoder/apkscope/releases/download/canary/apkscope-update.json`
- Specific stable version: `https://github.com/RohanDaCoder/apkscope/releases/download/v<version>/apkscope-update.json`

Use `curl` or `wget` (or an HTTP client) with HTTPS, redirect following, connection/overall timeouts and bounded retries. A 404 means the requested public Release/asset is not published; report that clearly. It does not mean authentication is required.

### Common flow

1. Resolve the requested channel (explicit option or embedded build channel).
2. Download `apkscope-update.json` from the corresponding stable/canary Release URL.
3. Validate the manifest schema, channel, version, full commit SHA and platform asset entry.
4. Construct the selected executable's Release download URL using the trusted repository, selected release tag/channel and exact asset name from the validated platform entry.
5. Compare local and remote identities:
   - Stable: compare package/release versions.
   - Canary: compare full commit SHAs.
6. For `--check`, report the result without downloading or installing the executable.
7. Download the raw executable to a temporary file beside the installed executable.
8. Enforce the manifest's exact byte size and verify SHA-256.
9. Validate the downloaded file as a plausible executable for the current platform.
10. Execute the staged binary's version command and verify channel, version and full commit SHA against the manifest.
11. Install using the staged replacement procedure.

A missing or malformed manifest, missing platform asset, invalid digest, size mismatch, checksum mismatch or embedded-build-identity mismatch is a hard failure. Never fall back to an unverified download or another channel.

The manifest must not be allowed to supply an arbitrary URL or path. Only its validated asset filename is used, and that name must exactly match the platform mapping in section 4.

The updater uses the canonical repository `RohanDaCoder/apkscope`. Fork/mirror overrides, if retained, must be explicit configuration and must preserve the same validation rules.

The client makes no GitHub API requests. Consequently it does not consume GitHub's unauthenticated REST API request quota. GitHub may still apply general CDN/download abuse controls; use bounded retries and report failures rather than spinning indefinitely.

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

- [ ] Stable latest uses the `releases/latest/download/apkscope-update.json` asset URL.
- [ ] Canary uses the `releases/download/canary/apkscope-update.json` asset URL.
- [ ] Specific stable versions use their versioned release asset URL.
- [ ] The updater makes no GitHub REST API or GraphQL API requests.
- [ ] No GitHub token, OAuth flow or user login is required.
- [ ] Plain `upgrade` follows the installed binary's embedded channel.
- [ ] `--stable` and `--canary` switch channels and are mutually exclusive.
- [ ] Canary comparison uses the full commit SHA, even when package versions are identical.

### Manifest and integrity

- [ ] Every stable and canary Release contains a valid `apkscope-update.json`.
- [ ] The manifest contains schema version, channel, package version, full commit SHA, and per-platform asset filename/size/SHA-256.
- [ ] The updater rejects malformed, missing or unsupported manifest data.
- [ ] The updater downloads only the exact expected asset for the current platform.
- [ ] The downloaded asset's exact size and SHA-256 must match the manifest.
- [ ] The staged executable's embedded channel, version and commit must match the manifest.
- [ ] There is no API fallback and no unverified-download mode.

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

- It exposes `apkscope update`; the target command is `apkscope upgrade`.
- It identifies canaries using a generated `<next-version>-canary.<sha6>` version and `version.txt`; the target uses the embedded channel plus full Git commit SHA.
- It deletes and recreates the canary tag and Release; the target preserves the Release, overwrites its assets, and moves the existing tag.
- It uses `cancel-in-progress: true`; publication must be serialized without cancelling a partially completed publish.
- It treats a missing checksum as a warning; the target requires successful verification against the manifest and fails closed.
- It does not perform the full staged backup, pre-install identity check, post-install verification and rollback contract.
- It does not yet use the manifest as the sole client-side release metadata source.

The implementation should be reconciled with this document before merging.

---

**Summary:** CI builds APKScope; GitHub Releases distribute it. Stable releases are immutable and versioned. Canary uses one persistent prerelease and one moving `canary` tag. Each Release contains one `apkscope-update.json` manifest with the channel, package version, full commit SHA, and every platform asset's filename, size and SHA-256. The CLI downloads that manifest and the selected binary through ordinary GitHub Release asset URLs only: no GitHub REST API, authentication, or API-rate-limit dependency. It verifies platform, channel, commit, size and SHA-256 before safely replacing the local executable.
