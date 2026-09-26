<div align="center">

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo">
  <img src="site/public/icon-192.png" width="80" alt="socai icon">
</a>

# socai

**English** · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

**A local agent that actually reads social media.**

Not a scraper. Not a reverse-engineered API. socai lives in the Chrome you already use, opens the real page, and comes back with results you want.

<p>
  <img src="site/public/platforms/xiaohongshu.png" height="32" alt="Xiaohongshu">
  &nbsp;&nbsp;
  <img src="site/public/platforms/tiktok.png" height="32" alt="TikTok / Douyin">
  &nbsp;&nbsp;
  <img src="site/public/platforms/instagram.png" height="32" alt="Instagram">
  &nbsp;&nbsp;
  <img src="site/public/platforms/linkedin.svg" height="32" alt="LinkedIn">
</p>
<p><sub>Xiaohongshu · TikTok / Douyin · Instagram · LinkedIn</sub></p>

[Website](https://socai.io/?utm_source=github&utm_medium=readme) · [Download](#desktop-app) · [Discord](https://discord.gg/CpQdA7bwt8) · [Quick start](#quick-start) · [Development](DEVELOPMENT.md)

[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)
[![discord](https://img.shields.io/badge/discord-join-5865F2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/CpQdA7bwt8)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-555?style=flat-square)](#desktop-app)
[![license](https://img.shields.io/badge/license-Apache--2.0-555?style=flat-square)](LICENSE)

<br>

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-banner">
  <img src="docs/assets/socai-readme-banner.png" width="100%" alt="socai research flow from browser discovery through LLM reasoning to structured findings">
</a>

</div>

Social platforms are where the real conversations happen. Public APIs hide them. Scrapers get you banned. socai takes the third path: it drives your signed-in Chrome the way a researcher would — search, open posts, expand comments, read profiles, OCR images, transcribe video — then keeps the artifacts.

Research is read-only by default. Explicit target-bound write commands run only when directly invoked and use durable one-shot receipts to prevent automatic resubmission.

https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d

## Quick start

### Desktop app

Use the desktop app to enter research tasks without setting up a command-line environment. It is available for macOS and Windows:

- [Download for macOS](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Download for Windows](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

After installation, follow the in-app steps to connect Chrome and enter a task such as:

> Compare how people discuss sugar-free tea on RedNote, Douyin, and Instagram. Identify recurring purchase criteria and cite specific posts, videos, comments, and replies.

The first connection to your existing Chrome requires enabling remote debugging and confirming the browser permission prompt. See the [Connect Chrome guide](https://socai.io/connect).

The desktop app keeps task history and artifacts. You can preview or download reports, spreadsheets, images, and other deliverables, or export results to a Feishu document or group chat.

### Command line

The CLI is designed for Claude Code, Codex, and other agents, as well as users who need structured data or scripted workflows.

macOS:

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows PowerShell:

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'; Invoke-WebRequest -UseBasicParsing https://github.com/socai-io/socai/releases/latest/download/install.ps1 -OutFile $installer; Unblock-File $installer; & $installer
```

The installers download and verify the release archive, install socai at `~/.socai/bin/socai` on macOS or `%USERPROFILE%\.socai\bin\socai.exe` on Windows, and configure or explain the PATH update.

Run a structured platform search:

```bash
socai xhs search "beginner camping gear mistakes" --num-notes 10 --num-comments 8 --pretty
socai dy search "beginner camping gear" --num 20
socai tiktok search "beginner camping gear" --num 20 --pretty
socai instagram search "beginner camping gear" --num 20 --pretty
socai linkedin search "product designer" --type people --num 20 --pretty
```

Run `socai` without a subcommand to ask the agent for cross-platform research across the same platforms.

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
| CLI | Agent calls, scripts, and structured JSON | Run `socai xhs ...`, `socai dy ...`, `socai tiktok ...`, `socai instagram ...`, or `socai linkedin ...` |
| Terminal interface | Manually running consecutive tasks in a terminal | Run `socai` |

All three interfaces share the same browser connection, site capabilities, and run-record core.

## Supported platforms

| Platform | Research capabilities | Access |
| --- | --- | --- |
| RedNote (Xiaohongshu) | Search, authors, posts, comments and replies, media download, OCR, and transcription | Agent and structured CLI |
| Douyin | Search, video details, authors, comments and replies, and media artifacts | Agent and structured CLI |
| TikTok | Search, video details, author profiles, comments and replies, and video download | Agent and structured CLI |
| Instagram | Keyword search, profiles, posts, reels, comments and replies, and playable video download | Agent and structured CLI |
| LinkedIn | People, company, and content search; profiles, experience, relationships, posts, and comments | Agent and structured CLI |

Research commands never mutate platform state. The separately documented publish and comment commands require an explicit target and content, verify the signed-in actor and rendered target immediately before dispatch, and never automatically retry an uncertain submit.

## Platform command reference

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

Change the run directory on macOS:

```bash
socai config set runs.dir "$(pwd)/socai-runs"
```

Or in Windows PowerShell:

```powershell
socai config set runs.dir (Join-Path $PWD 'socai-runs')
```

Relative values passed to `runs.dir` are stored as absolute paths from the current directory. `SOCAI_RUNS_DIR` takes precedence when set.

## Extending and developing socai

To add another site or custom capability, follow the [site extension guide](core/src/sites/creation/SKILL.md). It covers requirement confirmation, site capability design, and implementation steps for coding agents such as Claude Code, Codex, and Cursor.

Local development, build instructions, repository conventions, and the reference-document index live in [DEVELOPMENT.md](DEVELOPMENT.md).

## Built with socai

[Jev Social](https://github.com/socai-io/jev-social) is a local-first demo that lets Jev choose bounded socai CLI operations for Instagram, TikTok, and LinkedIn. Captured post cards stay visible while a source-linked research report streams into the browser.

Version 0.1.8 works with OpenRouter Jev or a user-started local System One endpoint; the local path does not use an OpenRouter key. It also ships a reproducible benchmark workflow, without claiming live speed results before the run set is complete.

[![Jev Social demo](https://raw.githubusercontent.com/socai-io/jev-social/main/docs/jev-social.gif)](https://github.com/socai-io/jev-social)

[View the demo](https://socai-io.github.io/jev-social/) · [Install the Agent Skill](https://github.com/socai-io/jev-social/tree/v0.1.8/skills/jev-social) · [Star Jev Social](https://github.com/socai-io/jev-social)

## Community

[Join the Discord](https://discord.gg/CpQdA7bwt8) · or scan the WeChat group QR:

<img src="docs/assets/wechat-group-qr.jpg" alt="socai social media research WeChat group QR code" width="280">

If socai is useful, star the repo.

## License

socai is licensed under the [Apache License 2.0](LICENSE).
