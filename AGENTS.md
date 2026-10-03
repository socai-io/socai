# socai agent notes

The repo has a Rust core (`core/`), a Rust CLI (`cli/`), and a Tauri 2 desktop
app (`app/`). The Rust core is the active shared implementation for
CLI/TUI/Tauri.

Build, run, local-dev workflows, and the reference-docs index live in
[DEVELOPMENT.md](./DEVELOPMENT.md). The [README](./README.md) is the user-facing
product pitch. Install, commands, and browser setup live in
[docs/guide.md](./docs/guide.md). Keep developer material in DEVELOPMENT.md.

## Engineering rules

- **Do NOT add new tests to Rust code unless the user explicitly asks.** This
  applies even when you add a new function or change behavior — ship the change
  without a test. It is fine (and expected) to *update* an existing test when you
  change an API it already covers, but do not create new `#[test]` functions or
  grow `mod tests` on your own initiative.
- **Add new features and new platforms by following [rsi](./skills/rsi/SKILL.md).**
  When adding a capability or supporting another site, follow that skill's
  instructions.

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
Bundle identifier `com.socai.app`.

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

## WorkBuddy ecosystem packages — `plugins/workbuddy/`

Skill and expert packages published to the WorkBuddy / CodeBuddy marketplace.
Layout, packaging, and upload paths are documented in
[`plugins/workbuddy/README.md`](./plugins/workbuddy/README.md).

Rules:

- **The Xiaohongshu skill source of truth is `plugins/workbuddy/xiaohongshu-socai/`.**
  `xiaohongshu-research-expert/skills/` is a build artifact injected by
  `build.sh` — never edit it by hand. The multi-platform WorkBuddy skill has
  its own source at `plugins/workbuddy/socai-social-research/`.
- **Run `plugins/workbuddy/build.sh` after any skill change.** It validates
  frontmatter / `plugin.json` against the open-platform spec before zipping, so
  a spec violation fails the build instead of failing upload. Three zips are
  produced: the Xiaohongshu skill, the multi-platform skill, and the expert
  (with the Xiaohongshu skill inlined).
- Expert display copy has hard constraints the validator enforces:
  `displayDescription.zh` must be 40–50 characters, `tags` and `quickPrompts`
  must each have exactly 3 entries, `defaultInitPrompt` must equal
  `quickPrompts[0]`, and the avatar must be ≤500KB.
- Keep the expert's product category at `05-MarketingGrowth`. Do not move it to
  `02-Engineering` — WorkBuddy's audience is office knowledge workers, and that
  category gets no traffic.

## WeChat group QR maintenance

The WeChat group QR lives in two places that must stay in sync:
`docs/assets/wechat-group-qr.jpg` (shown in the README) and
`site/public/wechat-group-qr.jpg` (served on the site's `/contact` page). They
are byte-identical copies. WeChat group QR codes expire after 7 days and can't
be fetched via any API — the user must re-export it manually from WeChat on
their phone.

A `sessionStart` hook in `.cursor/hooks.json` (and the Claude Code
`SessionStart` hook in `.claude/settings.json`) checks the file's last git
commit date and, if ≥6 days old, injects a `[wechat-qr-reminder]`. On seeing it,
remind the user at the start of your reply.

To update: ask the user for the freshly exported image, overwrite **both**
`docs/assets/wechat-group-qr.jpg` and `site/public/wechat-group-qr.jpg` (same
names/paths — README and `/contact` page need no change), then commit
(`docs: refresh wechat group QR`) and push to `main`.

This QR refresh is the only case where committing and pushing to `main` is
pre-authorized without per-time confirmation; everything else still follows the
default commit/push-only-when-asked rule.
