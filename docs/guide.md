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

- [Download for macOS](https://socai-downloads.oss-cn-hangzhou.aliyuncs.com/releases/latest/socai-macos-universal.dmg)
- [Download for Windows](https://socai-downloads.oss-cn-hangzhou.aliyuncs.com/releases/latest/socai-windows-x86_64-setup.exe)

After installation, follow the in-app steps to connect Chrome and enter a task such as:

> Compare how people discuss sugar-free tea on RedNote, Douyin, and Instagram. Identify recurring purchase criteria and cite specific posts, videos, comments, and replies.

The first connection to your existing Chrome requires enabling remote debugging and confirming the browser permission prompt. See the [Connect Chrome guide](https://socai.io/connect).

See [Desktop workflow](#desktop-workflow) for source selection and Feishu exports,
and [Run results and artifacts](#run-results-and-artifacts) for saved deliverables.

### Command line

The CLI is designed for agent-driven workflows, including Claude Code and Codex, and returns structured data for each platform operation.

macOS:

```bash
curl -fsSL https://socai-downloads.oss-cn-hangzhou.aliyuncs.com/releases/latest/install.sh | sh
```

Windows PowerShell:

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'; Invoke-WebRequest -UseBasicParsing https://socai-downloads.oss-cn-hangzhou.aliyuncs.com/releases/latest/install.ps1 -OutFile $installer; Unblock-File $installer; & $installer
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

### Workflow and research depth

The composer has independent **General / VC research**, **Light / Deep research** sliding switch
and source controls. New tasks start in light mode. VC research defaults to
opportunity discovery: useful teams, projects, relationships and uncertain
signals with source links. Meeting preparation, diligence and tracking suggestions
are produced when requested; selecting VC does not contact people or start a
monitor. The first skill is intentionally concise and lives at
[`vc-research/SKILL.md`](../core/src/agent/skills/vc-research/SKILL.md).

Light research makes an initial pass (30 action steps); deep research permits
more complementary exploration and selective depth (120). These are ceilings,
not quotas or guarantees of candidate count. Loading a skill does not raise the
budget. Workflow, depth and selected sources persist with each conversation and
can be changed before a follow-up. Existing tasks default to General and Light.

In deep mode, reading a post on a platform with comments (such as Xiaohongshu,
X or Douyin) also includes a lightweight look at the first few available comments,
typically 3–5. Useful praise, complaints, disagreements and recurring feedback
are included as supplementary, source-linked findings. This small sample does
not represent overall public opinion. Routine research does not deeply paginate
comments or expand reply chains; unavailable comments do not block the main task.
Explicit requests for reputation, sentiment or comment analysis determine the
scope and depth instead.

VC works with Auto, one platform, or multiple selected platforms. For broad
sourcing with Auto, it defaults to Xiaohongshu, X, WeChat public articles,
podcasts and GitHub. Explicit selections take precedence. The VC workflow first
calibrates how familiar the direction is to an experienced investor: popular
projects are brief context/comparables, while recommendations emphasize
substantive, less-discussed signals.

Desktop deep research also includes public-web search by default. It uses
DuckDuckGo's HTML results in a separate tab of the connected Chrome browser,
then reads useful original pages. The first parallel research round includes
this web branch alongside selected platforms, even for a single selected
platform. It is implicit in deep mode and has no source-picker option; explicit
user instructions excluding web research take precedence. Web findings retain
source URLs and saved page text; search snippets remain labelled as leads.
Access failures are reported without discarding other branches' findings.
Deep mode needs Chrome even when only GitHub or podcasts are selected; light
mode keeps their browser-free behavior. Temporary worker tabs close on completion
or cancellation.

`explore_platforms` runs up to five platform workers (plus the implicit web worker
in desktop deep mode) concurrently, one per
platform, using separate browser tabs where needed and sharing saved evidence
and usage accounting. The parent synthesizes their findings and can delegate
another focused round with new leads and gaps. Workers cannot recursively spawn
workers. Light/deep workers allow up to 8/18 tool rounds plus a final summary;
these auxiliary calls add to model usage, independently of the parent's action
ceiling. The existing `explore` tool remains available for XHS-only branches.
Unavailable sources and incomplete workers are disclosed. Choosing deep research
does not require diligence on every candidate.

X post links open the shared preview using archived text, authors, timestamps,
images and video covers. Twitter/X URL aliases and share parameters match by
post ID. Unread links show an explicit unavailable-content notice and a link to
the original; opening a preview does not fabricate or silently fetch evidence.

The platform picker, workflow button and depth switch share one control row.
The workflow button opens an icon-card library. **Custom workflow** lets you
write instructions or import a Markdown skill (up to 40 KB), add a name and
short description, then save and select it. Custom workflows are stored on the
current device; each submitted task keeps a copy of its selected instructions.
Editing a library entry does not rewrite a completed task. Workflow instructions
are guidance for research, not an additional source permission or execution budget.

The live **Research workbench** sits below execution progress, which is expanded
while running. It combines per-platform reading counts, focus and observed operations, a
filterable lead table, a sourced relationship graph (dashed for hypotheses),
and next steps while running. The graph stays above the table and includes
isolated leads; platform colors are muted. Lead titles open the shared hover
preview and detail overlay. The table follows new rows until the user scrolls
up. The model incrementally calls `update_research_board`; platform
workers share the same board but only update their assigned platform and leads.
Operation traces come from executed tools. Saved-source counts are archive
counts, not a completeness score. XHS author counts measure profile reads; other
platforms currently count distinct authors in saved sources, labelled separately.
WeChat article reads reuse the platform tab sequentially rather than opening
and closing a tab per article. Raw execution details remain expandable, with
step/time/cost status underneath. Board data is persisted in `research-board.json`
and survives reopening a task.

Deep research normally publishes a CSV or XLSX findings table. If no table was
published, the desktop exports the board to `outputs/research-findings.csv`;
if no leads were structured, it exports saved sources explicitly labelled as
source records. No candidate rows are fabricated. The VC workflow emphasizes
new trends and early signals in the main findings and keeps popular background
brief; unknown details are qualifications, not reasons to hide useful leads in
coverage gaps.

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

Xiaohongshu serves some accounts (and logged-out overseas visitors) from
`www.rednote.com` instead of `www.xiaohongshu.com`. The `xhs` commands work on
either host; result links are always canonical `xiaohongshu.com` URLs. Some
notes are not available on rednote.com and fail there with `note_read_failed`.

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

#### Read a favorites album

List the albums first:

```bash
socai xhs albums --pretty
```

Then open one by the name shown there:

```bash
socai xhs album "专辑名" --pretty
```

This opens the signed-in account from the left sidebar (我 → 收藏 → 专辑) and returns the album's note cards, including the `xsec_token` values `get-notes` needs. Add `--num-notes` to cap the list. It does not open each note.

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

XHS comment results include `top_comments_wait`: the platform's displayed total,
loaded parent/total counts, returned count, stop reason, and `complete`.
The count budget includes replies, and collection remains time-bounded per post.
`complete:false` means the result is a sample or partial collection, not every
comment; a missing comment result is not evidence of zero comments. Completion
requires the visible parent-list end, no remaining reply controls, a known
positive platform total and no count shortfall after trimming. An unknown total
does not prove completeness. The loader merges observed parent and reply IDs
across overlapping viewport steps before virtualized rows disappear, re-captures expanded replies
before scrolling, and refuses collection or actions after the active note changes.
Snapshots belong only to the current native call, not a browser-global cache;
this is not a persisted continuation cursor. Hidden, moderated or unavailable comments are
not bypassed. The coverage metadata remains in compact agent results as well as
the full artifact.

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

### WeChat Official Account articles

```bash
# Search result titles, publishers, snippets and article links.
socai weixin search "personal agent" --num 10
# Search and read the full rendered text of each result in Chrome.
socai weixin search "personal agent" --num 3 --read --pretty
# Read one Sogou result link or WeChat article URL (quote the entire URL).
socai weixin read 'https://mp.weixin.qq.com/s/ARTICLE_ID' --pretty
```

Search requests Sogou's WeChat article endpoint (`type=2`) over HTTP and parses
the returned HTML. Search-only CLI commands do not connect to or start Chrome;
there is no automatic browser fallback if Sogou returns a verification page.
Only `read` or `search --read` connects Chrome to read articles. Anonymous Sogou
search cookies are kept only in process memory, reused for up to one hour, and passed to Chrome
when reading a returned link, preserving its search session without opening a
search tab. Read links in the same daemon/session; restarting it loses this cache.
Each article opens in a fresh owned tab,
waits for a stable nonempty `#js_content`, and closes afterward. Article HTML is
never fetched by an HTTP client. `--num` accepts 1–30 (default 10), `--page` selects
the starting result page, and each call reads at most three search-result pages.
The result includes `last_page` and `has_more` for continuing pagination.
`--wait-seconds` controls the content wait per attempt (default 30, maximum 120).
Verification pages, HTTP 429 and Retry-After responses are retried at most three
times, waiting 1, 2, then 3 minutes. A longer server Retry-After (seconds or HTTP
date) takes precedence. Other failures are not retried. When access remains blocked,
`retry_exhausted: true` is returned; batch reading stops and remaining articles
are marked `not_attempted_after_access_block`. Successful earlier articles remain
in the result. These waits do not solve a CAPTCHA; manual verification may still
be necessary. Retry limits apply per search page or article, within the current call.

Successful reads return title, publisher, displayed publication time, observed
article URL, and full rendered text. Search snippets are never represented as
article bodies. WeChat reads also capture the publisher avatar, image URLs and
ordered text/image blocks. Images download asynchronously from WeChat's image
CDNs into the local archive, using the shared background-media queue and
cancellation lifecycle. The article remains readable while downloads run; each
completed image appears at its original paragraph position, including in an
already-open viewer. Failed/cancelled images show a compact status instead of an
empty gallery. The WeChat viewer is a single-column article reader, and citation
pills use the downloaded publisher avatar (or author initial as fallback).
Previously archived text-only articles remain readable; reread them to capture
images and avatars. Images and video are not transcribed. Sogou and
WeChat links can expire or require verification; failures carry `ok: false` and
`reason`, preserving successful articles when only some reads fail. Verified
article reads enter the shared note archive and JSON artifacts; unvisited search
cards remain in search JSON because their redirect URLs are temporary.
`--debug-snapshot` captures article DOM, accessibility and screenshots under
`weixin-article-*/snapshots/` in the tool output directory. HTTP-only searches
record their normal JSON artifacts without browser snapshots.

Desktop answers cite the actual source URL. When it matches an article in the
current conversation's local archive, the UI displays the archived title with
its preview/viewer; otherwise it keeps the clickable original URL. This first
stage does not fetch link previews or resolve unknown redirects. Internal archive
IDs remain implementation details, and existing `note:` citations still work.

The shared core exposes `search_weixin_articles` and `read_weixin_article` to
CLI/TUI, desktop and remote worker hosts. Select **微信公众号 / WeChat Articles**
in the desktop source picker, or enable site ID `weixin` in worker requests.
The manifest also supports `navigate_site` → `run_site_browser_tool` with
`articleState` and `articleDetail`; the search-page DOM tools remain available for
explicit browser diagnostics, but normal searches use the HTTP tool. The web/hosted
entrypoint must use a worker built with this capability; deploying that worker
is separate from the local implementation.

### GitHub

Select **GitHub** in the desktop source picker or enable site ID `github` in a
worker request. GitHub-only tasks run without Chrome. The read-only
`github_request` tool supports REST, GraphQL and public HTML, including Trending
and Topics; prefer REST/GraphQL for structured repository and discussion details.

Desktop previews show README text/images, owner, stars, language and dates.
Issue, discussion and file links preview their repository and keep the original
link destination. Profiles and discovery pages show their own information.
Snapshots, README files and images are stored in `$SOCAI_HOME/cache/github/v1/`
(default `~/.socai/cache/github/v1/`) for offline reading; metadata reflects the
saved snapshot. Images download in the background and appear without redrawing
existing content. Pending images stay blank; failures are summarized once.

Inside a registered CLI task:

```bash
socai github request /search/repositories --param 'q=personal agent in:name,description,readme fork:false' --param sort=updated --param per_page=10 --pretty
socai github request 'https://github.com/trending?since=weekly' --pretty
socai github request https://github.com/topics/personal-assistant --pretty
socai github request /repos/HKUDS/nanobot/readme --select /content --max-chars 6000 --pretty
```

Public REST/HTML allow unauthenticated reads within GitHub's limits; GraphQL
requires authentication. Credential priority on the execution host is
`SOCAI_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, then `gh auth token --hostname github.com`.
Desktop apps need the inherited environment or `gh` on PATH; workers need their
own credentials. Restart after environment changes. Tokens only go to
`api.github.com`; GitHub Enterprise is not supported.

Use `--query` with `--variables` for GraphQL, `--field` for result fields, and
`--select` for a JSON Pointer into the full response. `--limit` bounds local arrays;
`--max-chars` bounds text. For clipped data, follow `next_offset` before server
pagination: REST `pagination.links.next`, HTML `data.next_url`, or GraphQL
`pagination.connections`. Pages and comment/reply connections are read separately.

Results retain fetch time, actual URL, rate limits and full local response paths.
HTTP/GraphQL failures are explicit; partial HTML coverage and truncated fields
carry notices. Response files stay local rather than being published automatically.

### Podcasts (Chinese and international)

Select **播客 / Podcasts** in the desktop source picker, or use site ID
`podcast`. BidClub's public REST API requires no API key, login, or Chrome.
Podcast-only desktop tasks also work with Chrome disconnected; mixed-source
and automatic-source tasks may still use a browser for their other sources.

Inside a registered CLI task:

```bash
socai podcast shows "十字路口" --pretty
socai podcast shows "Lex" --pretty
socai podcast episodes --show shizilukou --limit 5 --pretty
socai podcast search "support ticket" --show shizilukou --limit 3 --pretty
socai podcast read shizilukou-2025-03-02-ai-google --pretty
socai podcast read shizilukou-2025-03-02-ai-google --section digest --pretty
socai podcast read shizilukou-2025-03-02-ai-google --section transcript --max-chars 12000 --pretty
```

The shared agent tools are `search_podcast_shows`, `list_podcast_episodes`,
`search_podcast_episodes`, and `read_podcast_episode`. Show search matches names,
hosts and IDs; episode search matches titles, summaries or transcripts.
Omit `--lang` to search across languages, or use `EN` / `ZH` to filter the
original language. This filter is separate from `--language ZH|EN`, which
selects BidClub’s bilingual titles and summaries without excluding foreign
episodes. Desktop tools default to the question’s language, falling back to the
app language for short/ambiguous prompts. Explicit tool `language` overrides
that default. Missing translations report `language_fallback`; transcripts
remain in the original language, and `full` reports `section_languages`.
Keep the same `--language` when continuing a paginated section.

Podcast cards use article previews with RSS channel artwork when available and
a separate original-podcast listening link. Material cards show only the show and
episode title. Previews have TL;DR expanded, with in-depth summary and full
transcript collapsed. Expanding either loads the complete section, outside the
agent context, and caches it under `$SOCAI_HOME/cache/podcast/v1/` (normally
`~/.socai/cache/podcast/v1/`), keyed by episode, language and section. Errors can
be retried; missing content is not persisted. Participants are taken from
BidClub's explicit episode participant list, not its general person tags, and
are cached separately. Known hosts remain visible if participants are missing. Cite the returned BidClub URL once;
there is no need to append the publisher URL again in the answer.

Episode searches and lists return metadata + TL;DR, default 5 results (maximum
20), with at most 4,000 characters per summary. `read` defaults to `tldr`;
`--section metadata|tldr|digest|transcript|full` selects the required depth.
`full` combines TL;DR, digest and transcript, reporting any missing sections.
Reads return `content`, `offset`, `total_chars`, `next_offset`, and `available`.
Pass `--offset <next_offset>` until it is null to read all text; offsets and
`--max-chars` count Unicode characters, with a 20,000-character page maximum.
A default summary with a non-null `next_offset` can be continued with `read`.

BidClub covers its indexed library, not every podcast. Search returns at most
20 provider hits without pagination, and `count` is the number returned, not
all matches. Preserve `partial` and `searched_recent`: the provider may search
only a recent subset, so zero hits do not prove absence. Narrowing with `--show`
can find older content missed by global search. `omitted_by_limit` identifies
hits left out by the requested result limit. Episode lists use
`pagination.next_offset`. `details_incomplete` and per-episode `detail_error`
identify failed summary reads without hiding the available metadata. Summaries
and transcripts are processed text and may lack reliable timestamps or verbatim
accuracy. Missing text returns `available: false`; it is never silently replaced
with another section. HTTP reads have a timeout and one bounded retry for
transient network/server errors.

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

### Xiaohongshu host preference

Xiaohongshu serves the same web app from `www.xiaohongshu.com` and, for
accounts it routes to its international brand, `www.rednote.com`. A login's
cookies stay on the host it was made on. The `xhs` commands open
`xiaohongshu.com` by default; for an account that lives on rednote.com, set
`xhs.host` so they open that host instead (otherwise they read as logged out):

```bash
socai config set xhs.host rednote.com      # or xiaohongshu.com (the default)
socai config unset xhs.host
```

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

Downstream desktop applications may depend directly on `socai-core` and own a
`SocaiRuntime` in process. Set `SOCAI_CONFIG_PATH` once at process startup to use
an application-specific configuration file; the default remains
`~/.socai/config.json`. This does not relocate other storage: configure
`SOCAI_HOME`, run paths and the managed Chrome profile separately. External site
packages under `SOCAI_SITE_SKILLS_DIR` can add new site ids without a native
adapter. `scoped_site_learning_tools` limits an agent to the host's chosen sites;
`run_site_browser_tool` supports direct host calls. `PageSession::bring_to_front`
reveals a page for manual login or verification without navigating or resizing.
`scroll_at` scrolls a rendered iframe or modal at viewport coordinates,
`select_all` sends the native text-selection chord, and
`screenshot_region_jpeg` captures a visible region for OCR. Scroll and selection
follow the configured action speed; keep site identity and completion checks in
the downstream workflow.

For source installation, see [Command line](#command-line). For day-to-day
iteration, build and run from the repository root:

```bash
cargo build                 # build the whole workspace (core + cli)
cargo run -p socai-cli -- xhs search "运营爆款思路" --num-notes 30
cargo test                  # run the workspace test suite
```

Page-script regressions run without a browser or third-party Node packages:

```bash
node --test core/src/sites/*/*.test.cjs
```

For the XHS native-DOM pagination fixture, reuse the already-approved daily
Chrome through the local CDP daemon (no new Chrome process):

```bash
SOCAI_ACCEPTANCE_TARGET='existing-dev-tab-id' \
SOCAI_ACCEPTANCE_EMAIL='your-email@example.com' \
SOCAI_ACCEPTANCE_ACCOUNT_ID='expected-account-id' \
node scripts/check-xhs-comment-dom.mjs /absolute/new-comment-dom-evidence.json
```

This local operator helper requires an already-signed-in dev tab and explicit
expected identity; it verifies both email and account ID before
opening its own loopback fixture in that tab's browser context. It foregrounds
only the verified fixture so Chrome dispatches native scroll events. An optional
second argument reuses a fixture target created by this helper. The fixture is
not live-platform or model/Worker acceptance.

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

### Shared hosted worker library

The registry-ready hosted worker manifest uses `socai-worker = "=0.1.0"`.
Registry consumption requires publishing the protocol, core, and worker in that
order; a GitHub release alone does not publish them to crates.io. Its dependencies pin
`socai-core = "=0.6.7"` and `socai-worker-protocol = "=0.1.0"`.
For unpublished development builds, pin a reviewed Git commit rather than
following a moving branch. Use `run_stdio_with_build_info` to identify that
build in the host's version/capability handshake. `socai-core` remains the
shared implementation for desktop, CLI and worker entrypoints.

Hosted workers can opt into a loopback, bearer-authenticated OCR service through
their host-tool endpoint. Leaving this unset preserves bundled local OCR for
desktop and CLI. Native social write tools are exposed to a worker only when
the host supplies exact, one-time `external_write_grants`; read-only research
does not grant publishing permission. Independent platform tool scopes may run
concurrently, while calls within one scope and external writes remain ordered.
Browserless site adapters keep their browser-free execution path.

Hosted operators can set `SOCAI_READ_TOOL_IDLE_SECONDS` (1–300) and
`SOCAI_READ_TOOL_WALL_SECONDS` (1–3600, default 900) to bound selected read-only
research calls. Only real tool progress renews the idle deadline; the wall-clock
deadline never renews. On timeout the agent stops bulk retries and produces an
explicitly partial answer from saved evidence instead of losing the whole run
to the host's outer inactivity timeout. This watchdog does not cancel external
writes or shell commands and is disabled unless the idle setting is supplied.

For OpenRouter only, `SOCAI_OPENROUTER_FALLBACK_MODELS` accepts a bounded,
comma-separated fallback list. Usage records retain the model actually served
so hosts can attribute fallback costs correctly. With the setting absent, the
primary model behavior is unchanged.

Validate changes with `cargo test --locked -p socai-core -p socai-worker -p
socai-worker-protocol` and compile the consuming CLI/desktop entrypoints before
updating a host's pinned dependency.

### HTTP-only site adapters

Native adapters expose `browserless_tools(task, language)` to skip page acquisition
in desktop, worker and TUI; CLI uses `runtime_command`. Keep frontend
`sourcesNeedBrowser` aligned with the registry. GitHub's client, extractors and
manifest live in `core/src/sites/github/`; discovery guidance lives in `knowledge.md`.
Check core/CLI/worker and desktop builds, then verify live API/HTML reads,
pagination, explicit failures and a GitHub-only run without Chrome.

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

### Sogou WeChat article experiment

Run the standalone script with Node.js 22+ and installed Google Chrome; no npm
packages or browser extension are needed:

```bash
node scripts/sogou-weixin-chrome.mjs "人工智能" --limit 3 --out /tmp/weixin-articles
```

It opens Sogou's `https://weixin.sogou.com/weixin?type=2&query=...` search
endpoint in Chrome, collects article links, follows each redirect in a fresh
Chrome tab, and reads the rendered WeChat `#js_content` body. Article content
is never downloaded by a separate HTTP client. This is a standalone experiment,
not a registered Socai site skill.

The output directory contains `results.json`, the search HTML/text, and each
article's `.html`, `.page.txt` (whole page), and `.txt` (article body). Stdout also
contains the JSON results; progress goes to stderr. A nonempty body on a WeChat
article URL is required for `status: "ok"`; this verifies text extraction, not
image/OCR or video coverage. Failed pages retain diagnostics rather than being
reported as article text. Exit codes: 0 for successful requested reads, 2 for
missing results or any failed article, 1 for setup/runtime errors. Fewer search
results than `--limit` may be returned; `requested` records the requested limit.

By default the script starts visible Chrome with a temporary profile and closes
it afterward. `--cdp http://127.0.0.1:9222` reuses an existing debugging-enabled
Chrome, closing only tabs created by the script. Use `--chrome /path/to/chrome`
(or `CHROME_PATH`) to override the executable, and `--timeout 60` to extend each
page's content wait. Sogou/WeChat can return verification or unavailable pages;
inspect the saved HTML/text when extraction fails. Use a separate output
directory per run to keep the captures distinct.

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

### macOS release validation

Release builds use the pinned `macos-15` runner and continue producing universal
Intel/Apple Silicon binaries. The build machine's OS is not the minimum user OS.
The desktop target is explicitly pinned to 10.13; CLI and ASR retain 10.12 on
Intel, and arm64 retains its 11.0 floor. The bundled Feishu CLI requires 11.0.
These are binary deployment targets, not a claim that every feature has been
validated on every older macOS release.

Before publication, `verify-macos-artifacts` installs the candidate CLI via the
real installer, copies the app from its DMG, and starts the app and its sidecars
on both `macos-15` (Apple Silicon) and `macos-15-intel`. It also checks both Mach-O
architectures and their deployment targets, Developer ID signatures, notarization
tickets, and the updater archive's signature against the shipped public key.
The updater archive is extracted and started separately. This does not exercise
the in-app update button, logged-in features, or older-OS UI behavior.

The macOS signing and verification jobs use GitHub's temporary HTTPS/TCP workaround for
[Apple ticket-service HTTP/3 resets](https://github.com/actions/runner-images/issues/14924).
Ticket verification also retries bounded transport failures; invalid tickets
still fail the release gate.

For a trial without publishing, dispatch `release.yml` from a `fix/release-*`
branch. The version bump exists only in the build checkout; the publish job and
public-latest checks are skipped. Full qualification requires the Apple signing,
notarization, and updater secrets even on the trial branch. No new release/tag or
OSS latest promotion is performed. For OS migrations, additionally run the same
artifact checks on the oldest affected OS while an appropriate runner is still
available; do not retain a retired runner as a production dependency.

### Google account login

See [Google login setup](google-login.md) for OAuth client creation, backend
configuration, account linking, database rollout and acceptance checks. The
companion private backend is `socai-server`; both it and the desktop app must
include the Google login implementation.

### Account balance and invalid sessions

A wallet response with HTTP 401 shows **sign-in expired** and a **sign in again**
button. That action clears the saved login through the logout flow and opens
the existing phone/Google login options. A network failure or other HTTP error
keeps the session and offers **retry**. Choosing a custom LLM API key does not
disable wallet requests or invalidate the account login.

Device credentials have no fixed expiry or refresh-token endpoint. When moving
the backend, preserve its device-token hash key or configure the backend's
trusted legacy device keys; copying device rows alone across different keys
does not preserve sign-in.

Run `node app/scripts/check-auth-session.mjs` from the repository root to check
these UI flows with a mocked Tauri transport, including late responses after
logout and re-login. It does not access real accounts or the network.

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

### Shared research policy across desktop and hosted workers

`core/src/agent/research_options.rs` owns depth/expert instructions, initial skill
selection, platform exploration tools, reading statistics and fallback findings
CSV. Both Tauri and `socai-worker` consume these options. Keep research policy and
VC skill changes in core, not in host-specific prompt copies. The durable
research ledger tools are shared through `research::ledger_tools()`.

Memory compression and rendering live in `agent::local_memory`; local storage
uses files while hosted workers exchange entries and summaries with authenticated
account-owned host storage. A hosted worker must not enable local cross-account
memory. Its capabilities `research_experts.v1` and `account_memory.v1` let hosts
reject older binaries that would silently omit these features.
