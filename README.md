# socai

[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)
[![rust](https://img.shields.io/badge/built%20with-Rust-orange?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](https://github.com/socai-io/socai/blob/main/LICENSE)
[![website](https://img.shields.io/badge/website-socai.io-blue?style=flat-square)](https://socai.io)

专为小红书优化的 web use agent，执行小红书调研、内容抽取和自定义 agent 任务。

几点优势：
- 不使用程序化的批量爬虫，而是像人一样点击，避免被屏蔽
- 沉淀了小红书网页知识，避免agent盲目探索，又快又准
- 默认复用你已登录的 chrome 小红书账号，避免未登录被屏蔽；也可通过 `socai config` 选择 socai 独立资料目录

## 使用方式

socai 有三种用法，内核相同，按你的场景选：

| 方式 | 是什么 | 如何开始 |
| --- | --- | --- |
| [**CLI**](#cli) | 命令行工具，给 Claude Code、Codex 等 AI agent 调用（核心） | 下载 CLI binary，或用 Cargo fallback |
| [**TUI**](#tui) | 终端里的交互界面，手动跑任务 | 安装 CLI 后运行 `socai` |
| [**GUI**](#desktop-app-gui) | 图形桌面应用（macOS），点击即用 | [下载 .dmg](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg) |

## CLI

socai 的核心，给 Claude Code、Codex 等 AI agent 提供开箱即用的小红书工具。

https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d

优先安装预编译 CLI binary（不需要 Rust/Cargo）：

macOS:

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows PowerShell:

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'; Invoke-WebRequest -UseBasicParsing https://github.com/socai-io/socai/releases/latest/download/install.ps1 -OutFile $installer; Unblock-File $installer; & $installer
```

安装脚本会下载并校验 CLI archive，安装到 `~/.socai/bin/socai`（macOS）或
`%USERPROFILE%\.socai\bin\socai.exe`（Windows），并提示/写入 PATH。

如果当前平台还没有可用的 CLI binary，或你要开发/调试源码，再使用 Cargo fallback：

```bash
git clone https://github.com/socai-io/socai.git
cd socai
cargo install --path cli --force
```

常用命令：

```bash
socai xhs topic_scan "运营爆款思路" --num-notes 30 --filter publish_time=一周内   # 搜索并逐个打开帖子，获取内容
socai xhs topic_scan "运营爆款思路" --num-notes 10 --download-media       # 下载图片/视频并输出 media_manifest_path
socai xhs search_notes "运营爆款思路" --num-notes 100 --filter sort=最新          # 只打开搜索结果页，拿帖子标题/点赞/封面，不读正文
socai xhs extract_note --note-id <id>                                          # 从当前结果页抽取某个帖子
socai config set chrome.profile managed                                    # 以后默认使用 socai 独立 chrome 资料目录
socai stop                                                                 # 停止 daemon（关闭工具标签页）
```

> 命令按站点分组（第一个站点是小红书 `xhs`）。旧的不带站点的写法（如
> `socai topic_scan …`）仍然可用，但已废弃并会打印警告，请迁移到 `socai xhs …`。

Options:

- `--filter <GROUP=OPTION>` — search-result filter, repeatable
  (`topic_scan` & `search_notes`). Groups & options:
  `sort` (综合/最新/最多点赞/最多评论/最多收藏), `note_type` (不限/视频/图文),
  `publish_time` (不限/一天内/一周内/半年内), `search_scope` (不限/已看过/未看过/已关注),
  `distance` (不限/同城/附近). Omitted groups reset to default.
  e.g. `--filter publish_time=一天内 --filter note_type=图文`
- `--tab <TAB>` — search tab to switch to, `topic_scan` only (`全部` / `图文` / `视频` / `用户`).
- `--num-notes <N>` — `topic_scan`: notes to read (opens each, body + comments).
  `search_notes`: cards to collect by auto-scrolling (titles/likes/covers only,
  no bodies — stays fast). Both scroll only if the first page holds fewer; omit
  for the first page only (~19).
- `--download-media` — `topic_scan` only: download note images/videos into the
  printed `run_dir` (`site_media/`), add `local_path` fields, emit top-level
  `media_manifest_path` / `media_manifest_count`, and write `<run_dir>/media_manifest.json`.
- `--pretty` — indented JSON (any tool command).
- `--debug-snapshot` — record DOM + a11y tree + screenshots per page change.

`extract_note` is a
continuation command: a prior `search_notes` / `topic_scan` must have left the
tool tab on a waterfall containing the target card.

Browser profile:

- `chrome.profile` values: `existing`, `managed`, `auto`.
- Default: `existing` — attach to your existing browser/profile with CDP enabled.
- To opt into socai's isolated profile persistently:

  ```bash
  socai config set chrome.profile managed
  socai stop   # only needed if the daemon is already running
  ```

  The default managed profile path is `~/.socai/chrome-profile`; sign in to
  xiaohongshu once there and future sessions reuse that login/cookies.
- To use a custom managed profile directory:

  ```bash
  socai config set chrome.profile_dir ~/.socai/profiles/xhs-test
  ```

- To switch back to your existing browser profile:

  ```bash
  socai config set chrome.profile existing
  ```

- Config lives in `~/.socai/config.json`. Advanced endpoint overrides still use
  `SOCAI_CDP_WS` / `SOCAI_CDP_URL`.

## TUI

安装方式与 CLI 相同，安装后不带子命令运行 `socai` 即可打开终端交互界面：

```bash
socai   # 不带子命令即打开 TUI
```

## Desktop App (GUI)

[Download .dmg for Mac](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg).

## 添加自定义功能和网站
socai具有自我迭代能力，如果你是AI（Claude Code, Codex, Cursor等），遵循`core/src/sites/creation/SKILL.md`中的步骤，和你的用户详细确认需求，并按照skill一步步新增代码，从而增加新功能或新网站。


## 欢迎加群交流

<img src="docs/assets/wechat-group-qr.jpg" alt="socai 小红书使用 微信群二维码" width="280">
