# socai agent notes

The repo has a Rust core (`core/`), a Rust CLI (`cli/`), and a Tauri 2 desktop
app (`app/`). The Rust core is the active shared implementation for
CLI/TUI/Tauri.

Build, run, local-dev workflows, and the reference-docs index live in
[DEVELOPMENT.md](./DEVELOPMENT.md). The [README](./README.md) is user-facing
only (CLI install + usage, desktop download); keep developer material out of it
and in DEVELOPMENT.md instead.

## Engineering rules

- **Do NOT add new tests to Rust code unless the user explicitly asks.** This
  applies even when you add a new function or change behavior — ship the change
  without a test. It is fine (and expected) to *update* an existing test when you
  change an API it already covers, but do not create new `#[test]` functions or
  grow `mod tests` on your own initiative.

## Rust core — `core/`

- `core/src/agent/`: generic agent loop, LLM providers, run state, tool trait.
- `core/src/cdp/`: CDP endpoint discovery, connection lifecycle, tab sessions,
  and page factories.
- `core/src/media/`: optional media enrichment helpers.
- `core/src/runtime/`: shared in-process runtime handle used by each entrypoint.
- `core/src/sites/xhs/`: Xiaohongshu entities, JS extractors, page runtime,
  and site tools.

## Rust CLI — `cli/`

Entry point package for the `socai` binary. It depends
on `socai-core`; keep CLI daemon/socket plumbing thin and keep browser/session
ownership inside the core runtime.

Rules:

- Keep browser/session ownership inside `core/src/runtime/` and
  `core/src/cdp/`; CLI daemon/socket plumbing should stay thin.
- Keep JS extractors in a small JSON-returning contract; Rust injects, calls,
  and validates results.
- Tool subcommands wrap existing `XhsPageRuntime` / site tools — don't
  duplicate XHS logic in the daemon. Any cleanups to the public data shape go
  in `core/src/sites/xhs/entities.rs` and `core/src/sites/xhs/tools.rs`, not in
  the daemon layer.

## Desktop app — `app/`

Stack: Tauri 2.11 (Rust shell) + Vite 6 + vanilla TypeScript (no UI framework).
Bundle identifier `com.socai.app`. Product name lowercase `socai`.

Layout:

- `app/src/`: frontend — `main.ts`, `styles.css`, `assets/`.
- `app/src-tauri/`: Rust shell — `lib.rs`, `tauri.conf.json`, `capabilities/`, `icons/`.
- `app/branding/`: icon source-of-truth — `app-icon.svg` + rasterized `app-icon.png`.

Dev and build (run from `app/`):

```bash
pnpm install                          # one-time
pnpm exec tauri dev                   # daily dev loop (Vite HMR + Rust hot recompile)
pnpm exec tauri build --bundles app   # → target/release/bundle/macos/socai.app
```

Rules:

- **Brand is always lowercase `socai`** — productName, window title, hero text,
  error strings, comments. No Title Case anywhere.
- **Design system is monochrome.** Use tokens from `app/src/styles.css`
  (`--ink-0..9`, `--canvas`, `--fg`, `--line`, etc.). **No accent colors.**
  Status is filled vs hollow, never hue.
- **Hairlines, not shadows.** `--line` (#e5e5e5) carries all structural
  separation. `--shadow-pop` is reserved for popovers only.
- **Use the type-scale classes** — `.t-display`, `.t-h1`, `.t-h2`, `.t-h3`,
  `.t-lede`, `.t-body`, `.t-small`, `.t-eyebrow` (mono uppercase), `.t-mono`.
  Don't reinvent.
- **`tauri` (Rust) and `@tauri-apps/api` (npm) must share major/minor.**
  Bumping one requires bumping the other in the same commit; Tauri CLI hard-
  fails on minor drift.

Regenerating the app icon — edit `branding/app-icon.svg`, then from `app/`:

```bash
rsvg-convert branding/app-icon.svg -w 1024 -h 1024 -o branding/app-icon.png
pnpm exec tauri icon branding/app-icon.png
```

Both steps are deterministic. Commit the changed files in `src-tauri/icons/`.
The mobile / Microsoft Store fan-out emitted by `tauri icon` is gitignored —
regenerate on demand if a mobile target is ever added.

Gotchas:

- macOS LaunchServices caches icons aggressively. After a rebuild that changes
  the icon, run `killall Dock; killall Finder` to flush. If the bundle
  identifier or productName changed, also run
  `/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -kill -r -domain local -domain system -domain user`.
- Fonts (General Sans, Geist Mono) load via Fontshare / Google Fonts CDNs at
  runtime. For offline-capable production builds, bundle `.woff2` and replace
  the two `@import` rules at the top of `app/src/styles.css`.
- The Vite dev server ignores `src-tauri/**` (see `vite.config.ts`) so Rust
  file changes don't cause spurious frontend reloads. Rust edits trigger a
  full Tauri shell restart instead.

## WeChat group QR maintenance

The WeChat group QR lives in two places that must stay in sync:
`docs/assets/wechat-group-qr.jpg` (shown in the README) and
`site/public/wechat-group-qr.jpg` (served on the site's `/contact` page). They
are byte-identical copies. WeChat group QR codes expire after 7 days and can't
be fetched via any API — the user must re-export it manually from WeChat on
their phone.

A `SessionStart` hook in `.claude/settings.json` checks the file's last git
commit date and, if ≥6 days old, injects a `[wechat-qr-reminder]`. On seeing it,
remind the user at the start of your reply.

To update: ask the user for the freshly exported image, overwrite **both**
`docs/assets/wechat-group-qr.jpg` and `site/public/wechat-group-qr.jpg` (same
names/paths — README and `/contact` page need no change), then commit
(`docs: refresh wechat group QR`) and push to `main`.

This QR refresh is the only case where committing and pushing to `main` is
pre-authorized without per-time confirmation; everything else still follows the
default commit/push-only-when-asked rule.
