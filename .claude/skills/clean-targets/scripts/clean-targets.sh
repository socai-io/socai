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

# Human-readable size from a KB count (shared by per-target and total lines).
human() {
  awk "BEGIN { kb = $1; if (kb >= 1048576) printf \"%.1f GB\", kb / 1048576; else if (kb >= 1024) printf \"%.0f MB\", kb / 1024; else printf \"%d KB\", kb }"
}

# One process snapshot up front; a target path appearing anywhere in a command
# line (rustc --out-dir, a binary running from target/debug, ...) means in use.
# Fail closed: without a snapshot the in-use check is blind, and deleting a
# target mid-build is the one thing this script must never do. `set -e`
# aborts if ps itself fails; the emptiness guard catches a silent no-op.
PROCS=$(ps -axo command)
if [ -z "$PROCS" ]; then
  echo "error: empty process snapshot; refusing to delete without in-use protection" >&2
  exit 1
fi

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
  size_kb=$(du -sk "$t" 2>/dev/null | cut -f1 || true)
  if [ -z "$size_kb" ]; then
    continue  # vanished between discovery and now (e.g. a concurrent clean)
  fi
  size_h=$(human "$size_kb")
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
freed_h=$(human "$freed_kb")
if [ "$DRY_RUN" -eq 1 ]; then
  echo "dry run: would free ${freed_h} (${skipped} in use, kept)"
else
  echo "freed ${freed_h} (${skipped} in use, kept)"
fi
