# socai development

Build, run, and maintainer documentation for working **on** socai. It
intentionally lives outside the [README](./README.md): the README stays focused
on what users need to install and run the socai CLI. This file is the entry
point for everything else.

For repo structure, architecture, and the conventions every AI tool must follow,
see [AGENTS.md](./AGENTS.md). This file complements it with local-dev workflows
and an index of the reference docs.

## Local development

### CLI / core

The published install path is documented in the README and prefers the release
CLI binary. For day-to-day iteration, build and run from the workspace instead:

```bash
cargo build                 # build the whole workspace (core + cli)
cargo run -p socai-cli -- topic_scan "运营爆款思路" --num-notes 30
cargo test                  # run the workspace test suite
```

Browser/session ownership lives in `core/src/runtime/` and `core/src/cdp/`; the
CLI daemon/socket plumbing stays thin. See the
[Rust CLI rules in AGENTS.md](./AGENTS.md#rust-cli--cli).

### TUI

Running `socai` with no subcommand opens the terminal UI (same `socai-cli`
binary, backed by `socai-core`).

### Desktop app

Run everything below from `app/`:

```bash
pnpm install                          # one-time
pnpm exec tauri dev                   # daily dev loop (Vite HMR + Rust hot recompile)
pnpm run dev:desktop:local            # dev loop, but write records/artifacts under the repo
pnpm exec tauri build --bundles app   # → target/release/bundle/macos/socai.app
```

`dev:desktop:local` points `SOCAI_HOME` / `SOCAI_RUNS_DIR` at the repo's
`.socai/` directory, so runs and the task index land alongside the checkout:

```text
.socai/app/tasks.json
.socai/runs/<run-dir>/
```

For app build targets, icon regeneration, the Tauri version-pinning rule, the
monochrome design system, and macOS icon-cache gotchas, see the
[Desktop app section in AGENTS.md](./AGENTS.md#desktop-app--app).

### LLM model catalog

The desktop app and TUI read selectable model versions from the generated
catalog at `core/src/agent/model_catalog.generated.json`. The app does **not**
discover models from provider APIs at runtime. Refresh the catalog at maintainer
time instead:

```bash
node scripts/sync-model-catalog.mjs --no-official --write  # pi/fallback only
pnpm --dir app sync-models                                # official APIs if keys exist, else pi/fallback
```

The sync script prefers official provider `/models` APIs, then pi's generated
`@earendil-works/pi-ai` catalog, then socai fallback entries. AI agents should
use the `socai-model-sync` skill for model-list refreshes and validation.

### Website

The marketing/download website lives in `site/` and builds as a static Astro
site. It is separate from the desktop product UI in `app/`.

```bash
cd site
pnpm install
pnpm dev
pnpm build
```

The build output is written to `site/dist/`. Deployment settings are documented
in [Website deployment](docs/website-deployment.md).

## Reference documentation

| Doc | Covers |
| --- | --- |
| [Data model](docs/data-model.md) | Run artifacts, desktop task index, and timeline replay. |
| [CLI telemetry schema](docs/telemetry-schema.md) | Telemetry schema, privacy, and configuration contract for the CLI daemon. |
| [Telemetry runbook](docs/development/telemetry-runbook.md) | Maintainer runbook for operating CLI telemetry. |
| [Release flow](docs/release-flow.md) | GitHub Release workflow, platform build graph, assets, and installer smoke tests. |
| [Website deployment](docs/website-deployment.md) | Vercel deployment runbook for `socai.io`. |
| [Website launch QA](docs/website-launch-qa.md) | Launch checklist used for the `socai.io` rollout. |
| [Browser automation on CDP](docs/browser-automation-evolution.md) | Conceptual map of CDP and how browser-automation frameworks evolved on it. |
