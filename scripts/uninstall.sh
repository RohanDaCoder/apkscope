#!/bin/sh
# apkscope uninstaller for Linux and macOS.
#
# Removes the apkscope binary installed by scripts/install.sh and undoes the
# PATH changes it may have made (the "# added by apkscope installer" block in
# ~/.bashrc / ~/.bash_profile / ~/.zshrc, and fish's fish_user_paths).
#
# Usage:
#   ./scripts/uninstall.sh
#
# Environment overrides (must match what you passed to install.sh):
#   APKSCOPE_INSTALL_DIR=/usr/local/bin   # where install.sh put the binary
set -eu

DEST="${APKSCOPE_INSTALL_DIR:-$HOME/.local/bin}"
TARGET="$DEST/apkscope"

# --- output helpers (kept in sync with install.sh; each script stays ----------
# self-contained because curl | sh fetches one file)
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

ok()   { printf '  %s%s%s %s\n' "$green" "$tick" "$reset" "$1"; }
note() { printf '%snote:%s %s\n' "$dim" "$reset" "$1" >&2; }
warn() { printf '%swarning:%s %s\n' "$yellow" "$reset" "$1" >&2; }
die()  { printf '%serror:%s %s\n' "$red" "$reset" "$1" >&2; exit 1; }

homepath() {
  case "$1" in
    "$HOME"?*) printf '~%s' "${1#"$HOME"}" ;;
    *) printf '%s' "$1" ;;
  esac
}

printf '\n%s  apkscope uninstaller%s\n\n' "$bold" "$reset"

if [ -f "$TARGET" ]; then
  if ! rm -f "$TARGET" 2>/dev/null; then
    die "cannot remove $TARGET (try: sudo APKSCOPE_INSTALL_DIR=$DEST ./scripts/uninstall.sh)"
  fi
  ok "removed $(homepath "$TARGET")"
else
  note "$(homepath "$TARGET") was not present (already uninstalled?)"
fi

# --- undo the PATH changes install.sh may have made ---------------------------
# install.sh appends a two-line block ("# added by apkscope installer" + one
# export line) to the rc file of the shell it ran under. We can't know which
# shell that was, so sweep all three rc files; the marker only ever comes from
# us, and only the marker line plus the export line directly under it are
# removed, so user content is never touched.
for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile"; do
  [ -f "$rc" ] && [ -w "$rc" ] || continue
  grep -qF '# added by apkscope installer' "$rc" || continue
  awk '
    $0 == "# added by apkscope installer" { skip = 1; next }
    skip && /^export PATH=/ { skip = 0; next }
    { skip = 0; print }
  ' "$rc" >"$rc.tmp" && cat "$rc.tmp" >"$rc" && rm -f "$rc.tmp"
  ok "PATH line removed from $(homepath "$rc")"
done

# fish: install.sh ran `fish_add_path "$DEST"`, which stores $DEST in the
# universal fish_user_paths variable that all future fish sessions read.
if command -v fish >/dev/null 2>&1; then
  if fish -c "contains -- '$DEST' \$fish_user_paths" 2>/dev/null; then
    if fish -c "set --erase fish_user_paths[(contains -i -- '$DEST' \$fish_user_paths)]" 2>/dev/null; then
      ok "removed $(homepath "$DEST") from fish's fish_user_paths"
    else
      warn "could not edit fish_user_paths; remove $(homepath "$DEST") manually:"
      printf '    %s\n' "fish -c 'set -e fish_user_paths[N]'  # N = its index" >&2
    fi
  fi
fi

printf '\n  Done.\n\n'
