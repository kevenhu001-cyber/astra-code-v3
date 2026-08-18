#!/bin/sh
#
# Astra CLI uninstaller (Linux / macOS).
#
#   curl -fsSL https://astracode.topodrive.top/install/uninstall.sh | sh
#
# Flags:
#   --purge / -p    Also remove ~/.astra (config, sessions, caches).
#   --dry-run       Print what would be removed without changing anything.
#
# Default behaviour is safe / non-destructive:
#   - Removes the binary from every known install location.
#   - Cleans PATH entries the installer added in ~/.bashrc, ~/.zshrc, ~/.profile.
#   - Leaves ~/.astra (config, sessions) intact.
#
# Exit status is non-zero only if at least one step failed.
set -eu
DRY_RUN=0
PURGE=0
for arg in "$@"; do
  case "$arg" in
    --purge|-p) PURGE=1 ;;
    --dry-run|-n) DRY_RUN=1 ;;
    -h|--help)
      cat <<'USAGE'
Usage: uninstall.sh [--purge] [--dry-run]
  --purge, -p   Also remove the ~/.astra directory (config + sessions).
  --dry-run, -n Print what would be removed without doing it.
By default this only removes the binary and the installer-managed PATH
entries it added to your shell rc files. ~/.astra is preserved.
USAGE
      exit 0
      ;;
    *)
      echo "error: unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done
# Begin the summary counters; we only print a final report after work runs.
removed=0
preserved=0
skipped=0
failed=0
# --- helpers --------------------------------------------------------------
run_rm() {
  target="$1"
  if [ -e "$target" ] || [ -L "$target" ]; then
    if [ "$DRY_RUN" = 1 ]; then
      echo "[dry-run] remove $target"
    else
      rm -f "$target" || { echo "warning: failed to remove $target" >&2; failed=$((failed + 1)); return 1; }
      echo "removed $target"
    fi
    removed=$((removed + 1))
  else
    skipped=$((skipped + 1))
  fi
}
run_rm_dir() {
  target="$1"
  if [ -d "$target" ] || [ -L "$target" ]; then
    if [ "$DRY_RUN" = 1 ]; then
      echo "[dry-run] remove $target"
    else
      rm -rf "$target" || { echo "warning: failed to remove $target" >&2; failed=$((failed + 1)); return 1; }
      echo "removed $target"
    fi
    removed=$((removed + 1))
  else
    skipped=$((skipped + 1))
  fi
}
run_rmdir_parent() {
  target="$1"
  if [ -d "$target" ] && [ -z "$(ls -A "$target" 2>/dev/null || true)" ]; then
    if [ "$DRY_RUN" = 1 ]; then
      echo "[dry-run] remove empty dir $target"
    else
      rmdir "$target" 2>/dev/null || true
      echo "removed empty dir $target"
    fi
  fi
}
# --- 1. known binary locations --------------------------------------------
#
# These match the paths the installer writes to (see site/install/install.sh).
# We try them in order. A common manual install puts the binary in
# ~/.local/bin/astra or /usr/local/bin/astra.
INSTALL_DIR="${ASTRA_INSTALL_DIR:-$HOME/.local/bin}"
HOME_BIN="$HOME/.local/bin/astra"
USR_LOCAL_BIN="/usr/local/bin/astra"
run_rm "$INSTALL_DIR/astra"
run_rm "$HOME_BIN"
run_rm "$USR_LOCAL_BIN"
# As a last resort, locate any stray `astra` binaries on PATH that the user
# could be executing, and remove the ones we are confident about.
# Search narrowly so we don't kill unrelated `astra*` tooling.
if command -v command >/dev/null 2>&1; then
  discovered="$(command -v astra 2>/dev/null || true)"
  case "$discovered" in
    "$INSTALL_DIR/astra") ;;
    "$HOME_BIN") ;;
    "$USR_LOCAL_BIN") ;;
    "") ;;
    *) echo "note: another 'astra' is on PATH at $discovered (left untouched)" ;;
  esac
fi
# Best-effort cleanup: drop the now-empty install directory.
run_rmdir_parent "$INSTALL_DIR"
# --- 2. installer-managed PATH entries in shell rc files ------------------
#
# `install.sh` writes a block with the marker comment `# added by astra
# installer`. We strip the whole block (comment + export line) plus any
# leading blank lines, idempotently.
PATH_MARKER='# added by astra installer'
RC_FILES="$HOME/.bashrc $HOME/.zshrc $HOME/.profile"
for rc in $RC_FILES; do
  [ -f "$rc" ] || continue
  # Skip rc files that have no astra marker at all.
  if ! grep -Fq "$PATH_MARKER" "$rc"; then
    continue
  fi
  if [ "$DRY_RUN" = 1 ]; then
    if grep -Fq "$PATH_MARKER" "$rc"; then
      echo "[dry-run] clean $rc (remove astra installer block)"
      removed=$((removed + 1))
    fi
    continue
  fi
  tmp="$(mktemp)" || continue
  # Remove the entire installer block: the marker line + the `export PATH=...`
  # line beneath it, plus any leading blank line above the marker. Repeat in a
  # loop until no more matches (handles multiple stale blocks).
  cp "$rc" "$tmp"
  changed=1
  while [ "$changed" -eq 1 ]; do
    changed=0
    if grep -Fq "$PATH_MARKER" "$tmp"; then
      awk -v marker="$PATH_MARKER" '
        BEGIN { in_block = 0; drop_blank = 0 }
        {
          if (in_block) {
            # The export line directly under the marker (next non-empty line).
            if ($0 ~ /^export[[:space:]]+PATH=/) {
              in_block = 0;
              drop_blank = 1;
              next;
            }
            # Defensive: any extra non-export line ends the block anyway.
            in_block = 0;
            drop_blank = 1;
          }
          if (index($0, marker) > 0) {
            in_block = 1;
            next;
          }
          if (drop_blank && $0 == "") {
            drop_blank = 0;
            next;
          }
          drop_blank = 0;
          print;
        }
      ' "$tmp" > "$tmp.new" && mv "$tmp.new" "$tmp"
      changed=1
      continue
    fi
  done
  if ! cmp -s "$rc" "$tmp"; then
    mv "$tmp" "$rc"
    echo "cleaned $rc (removed astra installer PATH block)"
    removed=$((removed + 1))
  else
    rm -f "$tmp"
    skipped=$((skipped + 1))
  fi
done
# --- 3. config / data directory -------------------------------------------
#
# Default behaviour: leave ~/.astra alone. Only remove it on --purge, so we
# never delete a user's config / sessions by accident.
case "${ASTRA_HOME:-}" in
  "") ASTRA_HOME_DIR="$HOME/.astra" ;;
  *)  ASTRA_HOME_DIR="$ASTRA_HOME" ;;
esac
if [ "$PURGE" = 1 ]; then
  if [ -d "$ASTRA_HOME_DIR" ] || [ -L "$ASTRA_HOME_DIR" ]; then
    # Belt and suspenders: refuse to delete if the resolved path is empty or
    # equals $HOME itself, which would mean ASTRA_HOME is unset and we somehow
    # fell back to a dangerous target.
    case "$ASTRA_HOME_DIR" in
      ""|"$HOME"|"/"|".")
        echo "refusing to purge unsafe path: '$ASTRA_HOME_DIR'" >&2
        failed=$((failed + 1))
        ;;
      *)
        run_rm_dir "$ASTRA_HOME_DIR"
        ;;
    esac
  else
    skipped=$((skipped + 1))
  fi
else
  if [ -d "$ASTRA_HOME_DIR" ] || [ -L "$ASTRA_HOME_DIR" ]; then
    echo "preserved $ASTRA_HOME_DIR (use --purge to remove)"
    preserved=$((preserved + 1))
  else
    skipped=$((skipped + 1))
  fi
fi
# --- 4. summary -----------------------------------------------------------
echo ""
echo "Astra uninstall summary"
echo "  removed:  $removed"
echo "  preserved: $preserved"
echo "  skipped:  $skipped"
echo "  failed:   $failed"
if [ "$failed" -ne 0 ]; then
  echo "one or more removal steps failed; review the messages above" >&2
  exit 1
fi
if [ "$DRY_RUN" = 1 ]; then
  echo "dry-run mode: nothing was actually removed."
elif [ "$PURGE" = 1 ]; then
  echo "Astra has been removed. ~/.astra was also deleted."
else
  cat <<'NOTE'
Note: your config and sessions were kept in ~/.astra.
To remove them later, re-run with --purge:
  curl -fsSL https://astracode.topodrive.top/install/uninstall.sh | sh -s -- --purge
NOTE
fi
exit 0