#!/usr/bin/env bash
# clean-targets.sh — remove cargo target/ build dirs across every checkout of this repo.
#
# Checkouts (main + all worktrees) are discovered via `git worktree list`, so
# nothing is hardcoded to one machine's directory layout. Cargo target dirs are
# identified by the CACHEDIR.TAG marker cargo writes into each one — safer than
# matching on the name "target" alone. Any target referenced by a running
# process (an active rustc/cargo build, a dev binary launched from
# target/debug, a `tauri dev` session) is skipped, because deleting it
# mid-build corrupts the build or kills the session.
#
# Usage: clean-targets.sh [--dry-run|-n]

set -euo pipefail

DRY_RUN=0
if [ "${1:-}" = "--dry-run" ] || [ "${1:-}" = "-n" ]; then
  DRY_RUN=1
fi

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "error: run from inside a git checkout of the repo" >&2
  exit 1
fi

# One process snapshot up front; a target path appearing anywhere in a command
# line (rustc --out-dir, a binary running from target/debug, ...) means in use.
PROCS=$(ps -axo command || true)

# Collect candidate target dirs from every checkout. maxdepth 4 reaches
# nested workspaces (e.g. app/src-tauri/target) without descending into
# worktrees-inside-worktrees, which are enumerated on their own. Deduped in
# case layouts overlap anyway.
TARGETS=$(
  git worktree list --porcelain | sed -n 's/^worktree //p' | while IFS= read -r wt; do
    [ -d "$wt" ] || continue
    find "$wt" -maxdepth 4 -path '*/target/CACHEDIR.TAG' 2>/dev/null || true
  done | sed 's|/CACHEDIR.TAG$||' | sort -u
)

if [ -z "$TARGETS" ]; then
  echo "no cargo target directories found — nothing to clean"
  exit 0
fi

freed_kb=0
skipped=0
while IFS= read -r t; do
  size_kb=$(du -sk "$t" 2>/dev/null | cut -f1)
  size_h=$(du -sh "$t" 2>/dev/null | cut -f1)
  if printf '%s\n' "$PROCS" | grep -qF "$t"; then
    echo "skip (in use)   ${size_h}  ${t}"
    skipped=$((skipped + 1))
    continue
  fi
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "would delete    ${size_h}  ${t}"
  else
    rm -rf "$t"
    echo "deleted         ${size_h}  ${t}"
  fi
  freed_kb=$((freed_kb + size_kb))
done <<< "$TARGETS"

echo "--"
freed_h=$(awk "BEGIN { kb = ${freed_kb}; if (kb >= 1048576) printf \"%.1f GB\", kb / 1048576; else printf \"%.0f MB\", kb / 1024 }")
if [ "$DRY_RUN" -eq 1 ]; then
  echo "dry run: would free ${freed_h} (${skipped} in use, kept)"
else
  echo "freed ${freed_h} (${skipped} in use, kept)"
fi
