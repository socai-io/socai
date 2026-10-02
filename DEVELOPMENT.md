# socai development

Build, run, and maintainer documentation for working **on** socai. It
intentionally lives outside the [README](./README.md). The README is the
product pitch. Install steps, commands, and browser setup live in the
[user guide](docs/guide.md). This file is the entry point for development.

For repo structure, architecture, and the conventions every AI tool must follow,
see [AGENTS.md](./AGENTS.md). This file complements it with local-dev workflows
and an index of the reference docs.

## Local development

### CLI / core

The published install path is documented in the [user guide](docs/guide.md) and
prefers the release CLI binary. For day-to-day iteration, build and run from the workspace instead:

```bash
cargo build                 # build the whole workspace (core + cli)
cargo run -p socai-cli -- xhs search "运营爆款思路" --num-notes 30
cargo test                  # run the workspace test suite
```

Browser/session ownership lives in `core/src/runtime/` and `core/src/cdp/`; the
CLI daemon/socket plumbing stays thin. See the
[Rust CLI rules in AGENTS.md](./AGENTS.md#rust-cli--cli).

Long-running site commands keep stdout machine-readable: the final command
result is the only JSON written there. Interactive progress is transported as
structured core events through the daemon and rendered on stderr. The default
behavior draws English progress bars only when stderr is an interactive
terminal, so non-interactive agents and scripts receive no progress output.
Desktop and TUI agents call core tools directly and continue to receive the
unchanged `ToolResult`. The CLI daemon remains warm for 24 hours after the
last site command so browser-backed clients can reuse it across a full day.

External agents begin a task with `socai task begin "<original user prompt>"`
(or `--context-file <json-path>` / `--context-file -`). Subsequent site commands
use the daemon's current task automatically, until the next successful begin
or daemon restart. Registration shares the site-command queue so an in-flight
command retains its original task through completion. The input contract,
content opt-out, and correlation fields are documented in
[CLI external-agent task context](docs/telemetry-schema.md#cli-external-agent-task-context).
The general external-agent skill is `skills/socai-cli/SKILL.md`. Rebuild the
WorkBuddy packages with `plugins/workbuddy/build.sh` after changing their skill.

### Privacy-safe browser readiness

`socai status --json` reads the current daemon state without starting the
daemon, connecting Chrome, or retrying a failed browser connection. Its
versioned JSON contract separates CLI availability, daemon state, browser
connection state, configured `profile_mode`, nullable `active_profile_mode`,
platform capabilities, and platform login state. This observational command
never probes a website, so login remains `unknown`; platform read commands
surface any observed login gate in their own result.

The status payload deliberately omits WebSocket URLs, debugging ports, browser
versions, cookies, account identifiers, user-data directories, local paths,
and raw connection errors. Browser failures are reduced to stable codes such
as `BROWSER_PERMISSION_REQUIRED`, `BROWSER_ENDPOINT_UNREACHABLE`, and
`REMOTE_SESSION_UNAVAILABLE`, with one safe next step. If no compatible daemon
is reachable, `daemon_compatible` is `false` and `browser_state` is `unknown`
instead of touching Chrome. `daemon_running` separately reports whether any
socai daemon answered the version-exempt local ping.

### TUI

Running `socai` with no subcommand opens the terminal UI (same `socai-cli`
binary, backed by `socai-core`).

### Desktop app

Run everything below from `app/`:

```bash
pnpm install                            # one-time
pnpm exec tauri dev                     # daily dev loop (Vite HMR + Rust hot recompile)
pnpm run dev:desktop:local -- --release # dev loop, but write records/artifacts under the repo
pnpm exec tauri build --bundles app     # → target/release/bundle/macos/socai.app
```

The desktop composer supports Auto or one or more research sources: Xiaohongshu,
Douyin, TikTok, Instagram, LinkedIn, and X. Auto is exclusive and is the new-task
default when no choice was saved; the agent chooses platforms from the request.
The selection is saved per conversation and can change before a follow-up; old
tasks default to Xiaohongshu. The source picker uses the original platform icons
and checkboxes; it starts collapsed with the selected icons and remembers the
new-task default locally. Auto exposes all supported
site tools without pre-navigating to a platform. Explicit selections scope the
site-learning tools and prompt to those sites. This is a research scope control,
not a security sandbox for the agent's local environment tools.

The app connects managed Chrome automatically at startup; runs also reconnect
it on demand. The existing-browser mode retains its remote-debugging and Allow
flow. The managed composer remains usable while Chrome is connecting, since the
run's browser admission performs the same connection and reports failures.

Files in `outputs/` appear as deliverables; automatic `artifacts/` extraction JSON
appears in a separate, initially collapsed intermediate-files section with the
same preview and file actions. HTTP(S) media URLs stay intact when resolving
archived paths, including for older tasks. Instagram CDN image variants are
deduplicated by resource path so refreshed URL signatures do not add slides.

Artifact cards preview on click. Their ellipsis menu reveals the original file
in Finder/Explorer or opens the native Save As dialog to choose a destination
and filename; the same menu is available from the preview header. Source reveal
and saving both reauthorize the current task artifact before reading it.

`dev:desktop:local` keeps app data and runs in the project but shares the normal
managed Chrome profile. `chrome.profile_dir` remains the explicit override;
otherwise `SOCAI_CHROME_PROFILE_DIR` overrides the default profile directory.
The local dev script captures the original `SOCAI_HOME/chrome-profile` (or
`~/.socai/chrome-profile`) before setting the project-local `SOCAI_HOME`. Restart
the dev command to apply this environment change; existing profiles are retained.

Existing post cards are reused for supported archives. Non-Xiaohongshu cards
without media show a text excerpt; X results currently use answer text and
canonical links without archived post cards. Platform access still depends on
the selected browser profile and the site's login and regional restrictions.

Desktop builds bundle the official Feishu `lark-cli` sidecar. The Tauri
pre-build hook runs `pnpm run prepare:lark-cli`, downloads the pinned release,
verifies its published SHA-256, and prepares the target-triple binary under
`app/src-tauri/binaries/` (ignored by git). macOS builds prepare arm64, x86_64,
and universal binaries; Windows builds prepare x64.

Each answer's “导出到飞书” action lets the user choose document or direct
Markdown group export before creating anything. The sidecar keeps one named
profile per connected account (`socai`, `socai-2`, …), so switching the browser
account and connecting again preserves existing accounts. New profiles use
one-click app creation with the socai name, Feishu's default avatar,
description, and a minimal explicit permission preset, followed by user
authorization for document
creation, group listing, and send-as-user. App secrets and user tokens are
stored/refreshed by the official CLI through the OS keychain; its non-secret
profile metadata remains in `~/.lark-cli/config.json`. Later exports reuse that
authorization. To refresh the bundled CLI version, update the version, asset
names, and pinned checksums together in
`app/scripts/prepare-lark-cli.mjs`.

`dev:desktop:local` points `SOCAI_HOME` / `SOCAI_RUNS_DIR` at the repo's
`.socai/` directory, so runs and the task index land alongside the checkout.

On macOS, attaching to your existing Chrome reads its `DevToolsActivePort`
file. If onboarding reports that Chrome data access is blocked, use its
button to open **System Settings → Privacy & Security → Files & Folders**
and enable **Google Chrome** under the responsible app. With `tauri dev`
(including `dev:desktop:local -- --release`), this may be the terminal or
editor that launched it, such as Cursor, rather than the installed socai app.
The installed and development apps do not necessarily share permission grants.
Detection resumes after granting access; restart the development app if needed.
This file-access permission is separate from Automation and from Chrome's
own remote-debugging checkbox and Allow dialog.

For normal CLI usage, the equivalent persistent run-artifact setting is
`socai config set runs.dir <path>`; the environment variable remains the highest
precedence override for local/dev scripts:

```text
.socai/app/tasks.json
.socai/runs/<run-dir>/
```

Persisted execution data has three ownership layers:

- Session/conversation: `~/.socai/sessions/<session-id>/session.json`.
- Agent execution: `<run-dir>/run.json`, exact `llm/` steps, and nested
  `tools/<tool-call>/` records.
- Standalone CLI command: `<run-dir>/tool.json`; the run directory itself is
  the single tool-call record.

See [Persisted execution model](docs/data-model.md) for the exact ownership and
file contracts. No parallel legacy/event/trace format is written.

For app build targets, icon regeneration, the Tauri version-pinning rule, the
monochrome design system, and macOS icon-cache gotchas, see the
[Desktop app section in AGENTS.md](./AGENTS.md#desktop-app--app).

### Google account login

See [Google login setup](docs/google-login.md) for OAuth client creation, backend
configuration, account linking, database rollout and acceptance checks. The
companion private backend is `socai-server`; both it and the desktop app must
include the Google login implementation.

### Stripe sandbox checkout

The desktop supports the private backend's optional Stripe Managed Payments
plan alongside WeChat and Alipay. Its current plan is USD 19/month with
1000 points per paid invoice, hosted browser checkout, automatic renewal, and
end-of-period cancellation. The UI marks sandbox payments explicitly.

Follow `socai-server`'s **Stripe Managed Payments sandbox** instructions to
configure the test key, forward signed webhooks, provision/reuse the product,
and run the isolated local backend. No Stripe secret belongs in this repository.
The server verifies payment and credits the wallet; browser redirects do not.

With Stripe CLI installed and a sandbox key configured, run `pnpm run dev:stripe`
from `app/`. It starts the webhook listener, captures its signing secret without
printing credentials, provisions the sandbox product, prepares an isolated login,
starts the local backend and frontend, builds and opens the desktop. It uses a sibling
`socai-server` checkout by default; `SOCAI_SERVER_DIR` overrides that location.
It refuses occupied ports instead of stopping other running apps.

The launcher reads the optional private `$HOME/.config/socai/stripe/sandbox.env`
file (`SOCAI_STRIPE_ENV_FILE` overrides the path), or the backend's `.env`.
Only a sandbox key is accepted. `STRIPE_CLI_PATH` overrides the CLI executable;
otherwise it uses `stripe` on PATH or the npm installation under
`$HOME/.local/share/socai-stripe-tools`. Closing the app stops its local services.
On macOS it uses a separate `socai sandbox.app` under the ignored `.socai/`
directory so the installed production app and its login remain separate.
The CLI forwards real sandbox events; no Dashboard webhook endpoint is needed
for this local setup. A deployed live backend needs its own HTTPS webhook endpoint.
To try another initial purchase without deleting earlier test records, set a new
`SOCAI_STRIPE_PROFILE` value before launching; each profile has its own local user.

For a manual launch, set these runtime variables:

```bash
export SOCAI_PRO_BASE_URL=http://127.0.0.1:8010
export SOCAI_CLOUD_AUTH_FILE=/absolute/path/to/sandbox/auth.json
export SOCAI_HOME=/absolute/path/to/sandbox/app-data
```

`SOCAI_CLOUD_AUTH_FILE` isolates cloud login credentials; `SOCAI_HOME` alone does
not isolate the existing default cloud auth file. The backend's
`python -m app.scripts.prepare_stripe_sandbox --auth-file ...` command creates a
local test user/session without sending SMS. It only accepts a local Stripe
sandbox SQLite database. Keep both variables set while testing and never point
the sandbox backend at the production database.

### Anonymous first answer

The desktop allows one completed managed-model answer before sign-in. Local
trial credentials and the consumed flag live separately in `~/.socai/guest.json`
(mode 0600); `guest.lock` prevents simultaneous trials across app processes.
Failed/cancelled answers can retry. New questions and follow-ups require sign-in
after completion. In the signed-in account menu, **Use my own LLM API key instead**
reveals the external provider/model/key fields; unchecking returns to the managed model.
An in-flight guest task keeps its guest billing identity if the user signs in.

The API requires `GUEST_TRIAL_ENABLED=true` and `/v1/auth/guest`. Quota is also
checked server-side per installation; this is not a per-person anti-fraud identity.
Guest tokens cannot access account, wallet, payment, or remote browser features.
The backend has configurable issuance and call/cost limits; see its `.env.example`.

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
Catalog entries can also carry per-million-token pricing used for the estimated
cost in `llm/*.response.json`, `run.json`, the CLI summary, and desktop task
metadata. Token counts are provider-reported; cost is an estimate at the
catalog rate, not a provider invoice.

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
| [User guide](docs/guide.md) | Install, interfaces, platform commands, Chrome profiles, and run artifacts. |
| [Data model](docs/data-model.md) | Run artifacts, desktop task index, and timeline replay. |
| [Context window management](docs/context-window-management.md) | Agent turns, tool-result bounds, sawtooth compaction, prompt caching, and artifact evidence retention. |
| [Agent skills and self-healing](docs/agent-skills.md) | Progressive skill loading, constrained local learnings, and the initial self-healing instruction. |
| [CLI telemetry schema](docs/telemetry-schema.md) | Telemetry schema, privacy, and configuration contract for the CLI daemon. |
| [Telemetry runbook](docs/development/telemetry-runbook.md) | Maintainer runbook for operating CLI telemetry. |
| [Release flow](docs/release-flow.md) | GitHub Release workflow, platform build graph, assets, and installer smoke tests. |
| [Website deployment](docs/website-deployment.md) | Vercel deployment runbook for `socai.io`. |
| [Website launch QA](docs/website-launch-qa.md) | Launch checklist used for the `socai.io` rollout. |
| [Browser automation on CDP](docs/browser-automation-evolution.md) | Conceptual map of CDP and how browser-automation frameworks evolved on it. |
