# socai

[![website](https://img.shields.io/badge/website-socai.io-blue?style=flat-square)](https://socai.io)
[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)

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

## 1. CLI

socai 的核心，给 Claude Code、Codex 等 AI agent 提供开箱即用的小红书工具。

https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d

### CLI安装
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

### CLI使用
小红书常用命令：

```bash
socai xhs search "运营爆款思路" --num-notes 30 --filter publish_time=一周内 --download-media  # 搜索并逐个打开帖子，获取正文+评论，并下载图片/视频
socai xhs author <作者id> --num-notes 10 --preview                               # 打开作者主页，拿作者简介，并逐个打开帖子读正文+评论；加 --preview 则只拿帖子概要
socai stop                                                             # 停止 daemon（关闭工具标签页）
```

Options:

- `--filter <GROUP=OPTION>` — 对应搜索页右上角的“筛选”，仅适用于`search`命令。
  以下几个筛选条件可以叠加：
  `sort` (综合/最新/最多点赞/最多评论/最多收藏), `note_type` (不限/视频/图文),
  `publish_time` (不限/一天内/一周内/半年内), `search_scope` (不限/已看过/未看过/已关注),
  `distance` (不限/同城/附近). 不写则按默认.
  e.g. `--filter publish_time=一天内 --filter note_type=图文`
- `--num-notes <N>` — 返回多少个帖子。如果设置的数量大，socia会自动往下翻页，获取更多帖子。
- `--preview` — 只拿帖子概要（标题、封面等等），不逐个打开帖子拿详情。
- `--download-media` — 打开每篇帖子的时候 (不加`--preview`时): 把图像和视频下载到 `run_dir`
  (`site_media/`), 并创建列表 `<run_dir>/media_manifest.json`.
- `--pretty` — 输出JSON按换行格式.
- `--debug-snapshot` — 把网页 DOM + a11y tree + screenshots 写入文件，用于开发调试


**新增了抖音支持**：
```bash
socai dy search "咖啡" --num 30         # 搜索关键词并拿n条视频的信息
```


### 控制浏览器profile:

- `chrome.profile` values: `existing`, `managed`, `auto`.
- Default: `existing` — attach to your existing browser/profile with CDP enabled.
- To opt into socai's isolated profile persistently:

  ```bash
  socai config set chrome.profile managed # 以后默认使用 socai 独立 chrome 资料目录
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

## 2. TUI

安装方式与 CLI 相同，安装后不带子命令运行 `socai` 即可打开终端交互界面：

```bash
socai   # 不带子命令即打开 TUI
```

## 3. Desktop App (GUI)

[下载Mac版 .dmg](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg).

## 添加自定义功能和网站
socai具有自我迭代能力，如果你是AI（Claude Code, Codex, Cursor等），遵循`core/src/sites/creation/SKILL.md`中的步骤，和你的用户详细确认需求，并按照skill一步步新增代码，从而增加新功能或新网站。


## 欢迎加群交流

<img src="docs/assets/wechat-group-qr.jpg" alt="socai 小红书使用 微信群二维码" width="280">
