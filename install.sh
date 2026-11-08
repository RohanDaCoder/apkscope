#!/bin/sh
# apkscope installer for Linux and macOS.
#
# Downloads the right binary from GitHub Releases, verifies its SHA-256 and
# installs it to a bin directory on your PATH.
#
# Usage:
#   ./install.sh                 # latest version
#   ./install.sh 0.9.1           # specific version
#
# Environment overrides (for forks, mirrors, custom locations):
#   APKSCOPE_REPO=suffixing/apkscope        # GitHub owner/name to download from
#   APKSCOPE_BASE_URL=https://github.com    # mirror / GitHub Enterprise host
#   APKSCOPE_INSTALL_DIR=/usr/local/bin     # where to put the binary
set -eu

REPO="${APKSCOPE_REPO:-suffixing/apkscope}"
BASE="${APKSCOPE_BASE_URL:-https://github.com}"
DEST="${APKSCOPE_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${1:-}"

usage() {
  sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
  exit 1
}

case "${1:-}" in
  -h|--help) usage ;;
esac

# --- detect platform ---------------------------------------------------------
os=$(uname -s)
case "$os" in
  Linux)  os=linux ;;
  Darwin) os=macos ;;
  *) echo "error: unsupported OS '$os' (this installer covers Linux and macOS;" \
       "on Windows use install.ps1 or grab apkscope.exe from the Releases page)" >&2
     exit 1 ;;
esac

arch=$(uname -m)
case "$arch" in
  x86_64|amd64)   arch=x86_64 ;;
  aarch64|arm64)  arch=aarch64 ;;
  *) echo "error: unsupported CPU architecture '$arch'" >&2; exit 1 ;;
esac

asset="apkscope_${os}_${arch}.tar.gz"

# --- resolve the download URL ------------------------------------------------
if [ -n "$VERSION" ]; then
  case "$VERSION" in v*) ;; *) VERSION="v$VERSION" ;; esac   # accept 0.9.1 or v0.9.1
  base_url="$BASE/$REPO/releases/download/$VERSION"
  what="apkscope $VERSION"
else
  base_url="$BASE/$REPO/releases/latest/download"
  what="apkscope (latest)"
fi

# --- downloader --------------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q "$1" -O "$2"; }
else
  echo "error: need curl or wget to download" >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $what for $os/$arch ..."
if ! fetch "$base_url/$asset" "$tmp/$asset"; then
  echo "error: could not download $asset" >&2
  echo "       (no release for this platform yet? check $BASE/$REPO/releases)" >&2
  exit 1
fi

# --- verify checksum (best effort: warn, don't block, if checksums absent) ---
if fetch "$base_url/checksums.txt" "$tmp/checksums.txt"; then
  want=$(awk -v f="$asset" '$0 ~ f"$" {print $1; exit}' "$tmp/checksums.txt")
  if [ -n "$want" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      got=$(sha256sum "$tmp/$asset" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
      got=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
    else
      got=""
      echo "warning: no sha256 tool found, skipping checksum verification"
    fi
    if [ -n "$got" ]; then
      if [ "$got" != "$want" ]; then
        echo "error: SHA-256 mismatch for $asset" >&2
        echo "  expected $want" >&2
        echo "  got      $got" >&2
        exit 1
      fi
      echo "Checksum OK ($want)"
    fi
  fi
else
  echo "warning: checksums.txt not found for this release, skipping verification"
fi

# --- unpack and install --------------------------------------------------------
tar -xzf "$tmp/$asset" -C "$tmp"
if [ ! -f "$tmp/apkscope" ]; then
  echo "error: $asset did not contain an 'apkscope' binary" >&2
  exit 1
fi

mkdir -p "$DEST"
if ! cp "$tmp/apkscope" "$DEST/apkscope" 2>/dev/null; then
  echo "error: cannot write to $DEST (try: sudo APKSCOPE_INSTALL_DIR=$DEST ./install.sh," \
       "or set APKSCOPE_INSTALL_DIR to a directory you own)" >&2
  exit 1
fi
chmod 0755 "$DEST/apkscope"

# --- report -------------------------------------------------------------------
case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "note: $DEST is not on your PATH; add it with:"
     echo "      echo 'export PATH=\"$DEST:\$PATH\"' >> ~/.profile && source ~/.profile" ;;
esac

echo "Installed: $DEST/apkscope"
"$DEST/apkscope" --version
