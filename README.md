<div align="center">

<h1>
  <a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo"><img src="site/public/icon-192.png" width="36" alt="socai icon" align="absmiddle"></a>
  socai
</h1>

**Makes your agent actually understand social platforms.**

**English** · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [한국어](README.ko.md)


<p>
  <img src="site/public/platforms/xiaohongshu.png" height="32" alt="Xiaohongshu">
  &nbsp;&nbsp;
  <img src="site/public/platforms/tiktok.png" height="32" alt="TikTok / Douyin">
  &nbsp;&nbsp;
  <img src="site/public/platforms/instagram.png" height="32" alt="Instagram">
  &nbsp;&nbsp;
  <img src="site/public/platforms/linkedin.svg" height="32" alt="LinkedIn">
  &nbsp;&nbsp;
  <img src="site/public/platforms/x.png" height="32" alt="X">
</p>

[![website](https://img.shields.io/badge/website-socai.io-555?style=flat-square&color=blue)](https://socai.io/?utm_source=github&utm_medium=readme)
[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)
[![discord](https://img.shields.io/badge/discord-join-5865F2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/CpQdA7bwt8)
[![license](https://img.shields.io/badge/license-Apache--2.0-555?style=flat-square)](LICENSE)

<br>

</div>

Social media platforms are hard to access for agents. Scrapers get you banned. Generic computer use methods are slow, expensive and lack the know-hows. 

socai, instead, builds the deep ontology of each platform. This enables agents to actually understand the entities, states and flows in each platform. Then socai reuses your signed-in Chrome -- and thus keeps your social status -- to search, open posts, expand comments, read profiles, OCR images, transcribe video and so on, just like a human does.

[![Beauty trend research and running creator discovery across Instagram, X and TikTok](docs/assets/research-demo-en.gif)](https://socai.io/?utm_source=github&utm_medium=readme&utm_campaign=research-demo#demo)

| Instagram research | Xiaohongshu research |
| --- | --- |
| https://github.com/user-attachments/assets/4849e0f3-87d5-4a0d-8e0b-2a58e3d0267a | https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d |

## Supported platforms

**RedNote (Xiaohongshu)** — search, authors, posts, comments, media, OCR, and transcription.

```bash
socai xhs search "sugar-free tea" --num-notes 10 --num-comments 8
socai xhs author <author_id> --num-notes 10
socai xhs get-notes --note '<note_id>=<xsec_token>' --num-comments 8
socai xhs comment '<note_url>' --text 'Exact comment text'
```

**X** — search, profiles, posts, and replies.

```bash
socai x search "open source" --num 10
socai x profile <handle>
socai x get-posts --post https://x.com/<handle>/status/<id> --num-comments 8
socai x reply https://x.com/<handle>/status/<id> --text 'Exact reply text'
```

**Douyin** — search, videos, authors, and comments.

```bash
socai dy search "coffee" --num 20
socai dy author <author_id> --num 10
socai dy get-videos --video <url> --num-comments 8
```

**TikTok** — search, videos, profiles, and comments.

```bash
socai tiktok search "coffee" --num 20
socai tiktok author <handle> --num 10
socai tiktok get-videos --video <url> --num-comments 8
```

**Instagram** — search, profiles, posts, reels, and comments.

```bash
socai instagram search "coffee" --num 10
socai instagram profile nike
socai instagram get-posts --post https://www.instagram.com/p/<shortcode>/ --num-comments 8
socai instagram search_accounts "nike"
socai instagram comment https://www.instagram.com/p/<shortcode>/ --text 'Exact comment text'
```

**LinkedIn** — people, companies, posts, and comments.

```bash
socai linkedin search "product designer" --type people --num 10
socai linkedin profile <id>
socai linkedin company <company>
socai linkedin get-posts --post https://www.linkedin.com/posts/<id> --num-comments 8
socai linkedin comment https://www.linkedin.com/posts/<id> --text 'Exact comment text'
```

Filters, comments, and the rest of the commands are in the [user guide](docs/guide.md#platform-command-reference).

## Get started

Desktop app for macOS and Windows:

- [Download for macOS](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Download for Windows](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

CLI on macOS:

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Connect the installed CLI to Codex, Claude Code, Cursor, Gemini CLI, OpenCode, GitHub Copilot, and other local Agent Skills hosts:

```bash
socai integrate install all
socai integrate status --json
```

The portable [`socai-social-research` Skill](https://skills.sh/socai-io/socai/socai-social-research) keeps agent-driven research read-only and covers all six supported platforms.

Windows installation, agent setup, platform commands, Chrome profiles, run artifacts, and local development are documented in the [guide](docs/guide.md).

Browser protocol compatibility and security boundaries are documented in the [browser backend support matrix](docs/browser-backends.md).

## Built with socai

[Jev Social](https://github.com/socai-io/jev-social) is a local-first demo that lets Jev choose bounded socai CLI operations for Instagram, TikTok, and LinkedIn. Captured post cards stay visible while a source-linked research report streams into the browser.

[![Jev Social demo](https://raw.githubusercontent.com/socai-io/jev-social/main/docs/jev-social.gif)](https://github.com/socai-io/jev-social)

[Install the Agent Skill](https://github.com/socai-io/jev-social/tree/v0.1.13/skills/jev-social)

## Community

[Join the Discord](https://discord.gg/CpQdA7bwt8).

Join WeChat group:

<img src="docs/assets/wechat-group-qr.jpg" alt="socai social media research WeChat group QR code" width="280">

If socai is useful, star the repo.
