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

Every stable and canary Release contains:

- The platform executable assets.
- `checksums.txt` in standard `sha256sum` format.
- `apkscope-update.json`, a small publication manifest and readiness marker.

Example manifest:

```json
{
  "schema_version": 1,
  "channel": "canary",
  "version": "0.10.0",
  "commit": "0123456789abcdef0123456789abcdef01234567",
  "assets": [
    {
      "platform": "linux-aarch64-gnu",
      "name": "apkscope_linux_aarch64",
      "size": 12345678,
      "sha256": "lowercase-64-character-sha256"
    }
  ]
}
```

The actual manifest lists every updater-supported executable.

Validation requirements:

- Reject unknown `schema_version` values.
- The manifest channel must match the requested channel.
- The manifest version and commit must identify the Release's build.
- The manifest must contain exactly one matching platform asset.
- Asset names must match the expected platform mapping; do not trust arbitrary URLs or names from the manifest.
- The selected asset must have a valid byte size and SHA-256 digest.
- The manifest is generated from the final build outputs, not handwritten.

The updater verifies both the manifest's size/digest and the downloaded bytes. A missing manifest, missing asset, missing checksum, malformed digest, size mismatch, or checksum mismatch is a hard error. It must never warn and continue with an unverified binary.

`checksums.txt` is retained for the shell/PowerShell installers and manual downloads. The structured manifest is the updater's channel, commit and asset contract.

A checksum downloaded from the same GitHub Release is useful for detecting corruption and inconsistent publication, but is not independent authenticity protection if the GitHub account or release credentials are compromised. Signing the manifest and verifying it with a public key embedded in APKScope is a possible later hardening step.

## 6. CI and publication lifecycle

Reuse the platform build matrix and artifact collection in the existing release workflow. Do not maintain a second, divergent platform matrix just for canary.

### 6.1 Stable publication

Trigger on version tags matching `v*`.

1. Run formatting, checks, lint and every required platform build.
2. Collect all platform outputs.
3. Generate `checksums.txt` and `apkscope-update.json` from those outputs.
4. Publish the versioned GitHub Release only after every required build succeeds.
5. Do not force-move the stable tag or overwrite an already published stable release's binaries.

A failed platform build prevents stable publication.

### 6.2 Canary publication

Trigger on pushes to `main`, **not** on pushes to the `canary` tag.

1. Run the same validation and platform build jobs as stable.
2. Collect all platform outputs.
3. Generate checksums and the manifest from those exact outputs.
4. Serialize the publication job with a concurrency group. Do not cancel a publication halfway through.
5. Before publishing, confirm the build commit is still the current `main` HEAD. Skip stale runs.
6. Reuse the existing GitHub Release tagged `canary`; create it on the first successful publication if it does not exist.
7. Invalidate the existing `apkscope-update.json` readiness marker before replacing any canary binaries. This makes clients fail closed during asset replacement.
8. Replace the executable assets and `checksums.txt` on the existing Release, retaining the exact filenames.
9. Upload the new `apkscope-update.json` **last**, after every referenced asset and checksum has uploaded successfully.
10. Move the remote lightweight Git tag `canary` to the exact commit recorded in the new manifest.
11. Recheck that the build is not stale immediately before the final tag update.
12. Keep the Release marked as a prerelease and do not mark it as GitHub's latest stable release.

The workflow requires `contents: write` permission. The moving tag is updated as a Git ref; do not delete the release or recreate the tag for every build.

### 6.3 Why the publication marker and order matter

GitHub Release asset replacement is not an atomic multi-file transaction. Clients must not see a mixture of old and new assets as a valid release.

The canary publication sequence therefore uses `apkscope-update.json` as a readiness marker:

- Remove/invalidate the old marker before touching any binary assets. While it is absent, the updater refuses the release.
- Upload all new binaries and checksums.
- Upload the new manifest last. At this point it identifies the new commit, but the old `canary` tag still points to the previous commit, so the updater still refuses the release.
- Move the `canary` tag to the manifest's commit. Only after this final step do the manifest and tag agree, making the release installable.

The updater must check the tag/manifest agreement on every canary update. It must not treat a missing or mismatched marker as a reason to fall back to another source.

If the first canary Release does not exist yet, create the `canary` tag/Release as part of the first publication, but keep the readiness marker absent until all assets are uploaded and validated.

## 7. Update discovery and download algorithm

The updater uses the GitHub Releases API for discovery and GitHub Release asset URLs for downloads.

### Stable

- Latest: `GET /repos/RohanDaCoder/apkscope/releases/latest`
- Specific version: `GET /repos/RohanDaCoder/apkscope/releases/tags/v<version>`

Require a non-draft release. For latest stable, reject prereleases. Validate that the manifest channel is `stable` and that the manifest version corresponds to the release tag.

### Canary

- Release: `GET /repos/RohanDaCoder/apkscope/releases/tags/canary`
- Tag ref: `GET /repos/RohanDaCoder/apkscope/git/ref/tags/canary`

Require a non-draft prerelease whose tag is exactly `canary`. Validate that:

- `apkscope-update.json` exists.
- The manifest channel is `canary`.
- The manifest commit is the full commit SHA referenced by the lightweight `canary` tag.
- The manifest has the selected platform asset.
- The Release has the selected asset and `checksums.txt`.

If the manifest and tag disagree, or the marker/asset is missing, report that the canary release is being published or is inconsistent. Do not download/install anything.

### Common flow

1. Resolve the requested channel (explicit option or embedded build channel).
2. Fetch and validate Release metadata and the update manifest.
3. Determine OS, architecture and Linux libc family.
4. Select the exact supported platform entry.
5. Compare local and remote identities:
   - Stable: compare release/package versions.
   - Canary: compare full commit SHAs.
6. For `--check`, report the result without downloading.
7. Download the raw executable to a temporary file beside the installed executable.
8. Enforce a reasonable download size limit and verify exact byte size and SHA-256 before installation.
9. Validate the downloaded file as a plausible executable for the current platform.
10. Install using the staged replacement procedure.
11. Run the new binary's version command and verify that its channel, version and commit match the target.

Handle HTTP errors, GitHub rate limits, redirects, timeouts, interrupted downloads, missing assets and malformed JSON with useful errors. Every failure before the replacement step must leave the installed binary untouched.

The updater uses the canonical repository `RohanDaCoder/apkscope`. It must not trust repository names or arbitrary download URLs supplied by release metadata. Fork/mirror overrides, if retained, must be explicit configuration and must preserve the same validation rules.

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
- [ ] Plain `upgrade` follows the installed binary's embedded channel.
- [ ] `--stable` and `--canary` switch channels and are mutually exclusive.
- [ ] Legacy binaries without channel metadata default to stable.
- [ ] Canary comparison uses full commit SHA, even when package versions are identical.

### Publishing

- [ ] Canary publication reuses the existing Release; it does not delete/recreate it.
- [ ] Canary assets retain the exact same filenames between builds.
- [ ] The readiness marker is invalidated before asset replacement and uploaded last.
- [ ] The `canary` tag points to the exact commit recorded in the manifest.
- [ ] Missing/mismatched marker/tag causes the updater to refuse the release.
- [ ] A failed platform build cannot publish or move the canary tag.
- [ ] A stale build cannot move the canary tag backwards.
- [ ] Canary remains a prerelease and never becomes latest stable.
- [ ] Stable tags are never force-moved and stable assets are never overwritten.

### Platform and integrity

- [ ] Every supported target maps to its exact asset filename.
- [ ] Linux glibc and musl select different assets.
- [ ] Missing asset/manifest/digest, invalid JSON, size mismatch and checksum mismatch all abort safely.
- [ ] Unsupported targets produce a useful error.

### Installation

- [ ] A failed download or verification leaves the current executable unchanged.
- [ ] Unix replacement preserves executable permissions.
- [ ] Windows replacement waits for the running process to exit.
- [ ] A failed post-install check rolls back to the previous executable.
- [ ] Package-manager-owned installations are refused with useful guidance.
- [ ] Stable-to-canary and canary-to-stable switches report the correct channel and commit.

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

This document is the target design. Existing code on `feat/update-command` is an earlier draft and should not be treated as the specification.

The draft currently differs in several material ways:

- It exposes `apkscope update`; the target command is `apkscope upgrade`.
- It identifies canaries using a generated `<next-version>-canary.<sha6>` version and `version.txt`; the target identity is the embedded channel plus full Git commit SHA, with a structured release manifest.
- Its canary workflow deletes and recreates the tag and Release; the target preserves the Release, overwrites assets, then moves the existing `canary` tag.
- Its canary workflow uses `cancel-in-progress: true`; publication must instead be serialized without cancelling a partially completed publish.
- It treats a missing checksum as a warning; the target requires successful verification and fails closed.
- Its replacement path does not perform the full staged backup, post-install verification and rollback contract described above.

The implementation should be reconciled with this document before merging.

---

**Summary:** CI builds APKScope; GitHub Releases distribute it. Stable releases are immutable and versioned. Canary uses one persistent prerelease and one moving `canary` tag. Every canary publication replaces the existing release assets under fixed filenames, invalidates and republishes a commit-bound readiness manifest, then moves the tag. The updater validates channel, commit, platform, size and SHA-256 before safely replacing the local executable.
