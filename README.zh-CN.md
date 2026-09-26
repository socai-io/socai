<div align="center">

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo">
  <img src="site/public/icon-192.png" width="80" alt="socai 图标">
</a>

# socai

[English](README.md) · **简体中文** · [日本語](README.ja.md) · [한국어](README.ko.md)

**真的会读社交媒体的 Agent。**

不是爬虫，也不是逆向接口。socai 住在你已经登录的 Chrome 里，打开真页面，交回带出处的证据。

<p>
  <img src="site/public/platforms/xiaohongshu.png" height="32" alt="小红书">
  &nbsp;&nbsp;
  <img src="site/public/platforms/tiktok.png" height="32" alt="抖音 / TikTok">
  &nbsp;&nbsp;
  <img src="site/public/platforms/instagram.png" height="32" alt="Instagram">
  &nbsp;&nbsp;
  <img src="site/public/platforms/linkedin.svg" height="32" alt="LinkedIn">
</p>
<p><sub>小红书 · 抖音 / TikTok · Instagram · LinkedIn</sub></p>

[官网](https://socai.io/?utm_source=github&utm_medium=readme) · [下载桌面端](#桌面端) · [Discord](https://discord.gg/CpQdA7bwt8) · [快速开始](#快速开始) · [开发文档](DEVELOPMENT.md)

[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)
[![discord](https://img.shields.io/badge/discord-join-5865F2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/CpQdA7bwt8)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-555?style=flat-square)](#桌面端)
[![license](https://img.shields.io/badge/license-Apache--2.0-555?style=flat-square)](LICENSE)

<br>

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-banner">
  <img src="docs/assets/socai-readme-banner.png" width="100%" alt="socai 从浏览器检索，经 LLM 理解，到结构化研究结果的工作流">
</a>

</div>

真实讨论发生在社交媒体上。公开 API 看不到，爬虫容易被封。socai 走第三条路：用你已登录的 Chrome，像调研的人一样去搜、点开、翻评论、看主页、OCR 图片、转写视频，并把证据留下来。

调研能力默认只读。只有用户直接调用、明确指定目标的写入命令才会执行，并通过持久化的一次性凭据防止自动重复提交。

https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d

## 快速开始

### 桌面端

适合直接输入调研任务，无需配置命令行环境。桌面端支持 macOS 和 Windows：

- [下载 macOS 版](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [下载 Windows 版](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

安装后按界面提示连接 Chrome，即可输入任务。例如：

> 对比小红书、抖音和 Instagram 上关于无糖茶的讨论，分析用户选择品牌时反复提到的因素，并引用具体帖子、视频、评论和回复。

首次连接现有 Chrome 时，需要开启远程调试并确认浏览器授权。可参考 [连接 Chrome 指南](https://socai.io/connect)。

桌面端会保存任务历史和产物，可预览或下载报告、表格、图片等文件，也支持将结果导出到飞书文档或群聊。

### 命令行

命令行适合 Claude Code、Codex 等 Agent 调用，也适合需要结构化数据和自动化流程的用户。

macOS：

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows PowerShell：

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'; Invoke-WebRequest -UseBasicParsing https://github.com/socai-io/socai/releases/latest/download/install.ps1 -OutFile $installer; Unblock-File $installer; & $installer
```

安装脚本会下载并校验对应平台的命令行程序，安装到 `~/.socai/bin/socai`（macOS）或 `%USERPROFILE%\.socai\bin\socai.exe`（Windows），并处理或提示 PATH 配置。

安装完成后，可以直接运行结构化平台搜索：

```bash
socai xhs search "露营装备新手避坑" --num-notes 10 --num-comments 8 --pretty
socai dy search "露营装备" --num 20
socai tiktok search "camping gear" --num 20 --pretty
socai instagram search "camping gear" --num 20 --pretty
socai linkedin search "product designer" --type people --num 20 --pretty
```

不带子命令运行 `socai`，可以直接向 Agent 提出同样覆盖这些平台的跨平台任务。

如果当前平台没有预编译版本，或需要从源码调试，可使用 Cargo 安装：

```bash
git clone https://github.com/socai-io/socai.git
cd socai
cargo install --path cli --force --locked
cargo install --path asr --force --locked
```

第二条命令会把本地 Whisper helper 安装到 `socai` 同一目录；非付费或离线转写使用内置模型时需要该组件。

### 终端交互界面

安装命令行程序后，不带子命令运行 `socai` 即可进入终端交互界面：

```bash
socai
```

## 使用入口

| 入口 | 适合场景 | 开始方式 |
| --- | --- | --- |
| 桌面端 | 直接输入自然语言任务、查看历史、预览或下载产物 | 下载 macOS 或 Windows 安装包 |
| 命令行 | 交给 Agent 调用、接入脚本、获取结构化 JSON | 运行 `socai xhs ...`、`socai dy ...`、`socai tiktok ...`、`socai instagram ...` 或 `socai linkedin ...` |
| 终端交互界面 | 在终端中手动运行连续任务 | 直接运行 `socai` |

三个入口共享浏览器连接、站点能力和运行记录内核，可根据当前工作方式选择。

## 支持平台

| 平台 | 调研能力 | 使用方式 |
| --- | --- | --- |
| 小红书 | 搜索、作者、帖子、评论与回复、素材下载、OCR 和语音转写 | Agent 与结构化命令行 |
| 抖音 | 搜索、视频详情、作者、评论与回复、素材留存 | Agent 与结构化命令行 |
| TikTok | 搜索、视频详情、作者主页、评论与回复、视频下载 | Agent 与结构化命令行 |
| Instagram | 关键词搜索、个人主页、帖子、Reels、评论与回复、视频下载 | Agent 与结构化命令行 |
| LinkedIn | 人员、公司和内容搜索，个人经历、关系线索、帖子与评论 | Agent 与结构化命令行 |

调研命令不会改变平台状态。单独说明的发布和评论命令必须提供明确目标与内容，提交前会再次校验登录账号及页面目标；提交结果不确定时绝不自动重试。

## 平台命令参考

### 小红书

#### 搜索并深读帖子

```bash
socai xhs search "运营爆款思路" \
  --num-notes 30 \
  --num-comments 20 \
  --filter publish_time=一周内 \
  --filter sort=最多评论 \
  --download-media \
  --ocr \
  --pretty
```

`search` 会执行站内搜索，并逐个打开结果读取正文和评论。加上 `--preview` 时，仅返回标题、封面和互动信息等概要，不打开帖子详情。

#### 查看作者及其帖子

```bash
socai xhs author <作者id> --num-notes 10 --num-comments 8
```

只查看作者信息和帖子概要：

```bash
socai xhs author <作者id> --num-notes 20 --preview
```

#### 重新读取指定帖子

使用 `search` 或 `author` 返回的帖子 ID 与 `xsec_token`：

```bash
socai xhs get-notes \
  --note '<帖子id>=<xsec_token>' \
  --note '<帖子id>=<xsec_token>' \
  --num-comments 20
```

#### 常用参数

| 参数 | 作用 |
| --- | --- |
| `--num-notes <N>` | 计划返回的帖子数量；数量较大时会继续滚动页面。 |
| `--num-comments <N>` | 每篇帖子读取的评论和回复数量；设为 `0` 时跳过评论。 |
| `--preview` | 只读取搜索结果或作者页上的帖子概要。 |
| `--download-media` | 下载已打开帖子的图片和视频，并记录本地路径。 |
| `--ocr` | 使用本地 OCR 读取图片文字；视频帖子读取封面文字。 |
| `--transcribe-audio` | 下载已打开的视频并转写语音；需要选择 socai agent 并保持登录。 |
| `--filter <组=选项>` | 使用小红书搜索页筛选条件，可重复传入。 |
| `--pretty` | 将最终 JSON 按缩进和换行输出。 |
| `--debug-snapshot` | 保存页面 DOM、无障碍树和截图，供开发排查。 |

可用筛选组：

| 筛选组 | 可用选项 |
| --- | --- |
| `sort` | 综合、最新、最多点赞、最多评论、最多收藏 |
| `note_type` | 不限、视频、图文 |
| `publish_time` | 不限、一天内、一周内、半年内 |
| `search_scope` | 不限、已看过、未看过、已关注 |
| `distance` | 不限、同城、附近 |

多个筛选条件可以叠加：

```bash
socai xhs search "上海周末活动" \
  --filter publish_time=一周内 \
  --filter note_type=图文 \
  --filter sort=最新
```

### 抖音与 TikTok

```bash
socai dy search "咖啡" --num 30
socai tiktok search "coffee" --num 30 --pretty
```

视频详情、作者、评论、素材下载和诊断命令可通过 `socai dy --help` 或 `socai tiktok --help` 查看。

### Instagram

```bash
socai instagram search "coffee" --num 20 --pretty
socai instagram profile nike --num 12
socai instagram get-posts --post https://www.instagram.com/p/<shortcode>/ --num-comments 8
```

个人主页、帖子 / Reels、评论和诊断命令可通过 `socai instagram --help` 查看。

### LinkedIn

```bash
socai linkedin search "product designer" --type people --num 20 --pretty
socai linkedin profile https://www.linkedin.com/in/<id>/
socai linkedin history <id> --section experience
socai linkedin company <company-id>
socai linkedin get-posts --post https://www.linkedin.com/posts/<id> --num-comments 8
```

公司、关系线索、帖子、评论和诊断命令可通过 `socai linkedin --help` 查看。

## 浏览器与登录

socai 提供四种浏览器资料目录模式：

| 模式 | 适合场景 | 登录状态 |
| --- | --- | --- |
| `existing` | 日常使用，默认选项 | 复用现有 Chrome 及各支持平台的登录状态 |
| `managed` | 希望与日常浏览器隔离 | 使用 `~/.socai/chrome-profile`，首次需要登录 |
| `auto` | 希望自动选择连接方式 | 优先启动独立资料目录，失败时连接现有 Chrome |
| `remote` | 测试托管云浏览器 | socai pro 测试能力，受会话额度限制 |

这些设置保存在 `~/.socai/config.json`，命令行和桌面端读取同一份配置。

切换到独立资料目录：

```bash
socai config set chrome.profile managed
socai stop
```

如需指定独立资料目录的位置：

```bash
socai config set chrome.profile_dir ~/.socai/profiles/social-research
```

切回现有 Chrome：

```bash
socai config set chrome.profile existing
socai stop
```

`socai stop` 仅在后台进程已经运行时需要，用于让新配置在下一次启动时生效。

托管浏览器目前处于测试阶段。使用前需要激活 socai pro：

```bash
socai pro activate <invite_code>
socai config set chrome.profile remote
```

高级连接覆盖仍可使用 `SOCAI_CDP_WS` 或 `SOCAI_CDP_URL`。

## 运行结果与素材

默认情况下，每次运行会写入：

```text
~/.socai/runs/<时间>_<任务>/
```

常见内容包括：

- 最终结构化结果与运行元数据
- 搜索、帖子和作者信息
- 下载的图片、视频和 OCR 结果
- `media_manifest.json` 素材清单
- 调试快照和 Agent 生成的报告、表格等产物

可以将运行目录改到指定位置：

macOS：

```bash
socai config set runs.dir "$(pwd)/socai-runs"
```

Windows PowerShell：

```powershell
socai config set runs.dir (Join-Path $PWD 'socai-runs')
```

相对路径会按当前目录转换为绝对路径保存。环境变量 `SOCAI_RUNS_DIR` 的优先级更高。

## 扩展与开发

如果需要增加新的站点或自定义能力，请参考 [站点扩展指南](core/src/sites/creation/SKILL.md)。该文件包含需求确认、站点能力设计和实现步骤，适合由 Claude Code、Codex、Cursor 等编程 Agent 按流程执行。

开发环境、构建方式、项目约定和参考文档入口统一收录在 [DEVELOPMENT.md](DEVELOPMENT.md)。

## 社区交流

[加入 Discord](https://discord.gg/CpQdA7bwt8) · 或扫描微信群二维码：

<img src="docs/assets/wechat-group-qr.jpg" alt="socai 社交媒体调研微信群二维码" width="280">

觉得有用的话，欢迎 Star。

## 许可证

socai 采用 [Apache License 2.0](LICENSE) 开源。
