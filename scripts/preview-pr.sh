#!/bin/sh
# apkscope PR-preview installer for Linux and macOS.
#
# Downloads the CI build of a pull request (the workflow artifacts ci.yml
# uploads for every PR), verifies the SHA-256 digest GitHub reports for the
# artifact, and installs it SIDE BY SIDE as `apkscope-pr-<N>`. The real
# install is never touched, and the preview never goes through
# `apkscope upgrade`: per #52, the updater stays limited to signed-off
# stable and canary releases.
#
# Usage:
#   ./scripts/preview-pr.sh 53            # the latest build of PR #53
#   ./scripts/preview-pr.sh 53 1a2b3c4    # only if that is the PR's head commit
#
# Needs a GitHub login: the artifact download API requires authentication even
# on public repos (GitHub policy). `gh` (logged in) or GITHUB_TOKEN is enough;
# without either, the script prints the Checks-tab link where any logged-in
# browser can download the same zip by hand, and APKSCOPE_PREVIEW_ZIP=... (or
# --zip) installs from that file.
#
# Environment overrides (for forks, mirrors, custom locations):
#   APKSCOPE_REPO=suffixing/apkscope        # GitHub owner/name to download from
#   APKSCOPE_INSTALL_DIR=/usr/local/bin     # where to put the binary
#   APKSCOPE_PREVIEW_ZIP=/path/to/zip       # a hand-downloaded artifact zip
#   GITHUB_TOKEN                            # a token, when gh is not installed
set -eu

REPO="${APKSCOPE_REPO:-suffixing/apkscope}"
DEST="${APKSCOPE_INSTALL_DIR:-$HOME/.local/bin}"
API="${GITHUB_API_URL:-https://api.github.com}/repos/$REPO"
PR="${1:-}"
WANT_SHA="${2:-}"
ZIP="${APKSCOPE_PREVIEW_ZIP:-}"

usage() {
  sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
  exit 1
}

case "${1:-}" in
  -h|--help) usage ;;
esac

# --- output helpers (same vocabulary as install.sh) ------------------------------
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != "dumb" ]; then
  bold=$(printf '\033[1m'); dim=$(printf '\033[2m')
  green=$(printf '\033[32m'); yellow=$(printf '\033[33m'); red=$(printf '\033[31m')
  reset=$(printf '\033[0m')
else
  bold=""; dim=""; green=""; yellow=""; red=""; reset=""
fi
if [ -t 1 ] && [ "${TERM:-}" != "dumb" ]; then
  tick="✓"
else
  tick="+"
fi

field() { printf '  %s%-11s%s %s\n' "$dim" "$1" "$reset" "$2"; }
ok()    { printf '  %s%s%s %s\n' "$green" "$tick" "$reset" "$1"; }
step()  { printf '  %s ...\n' "$1"; }
note()  { printf '%snote:%s %s\n' "$dim" "$reset" "$1" >&2; }
warn()  { printf '%swarning:%s %s\n' "$yellow" "$reset" "$1" >&2; }
die()   { printf '%serror:%s %s\n' "$red" "$reset" "$1" >&2; exit 1; }

[ -n "$PR" ] || usage
case "$PR" in
  *[!0-9]*|'') die "the PR number must be a number (got '$PR')" ;;
esac

# human-readable byte count (POSIX arithmetic only, so no locale surprises)
hsize() {
  if [ "$1" -ge 1048576 ]; then
    printf '%d.%d MB' $(( $1 / 1048576 )) $(( ($1 % 1048576) / 104858 ))
  elif [ "$1" -ge 1024 ]; then
    printf '%d KB' $(( $1 / 1024 ))
  else
    printf '%d B' "$1"
  fi
}

# show a path under $HOME as ~/... when possible
homepath() {
  case "$1" in
    "$HOME"?*) printf '~%s' "${1#"$HOME"}" ;;
    *) printf '%s' "$1" ;;
  esac
}

# --- detect platform ------------------------------------------------------------
os=$(uname -s)
case "$os" in
  Linux)  os=linux ;;
  Darwin) os=macos ;;
  *) die "unsupported OS '$os' (this preview installer covers Linux and macOS; on Windows use scripts/preview-pr.ps1)" ;;
esac

arch=$(uname -m)
case "$arch" in
  x86_64|amd64)  arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) die "unsupported CPU architecture '$arch'" ;;
esac

# The CI matrix builds exactly three preview artifacts: apkscope-linux-x64
# (glibc, viewer included), apkscope-macos-arm64 (CLI + MCP) and
# apkscope-windows-x64. Match the host to what exists, and say clearly when
# there is no native build to hand out.
case "$os-$arch" in
  linux-x86_64)  artifact="apkscope-linux-x64" ;;
  linux-aarch64) die "the CI matrix builds Linux x64 only — there is no aarch64 Linux preview build" ;;
  macos-aarch64) artifact="apkscope-macos-arm64" ;;
  macos-x86_64)  artifact="apkscope-macos-arm64"
                 note "the CI builds macOS arm64 only; on an Intel Mac the preview runs through Rosetta" ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# --- GitHub API access ------------------------------------------------------------
# Listing endpoints work anonymously within IP quota; the artifact download
# needs a login (gh, or GITHUB_TOKEN). Both helpers fall through to plain
# anonymous HTTPS so read-only steps keep working without any credentials.
api_get() {
  # $1 = API path under /repos/<owner>/<repo>; prints the body, fails on 4xx/5xx
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    gh api "repos/$REPO/$1" && return 0
  fi
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$API/$1" && return 0
  fi
  # anonymous: works for read endpoints while IP quota lasts (curl's own
  # stderr is silenced — the callers print the actionable message)
  curl -fsSL "$API/$1" 2>/dev/null
}
api_download() {
  # $1 = API path, $2 = output file; the artifact zip needs authentication
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    gh api "repos/$REPO/$1" > "$2" && return 0
    return 1
  fi
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$API/$1" -o "$2" && return 0
  fi
  return 1
}

# --- JSON field parsing -----------------------------------------------------------
# gh api prints compact JSON (one line), plain curl pretty-prints it. Strip all
# whitespace first so both shapes parse the same, then walk objects with
# index()/substr() — no jq dependency, and the awk is POSIX (unlike sed with
# \n in the replacement, which BSD sed does not support).
json_compact() { tr -d ' \t\r\n'; }

# sha of the "head" object of a PR (compact JSON on stdin)
pr_head_sha() {
  awk '
    { s = index($0, "\"head\":{"); if (!s) exit
      r = substr($0, s)
      p = index(r, "\"sha\":\""); if (!p) exit
      r = substr(r, p + 7)
      print substr(r, 1, index(r, "\"") - 1) }'
}
# "<id> <digest> <size>" of the artifact named $1 in an artifacts listing
pick_artifact() {
  awk -v want="\"name\":\"$1\"" '
    { gsub(/\},\{/, "}\n{"); n = split($0, rec, "\n")
      for (i = 1; i <= n; i++) {
        if (index(rec[i], want) == 0) continue
        line = rec[i]
        p = index(line, "\"id\":"); d = substr(line, p + 5)
        id = substr(d, 1, index(d, ",") - 1)
        g = ""; p = index(line, "\"digest\":\"sha256:")
        if (p) { dg = substr(line, p + 17); g = substr(dg, 1, index(dg, "\"") - 1) }
        sz = ""; p = index(line, "\"size_in_bytes\":")
        if (p) { z = substr(line, p + 16); sz = substr(z, 1, index(z, ",") - 1); gsub(/[^0-9]/, "", sz) }
        print id, g, sz
        exit
      } }'
}
# id of the run named "ci" in a workflow-runs listing (newest first)
pick_run() {
  awk '
    { gsub(/\},\{/, "}\n{"); n = split($0, rec, "\n")
      for (i = 1; i <= n; i++) {
        if (index(rec[i], "\"name\":\"ci\"") == 0) continue
        line = rec[i]
        p = index(line, "\"id\":"); d = substr(line, p + 5)
        print substr(d, 1, index(d, ",") - 1)
        exit
      } }'
}

# --- resolve the PR and its CI run -------------------------------------------------
# The preview is the ci run of the PR's head commit; pull_request-event runs
# live in the base repo even for fork PRs, which is exactly why their
# artifacts are downloadable by anyone with a GitHub login.
step "resolving PR #$PR"
if ! pr_json=$(api_get "pulls/$PR") || [ -z "$pr_json" ]; then
  die "could not read PR #$PR — is it open on github.com/$REPO?"
fi
head_sha=$(printf '%s' "$pr_json" | json_compact | pr_head_sha)
[ -n "$head_sha" ] || die "could not parse the head commit of PR #$PR"
short_sha=$(printf '%.7s' "$head_sha")

# a specific commit was requested: accept it as a full sha or a short prefix
if [ -n "$WANT_SHA" ]; then
  case "$head_sha" in
    "$WANT_SHA"*) ;;
    *) die "PR #$PR's latest build is $short_sha, not $WANT_SHA — drop the argument, or update it after pushing" ;;
  esac
fi

if ! runs_json=$(api_get "actions/runs?event=pull_request&head_sha=$head_sha") || [ -z "$runs_json" ]; then
  die "could not list CI runs for $short_sha"
fi
run_id=$(printf '%s' "$runs_json" | json_compact | pick_run)
[ -n "$run_id" ] || die "no ci run found for $short_sha — the PR's checks may not have finished; open https://github.com/$REPO/pull/$PR/checks"
run_url="https://github.com/$REPO/actions/runs/$run_id"

printf '\n%s  apkscope PR preview%s\n\n' "$bold" "$reset"
field "PR"          "#$PR (https://github.com/$REPO/pull/$PR)"
field "commit"      "$short_sha"
field "run"         "$run_url"
field "artifact"    "$artifact"
field "destination" "$(homepath "$DEST")/apkscope-pr-$PR"
printf '\n'

# --- get the artifact zip ------------------------------------------------------------
if [ -n "$ZIP" ]; then
  # a zip downloaded by hand from the PR's Checks tab (no login on this machine)
  [ -f "$ZIP" ] || die "APKSCOPE_PREVIEW_ZIP points at '$ZIP', which is not a file"
  step "using $ZIP"
  cp "$ZIP" "$tmp/preview.zip"
  want_digest=""
else
  if ! arts_json=$(api_get "actions/runs/$run_id/artifacts?per_page=100") || [ -z "$arts_json" ]; then
    die "could not list artifacts of run $run_id"
  fi
  art_line=$(printf '%s' "$arts_json" | json_compact | pick_artifact "$artifact")
  if [ -z "$art_line" ]; then
    warn "artifact '$artifact' not found on run $run_id; available:"
    printf '%s' "$arts_json" | json_compact | awk -F'"name":"' 'NF > 1 { split($2, a, "\""); print "    " a[1] }' >&2
    die "no preview build for this platform (macOS arm64 is an optional CI leg and can be missing)"
  fi
  art_id=$(printf '%s' "$art_line" | cut -d' ' -f1)
  want_digest=$(printf '%s' "$art_line" | cut -d' ' -f2)
  art_size=$(printf '%s' "$art_line" | cut -d' ' -f3)

  step "downloading $artifact${art_size:+ ($(hsize "$art_size"))}"
  if ! api_download "actions/artifacts/$art_id/zip" "$tmp/preview.zip"; then
    cat >&2 <<EOF
error: the artifact download needs a GitHub login
  - gh auth login   (then rerun), or
  - set GITHUB_TOKEN, or
  - download the zip by hand (logged-in browser): $run_url
    Checks tab -> $artifact, then:
    APKSCOPE_PREVIEW_ZIP=/path/to/$artifact.zip ./scripts/preview-pr.sh $PR
EOF
    exit 1
  fi
fi
size=$(wc -c < "$tmp/preview.zip" | tr -cd '0-9')
ok "downloaded ($(hsize "$size"))"

# --- verify the artifact digest --------------------------------------------------------
# GitHub reports the SHA-256 of the artifact zip itself; verify what we
# received against it, like install.sh verifies checksums.txt.
if [ -n "$want_digest" ]; then
  if command -v sha256sum >/dev/null 2>&1; then
    got=$(sha256sum "$tmp/preview.zip" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    got=$(shasum -a 256 "$tmp/preview.zip" | awk '{print $1}')
  else
    got=""
    warn "no sha256 tool found, skipping digest verification"
  fi
  if [ -n "$got" ]; then
    if [ "$got" != "$want_digest" ]; then
      die "SHA-256 mismatch for $artifact
  expected $want_digest
  got      $got"
    fi
    ok "digest verified (sha256, GitHub's artifact digest)"
  fi
else
  warn "hand-provided zip: digest not verified (fetch it from the run's Checks tab, not from anywhere else)"
fi

# --- unpack ---------------------------------------------------------------------------
# The artifact zip holds the bare binary (apkscope / apkscope.exe).
step "unpacking"
if command -v unzip >/dev/null 2>&1; then
  unzip -oq "$tmp/preview.zip" -d "$tmp/unpacked" || die "could not unzip the artifact"
elif command -v ditto >/dev/null 2>&1; then
  ditto -x -k "$tmp/preview.zip" "$tmp/unpacked" || die "could not unzip the artifact"
elif command -v python3 >/dev/null 2>&1; then
  python3 -c "import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" \
    "$tmp/preview.zip" "$tmp/unpacked" || die "could not unzip the artifact"
else
  die "need unzip, ditto or python3 to unpack the artifact"
fi
bin="$tmp/unpacked/apkscope"
[ -f "$bin" ] || bin="$tmp/unpacked/apkscope.exe"
[ -f "$bin" ] || die "the artifact zip did not contain a binary (files: $(ls "$tmp/unpacked" | tr '\n' ' '))"

# --- install side-by-side ---------------------------------------------------------------
step "installing side-by-side"
name="apkscope-pr-$PR"
mkdir -p "$DEST"
if ! cp "$bin" "$DEST/$name" 2>/dev/null; then
  die "cannot write to $DEST (try: sudo APKSCOPE_INSTALL_DIR=$DEST ./scripts/preview-pr.sh $PR, or set APKSCOPE_INSTALL_DIR to a directory you own)"
fi
chmod 0755 "$DEST/$name"
ok "installed $(homepath "$DEST")/$name"

case ":$PATH:" in
  *":$DEST:"*) ok "PATH already includes $(homepath "$DEST")" ;;
  *) warn "$(homepath "$DEST") is not on your PATH; add it or call the preview by full path" ;;
esac

printf '\n'
"$DEST/$name" --version
printf '\n'
note "this preview is UNREVIEWED PR code, not a release. It replaced nothing: the real apkscope is untouched, and 'apkscope upgrade' still only offers stable and canary. Remove the preview with: rm \"$(homepath "$DEST")/$name\""
