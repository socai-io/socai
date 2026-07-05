---
name: clean-targets
description: Reclaim disk space by deleting cargo target/ build directories across the main checkout and every git worktree of this repo. Use whenever the user wants to clean up build artifacts, free disk space, run "cargo clean" everywhere, prune worktree build caches, or complains that the repo or its worktrees are eating gigabytes of disk. Safely skips any target dir that a running build or dev session (cargo, rustc, tauri dev) is using.
---

# clean-targets

Delete cargo `target/` directories across every checkout of this repo — the
main checkout and all git worktrees — in one pass. Deleting `target/` is
exactly what `cargo clean` does; the difference is this covers every checkout
at once and refuses to touch a target that a live build is writing to.

## How to run

From anywhere inside any checkout of the repo:

```bash
bash "$(git rev-parse --show-toplevel)/.claude/skills/clean-targets/scripts/clean-targets.sh" --dry-run
```

Run with `--dry-run` first and show the user what would be deleted and what is
being kept (and why). If the plan looks as expected, run it again without the
flag to actually delete. Report the total freed at the end, plus current free
disk (`df -h /`).

## What the script does

- Discovers all checkouts with `git worktree list --porcelain` — nothing is
  hardcoded, so it works on any machine and any worktree layout.
- Detects target dirs by cargo's `CACHEDIR.TAG` marker file, not by directory
  name, so unrelated directories that happen to be called `target` are never
  touched.
- Skips any target whose path appears in a running process's command line
  (active `rustc`/`cargo` compile, a dev binary running out of `target/debug`,
  a `tauri dev` session). Tell the user which ones were kept and that they can
  re-run after stopping those sessions.

## Notes

- A skipped "in use" target usually means an active `pnpm exec tauri dev` in
  that worktree. Never work around the skip by killing the process yourself —
  surface it and let the user decide.
- Everything deleted is a rebuildable cache; the next `cargo build` in that
  checkout just starts cold. No source files, git state, or `node_modules` are
  touched.
