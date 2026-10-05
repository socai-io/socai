# socai guide

Install and use socai, configure browser workflows, and develop or maintain the
project. The [README](../README.md) is the product overview.

- [Quick start](#quick-start)
- [Choose an interface](#choose-an-interface)
- [Desktop workflow](#desktop-workflow)
- [Platform commands](#platform-command-reference)
- [Browser setup, speed, and readiness](#browser-and-login-modes)
- [Run results and artifacts](#run-results-and-artifacts)
- [Local development](#local-development)
- [Reference documentation](#reference-documentation)

Research is read-only by default. Explicit target-bound write commands run only when directly invoked and use durable one-shot receipts to prevent automatic resubmission.

## Quick start

### Desktop app

Use the desktop app to enter research tasks without setting up a command-line environment. It is available for macOS and Windows:

- [Download for macOS](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Download for Windows](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

After installation, follow the in-app steps to connect Chrome and enter a task such as:

> Compare how people discuss sugar-free tea on RedNote, Douyin, and Instagram. Identify recurring purchase criteria and cite specific posts, videos, comments, and replies.

The first connection to your existing Chrome requires enabling remote debugging and confirming the browser permission prompt. See the [Connect Chrome guide](https://socai.io/connect).

See [Desktop workflow](#desktop-workflow) for source selection and Feishu exports,
and [Run results and artifacts](#run-results-and-artifacts) for saved deliverables.

### Command line

The CLI is designed for agent-driven workflows, including Claude Code and Codex, and returns structured data for each platform operation.

macOS:

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows PowerShell:

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'; Invoke-WebRequest -UseBasicParsing https://github.com/socai-io/socai/releases/latest/download/install.ps1 -OutFile $installer; Unblock-File $installer; & $installer
```

The installers download and verify the release archive, install socai at `~/.socai/bin/socai` on macOS or `%USERPROFILE%\.socai\bin\socai.exe` on Windows, and configure or explain the PATH update.

Let a coding agent discover socai from ordinary requests such as “research how people discuss this product on Instagram and X”:

```bash
socai integrate install codex
socai integrate install claude-code
socai integrate install cursor
socai integrate install gemini-cli
socai integrate install kimi-code
socai integrate install qwen-code
socai integrate install trae-code
socai integrate install codebuddy
socai integrate install opencode
socai integrate install github-copilot
# Or install the same portable Skill for every supported local Agent Skills host:
socai integrate install all
socai integrate status --json
```

Use `--scope project` to keep the integration inside the current repository. `claude`, `gemini`, `kimi`, `qwen`, `trae`, and `copilot` are accepted as aliases. The installer writes only the `socai-social-research` Skill directory and refuses to replace different contents unless `--force` is explicit. The repository also exposes the same Skill as the `socai-social-research` plugin for Codex, Claude, Cursor, WorkBuddy/CodeBuddy, Kimi Code, Gemini CLI, Qwen Code, TraeCode, and Agent Plugins-compatible distribution.

Start every new CLI task with `socai task begin`, passing the user’s original question, then run the platform commands. This workflow applies to all platforms:

```bash
socai task begin "Research the gear purchases first-time campers regret across social platforms."
socai xhs search "beginner camping gear mistakes" --num-notes 10 --num-comments 8 --pretty

socai dy search "beginner camping gear" --num 20
socai tiktok search "beginner camping gear" --num 20 --pretty
socai instagram search "beginner camping gear" --num 20 --pretty

socai task begin "Find product designers on LinkedIn."
socai linkedin search "product designer" --type people --num 20 --pretty
```
See the [CLI skill](../skills/socai-cli/SKILL.md) for the complete workflow.

If a prebuilt binary is unavailable for your platform, or you need a source build for development, use Cargo:

```bash
git clone https://github.com/socai-io/socai.git
cd socai
cargo install --path cli --force --locked
cargo install --path asr --force --locked
```

The second command installs the local Whisper helper next to `socai`; it is
required when unpaid or offline transcription routes to the bundled model.

### Terminal interface

After installing the CLI, run `socai` without a subcommand to open the terminal interface:

```bash
socai
```

## Choose an interface

| Interface | Best for | Start with |
| --- | --- | --- |
| Desktop app | Natural-language tasks, task history, and artifact preview or download | Install the macOS or Windows app |
| CLI | Agent calls, scripts, and structured JSON | Start with `socai task begin "<original user question>"`, then run platform commands |
| Terminal interface | Manually running consecutive tasks in a terminal | Run `socai` |

All three interfaces share the same browser connection, site capabilities, and run-record core.

## Desktop workflow

The desktop app keeps task history and artifacts for each research task.

### Research sources

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

### Feishu export

Each answer's “导出到飞书” action lets the user choose document or direct
Markdown group export before creating anything. The sidecar keeps one named
profile per connected account (`socai`, `socai-2`, …), so switching the browser
account and connecting again preserves existing accounts. New profiles use
one-click app creation with the socai name, Feishu's default avatar,
description, and a minimal explicit permission preset, followed by user
authorization for document creation, group listing, and send-as-user. App secrets and user tokens are
stored/refreshed by the official CLI through the OS keychain; its non-secret
profile metadata remains in `~/.lark-cli/config.json`. Later exports reuse that
authorization.

## Supported platforms

Platform capabilities and short commands are in the [README](../README.md#supported-platforms).

Research commands never mutate platform state. The separately documented publish and comment commands require an explicit target and content, verify the signed-in actor and rendered target immediately before dispatch, and never automatically retry an uncertain submit.

## Platform command reference

All examples below are operations within a task already started with `socai task begin "<original user question>"`. Start a new task when the user’s goal changes; do not repeat registration for each example or command.

### RedNote (Xiaohongshu)

#### Search and read posts

```bash
socai xhs search "content marketing ideas" \
  --num-notes 30 \
  --num-comments 20 \
  --filter publish_time=一周内 \
  --filter sort=最多评论 \
  --download-media \
  --ocr \
  --pretty
```

`search` opens result posts and reads their bodies and comments. Add `--preview` to return only result-card metadata such as titles, covers, and engagement counts without opening post details.

#### Read an author and their posts

```bash
socai xhs author <author_id> --num-notes 10 --num-comments 8
```

Return only the author and post-card summaries:

```bash
socai xhs author <author_id> --num-notes 20 --preview
```

#### Read selected posts again

Use the post IDs and `xsec_token` values returned by `search` or `author`:

```bash
socai xhs get-notes \
  --note '<note_id>=<xsec_token>' \
  --note '<note_id>=<xsec_token>' \
  --num-comments 20
```

Post one explicitly requested comment through the existing signed-in browser:

```bash
socai xhs comment '<complete_note_url>' --text 'Exact comment text'
```

#### Common options

| Option | Purpose |
| --- | --- |
| `--num-notes <N>` | Target number of posts; socai scrolls when more results are needed. |
| `--num-comments <N>` | Number of comments and replies per post; use `0` to skip comments. |
| `--preview` | Read only search-result or author-page post cards. |
| `--download-media` | Download images and videos from opened posts and record local paths. |
| `--ocr` | Run local OCR on post images or a video post's cover. |
| `--transcribe-audio` | Download opened videos and transcribe speech; requires signing in and selecting socai agent. |
| `--filter <group=option>` | Apply a RedNote search-page filter; repeat to combine filters. |
| `--pretty` | Pretty-print the final JSON result. |
| `--debug-snapshot` | Save page DOM, accessibility trees, and screenshots for development diagnostics. |

Available filter groups and UI values:

| Group | Values |
| --- | --- |
| `sort` | 综合, 最新, 最多点赞, 最多评论, 最多收藏 |
| `note_type` | 不限, 视频, 图文 |
| `publish_time` | 不限, 一天内, 一周内, 半年内 |
| `search_scope` | 不限, 已看过, 未看过, 已关注 |
| `distance` | 不限, 同城, 附近 |

Filter values mirror the RedNote web interface and should be passed as shown. Multiple filters can be combined:

```bash
socai xhs search "Shanghai weekend activities" \
  --filter publish_time=一周内 \
  --filter note_type=图文 \
  --filter sort=最新
```

### Douyin and TikTok

```bash
socai dy search "coffee" --num 30
socai tiktok search "coffee" --num 30 --pretty
```

Use `socai dy --help` or `socai tiktok --help` for video-detail, author, comment, media-download, and diagnostic commands.

### Instagram

```bash
socai instagram search "coffee" --num 20 --pretty
socai instagram profile nike --num 12
socai instagram get-posts --post https://www.instagram.com/p/<shortcode>/ --num-comments 8
```

Use `socai instagram --help` for profile, post/Reel, comment, and diagnostic commands.

### LinkedIn

```bash
socai linkedin search "product designer" --type people --num 20 --pretty
socai linkedin profile https://www.linkedin.com/in/<id>/
socai linkedin history <id> --section experience
socai linkedin company <company-id>
socai linkedin get-posts --post https://www.linkedin.com/posts/<id> --num-comments 8
socai linkedin comment https://www.linkedin.com/posts/<id> --text 'Exact comment text'
```

Use `socai linkedin --help` for company, relationship, post, comment, and diagnostic commands.

## Browser and login modes

socai supports four Chrome profile modes:

| Mode | Best for | Login behavior |
| --- | --- | --- |
| `existing` | Everyday use; the default | Reuses your existing Chrome and supported-platform logins |
| `managed` | Isolating research from everyday browsing | Uses `~/.socai/chrome-profile`; sign in once |
| `auto` | Automatic connection selection | Tries the managed profile first, then falls back to existing Chrome |
| `remote` | Testing a hosted cloud browser | Beta socai pro capability with session limits |

These settings are stored in `~/.socai/config.json`; the CLI and desktop app read the same configuration.

The app connects managed Chrome automatically at startup; runs also reconnect
it on demand. The existing-browser mode retains its remote-debugging and Allow
flow. The managed composer remains usable while Chrome is connecting, since the
run's browser admission performs the same connection and reports failures.

Switch to an isolated profile:

```bash
socai config set chrome.profile managed
socai stop
```

Set a custom managed profile directory:

```bash
socai config set chrome.profile_dir ~/.socai/profiles/social-research
```

Switch back to your existing Chrome:

```bash
socai config set chrome.profile existing
socai stop
```

`socai stop` is only required when the background daemon is already running; it lets the new setting take effect at the next start.

The hosted browser is currently in beta. Activate socai pro before selecting it:

```bash
socai pro activate <invite_code>
socai config set chrome.profile remote
```

Advanced endpoint overrides remain available through `SOCAI_CDP_WS` and `SOCAI_CDP_URL`.

### Browser action speed

Configure each platform's speed through the CLI or `~/.socai/config.json`:
`instant` (no added delay), `normal` (a random 1–3 seconds), or `slow`
(a random 3–6 seconds). The desktop app applies these preferences without
exposing a speed setting in its UI.

| Platform | Default |
| --- | --- |
| Xiaohongshu, Instagram | `instant` |
| LinkedIn | `slow` |
| Douyin, TikTok, X, other platforms/websites | `normal` |

Preferences apply to all browser sources and CLI/TUI/desktop use without a
browser or daemon restart. Actions use the current page's platform; navigation
uses the destination platform. Each platform has an independent pacing lane.

```bash
socai config set browser.action_speeds.linkedin normal
socai config set browser.action_speeds.xhs slow
socai config unset browser.action_speeds.linkedin  # restore LinkedIn's slow default
```

Platform keys are site-skill IDs (`xhs`, `instagram`, `linkedin`, `dy`, `tiktok`,
`x`, or an installed custom skill ID); `other` controls unmatched websites.
The former global `browser.action_speed` value is ignored in favor of these
platform preferences and defaults.

Clicks, navigation after DOM readiness, scrolls, key presses, and a whole text
entry are paced as actions; individual characters and key-down/key-up pairs do
not each incur the delay. DOM reads and readiness polling have no added delay.
Existing page-readiness waits still apply in every mode.

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

## Run results and artifacts

Each run is written under the following directory by default:

```text
~/.socai/runs/<timestamp>_<task>/
```

Typical contents include:

- final structured results and run metadata
- search, post, and author data
- downloaded images, videos, and OCR output
- a `media_manifest.json` media inventory
- debug snapshots and agent-generated reports, spreadsheets, or other deliverables

Files in `outputs/` appear as deliverables; automatic `artifacts/` extraction JSON
appears in a separate, initially collapsed intermediate-files section with the
same preview and file actions. HTTP(S) media URLs stay intact when resolving
archived paths, including for older tasks. Instagram CDN image variants are
deduplicated by resource path so refreshed URL signatures do not add slides.

Artifact cards preview on click. Their ellipsis menu reveals the original file
in Finder/Explorer or opens the native Save As dialog to choose a destination
and filename; the same menu is available from the preview header. Source reveal
and saving both reauthorize the current task artifact before reading it.

Existing post cards are reused for supported archives. Non-Xiaohongshu cards
without media show a text excerpt; X results currently use answer text and
canonical links without archived post cards. Platform access still depends on
the selected browser profile and the site's login and regional restrictions.

Change the run directory on macOS:

```bash
socai config set runs.dir "$(pwd)/socai-runs"
```

Or in Windows PowerShell:

```powershell
socai config set runs.dir (Join-Path $PWD 'socai-runs')
```

Relative values passed to `runs.dir` are stored as absolute paths from the current directory. `SOCAI_RUNS_DIR` takes precedence when set.

Persisted execution data has three ownership layers:

- Session/conversation: `~/.socai/sessions/<session-id>/session.json`.
- Agent execution: `<run-dir>/run.json`, exact `llm/` steps, and nested
  `tools/<tool-call>/` records.
- Standalone CLI command: `<run-dir>/tool.json`; the run directory itself is
  the single tool-call record.

See [Persisted execution model](data-model.md) for the exact ownership and
file contracts. No parallel legacy/event/trace format is written.

## Local development

Repository structure and engineering conventions live in [AGENTS.md](../AGENTS.md).
Commands below run from the repository root unless a section specifies otherwise.

### CLI and core development

For source installation, see [Command line](#command-line). For day-to-day
iteration, build and run from the repository root:

```bash
cargo build                 # build the whole workspace (core + cli)
cargo run -p socai-cli -- xhs search "运营爆款思路" --num-notes 30
cargo test                  # run the workspace test suite
```

Browser/session ownership lives in `core/src/runtime/` and `core/src/cdp/`; the
CLI daemon/socket plumbing stays thin. See the
[Rust CLI rules in AGENTS.md](../AGENTS.md#rust-cli--cli).

Long-running site commands keep stdout machine-readable: the final command
result is the only JSON written there. Interactive progress is transported as
structured core events through the daemon and rendered on stderr. The default
behavior draws English progress bars only when stderr is an interactive
terminal, so non-interactive agents and scripts receive no progress output.
Desktop and TUI agents call core tools directly and continue to receive the
unchanged `ToolResult`. The CLI daemon remains warm for 24 hours after the
last site command so browser-backed clients can reuse it across a full day.

A site command whose result carries `ok: false` still prints that JSON on
stdout, partial results included. It also writes one
`socai <site> <command>: ok=false (<reason>)` line to stderr and exits with
status 1, so a script can branch on the exit status and read `reason` for the
cause.

The task registration shown in [Command line](#command-line) also accepts
`--context-file <json-path>` or `--context-file -`. Subsequent site commands use
the daemon's current task until the next successful begin or daemon restart.
Registration shares the site-command queue, so an in-flight command retains its
original task through completion. The input contract,
content opt-out, and correlation fields are documented in
[CLI external-agent task context](telemetry-schema.md#cli-external-agent-task-context).
The general external-agent skill is `skills/socai-cli/SKILL.md`. Rebuild the
WorkBuddy packages with `plugins/workbuddy/build.sh` after changing their skill.

### Browser action implementation

Configuration, defaults, and action boundaries are documented under
[Browser action speed](#browser-action-speed). The shared core resolves platforms
through site-skill manifest domains and CDP's committed top-frame URL, including
redirects and SPA changes. Defaults live in `BrowserConfig::action_speed_for`.
A process-wide lane per platform holds the action through its cooldown; dropping
a cancelled action preserves that cooldown.

New DOM browser tools that perform actions must declare `"action": true` in their
manifest. They execute through `PageSession::evaluate_action`; pure readers use
`evaluate_json`. A compound page script must await `socaiAction(() => action)`
for each individual click/scroll, so a batch does not bypass the selected rate.
The helper is scoped to that evaluation. Built-in scripts retain a direct-call
fallback for standalone fixtures. Use the typed `PageSession` methods for trusted
input: they group mouse press/release and whole text entry into single actions.

### Desktop development

Run everything below from `app/`:

```bash
pnpm install                            # one-time
pnpm exec tauri dev                     # daily dev loop (Vite HMR + Rust hot recompile)
pnpm run dev:desktop:local -- --release # dev loop, but write records/artifacts under the repo
pnpm exec tauri build --bundles app     # → target/release/bundle/macos/socai.app
```

`dev:desktop:local` points `SOCAI_HOME` / `SOCAI_RUNS_DIR` at the repo's `.socai/`
directory while sharing the normal managed Chrome profile.
`chrome.profile_dir` remains the explicit override;
otherwise `SOCAI_CHROME_PROFILE_DIR` overrides the default profile directory.
The local dev script captures the original `SOCAI_HOME/chrome-profile` (or
`~/.socai/chrome-profile`) before setting the project-local `SOCAI_HOME`. Restart
the dev command to apply this environment change; existing profiles are retained.

Runs and the task index land alongside the checkout:

```text
.socai/app/tasks.json
.socai/runs/<run-dir>/
```

Desktop builds bundle the official Feishu `lark-cli` sidecar. The Tauri
pre-build hook runs `pnpm run prepare:lark-cli`, downloads the pinned release,
verifies its published SHA-256, and prepares the target-triple binary under
`app/src-tauri/binaries/` (ignored by git). macOS builds prepare arm64, x86_64,
and universal binaries; Windows builds prepare x64.

To refresh the bundled CLI version, update the version, asset
names, and pinned checksums together in
`app/scripts/prepare-lark-cli.mjs`.

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

For app build targets, icon regeneration, the Tauri version-pinning rule, the
monochrome design system, and macOS icon-cache gotchas, see the
[Desktop app section in AGENTS.md](../AGENTS.md#desktop-app--app).

### Google account login

See [Google login setup](google-login.md) for OAuth client creation, backend
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
in [Website deployment](website-deployment.md).

## Reference documentation

| Doc | Covers |
| --- | --- |
| [Data model](data-model.md) | Run artifacts, desktop task index, and timeline replay. |
| [Context window management](context-window-management.md) | Agent turns, tool-result bounds, sawtooth compaction, prompt caching, and artifact evidence retention. |
| [Agent skills and self-healing](agent-skills.md) | Progressive skill loading, constrained local learnings, and the initial self-healing instruction. |
| [CLI telemetry schema](telemetry-schema.md) | Telemetry schema, privacy, and configuration contract for the CLI daemon. |
| [Telemetry runbook](development/telemetry-runbook.md) | Maintainer runbook for operating CLI telemetry. |
| [Release flow](release-flow.md) | GitHub Release workflow, platform build graph, assets, and installer smoke tests. |
| [Website deployment](website-deployment.md) | Vercel deployment runbook for `socai.io`. |
| [Website launch QA](website-launch-qa.md) | Launch checklist used for the `socai.io` rollout. |
| [Browser automation on CDP](browser-automation-evolution.md) | Conceptual map of CDP and how browser-automation frameworks evolved on it. |

## Built with socai

[Jev Social](https://github.com/socai-io/jev-social) is shown in the [README](../README.md#built-with-socai).
