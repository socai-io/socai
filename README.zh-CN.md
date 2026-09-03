<div align="center">

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo">
  <img src="site/public/icon-192.png" width="80" alt="socai 图标">
</a>

# socai

[English](README.md) · **简体中文** · [日本語](README.ja.md) · [한국어](README.ko.md)

**专为小红书内容调研优化的本地 Web Agent**

连接你已登录的 Chrome，让 Agent 深读帖子、评论和多媒体内容，完成选题、竞品与消费者洞察调研。

[官网](https://socai.io/?utm_source=github&utm_medium=readme) · [下载桌面端](#桌面端) · [快速开始](#快速开始) · [命令参考](#小红书命令参考) · [开发文档](DEVELOPMENT.md)

[![release](https://img.shields.io/github/v/release/socai-io/socai?style=flat-square&color=blue&label=release)](https://github.com/socai-io/socai/releases/latest)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-555?style=flat-square)](#桌面端)
[![license](https://img.shields.io/badge/license-Apache--2.0-555?style=flat-square)](LICENSE)

<br>

<a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-banner">
  <img src="docs/assets/socai-readme-banner.png" width="100%" alt="socai 从浏览器检索，经 LLM 理解，到结构化研究结果的工作流">
</a>

</div>

## 项目简介

socai 面向需要读懂小红书内容的研究任务。它通过 Chrome DevTools Protocol（CDP）连接真实浏览器，复用现有登录状态，以页面点击、输入和滚动完成搜索、帖子阅读、评论展开与作者页查看，并把结果保存为结构化数据和本地素材。

常见任务包括：

- 追踪一个品类近期出现的新话题、情绪和表达方式
- 深读帖子与评论区，提炼消费者需求、顾虑和决策语言
- 比较品牌、产品、门店或活动在小红书上的讨论差异
- 研究账号内容方向、热门帖子和受众反馈
- 下载图片和视频，结合 OCR 与视频语音转写补充多模态证据

当前能力以内容读取和研究为主，暂未提供发布、点赞、收藏或评论等写入操作。

https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d

## 核心能力

| 能力 | 说明 |
| --- | --- |
| 真实浏览器执行 | 默认连接你正在使用的 Chrome，沿页面交互路径完成任务，减少对逆向接口和批量请求的依赖。 |
| 帖子与评论深读 | 获取标题、正文、作者、互动信息和评论，可按需要继续展开评论与回复。 |
| 多模态内容理解 | 支持下载帖子图片和视频、本地图片 OCR，以及视频语音转写。 |
| 调研筛选与采样 | 支持发布时间、内容类型、排序方式、搜索范围和距离等小红书页面筛选条件。 |
| 证据与产物留存 | 每次运行保留结构化结果、素材清单和任务产物，便于复核与继续分析。 |
| 多种使用入口 | 同一套 Rust 内核提供桌面端、命令行和终端交互界面。 |
| Agent 友好 | 命令输出为结构化 JSON，可直接交给 Claude Code、Codex 等 Agent 调用。 |

## 快速开始

### 桌面端

适合直接输入调研任务，无需配置命令行环境。桌面端支持 macOS 和 Windows：

- [下载 macOS 版](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [下载 Windows 版](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

安装后按界面提示连接 Chrome，即可输入任务。例如：

> 调研小红书上最近一个月关于无糖茶的高互动帖子，重点分析用户选择品牌时在意什么，并引用具体帖子和评论作为依据。

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

安装完成后，可以直接运行一次小红书搜索：

```bash
socai xhs search "露营装备新手避坑" --num-notes 10 --num-comments 8 --pretty
```

如果当前平台没有预编译版本，或需要从源码调试，可使用 Cargo 安装：

```bash
git clone https://github.com/socai-io/socai.git
cd socai
cargo install --path cli --force
```

### 终端交互界面

安装命令行程序后，不带子命令运行 `socai` 即可进入终端交互界面：

```bash
socai
```

## 使用入口

| 入口 | 适合场景 | 开始方式 |
| --- | --- | --- |
| 桌面端 | 直接输入自然语言任务、查看历史、预览或下载产物 | 下载 macOS 或 Windows 安装包 |
| 命令行 | 交给 Agent 调用、接入脚本、获取结构化 JSON | 安装后运行 `socai xhs ...` |
| 终端交互界面 | 在终端中手动运行连续任务 | 直接运行 `socai` |

三个入口共享浏览器连接、站点能力和运行记录内核，可根据当前工作方式选择。

## 小红书命令参考

### 搜索并深读帖子

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

### 查看作者及其帖子

```bash
socai xhs author <作者id> --num-notes 10 --num-comments 8
```

只查看作者信息和帖子概要：

```bash
socai xhs author <作者id> --num-notes 20 --preview
```

### 重新读取指定帖子

使用 `search` 或 `author` 返回的帖子 ID 与 `xsec_token`：

```bash
socai xhs get-notes \
  --note '<帖子id>=<xsec_token>' \
  --note '<帖子id>=<xsec_token>' \
  --num-comments 20
```

### 常用参数

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

## 浏览器与登录

socai 提供四种浏览器资料目录模式：

| 模式 | 适合场景 | 登录状态 |
| --- | --- | --- |
| `existing` | 日常使用，默认选项 | 复用现有 Chrome 和小红书登录状态 |
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
socai config set chrome.profile_dir ~/.socai/profiles/xhs-research
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

## 抖音搜索

命令行补充提供基础抖音搜索能力：

```bash
socai dy search "咖啡" --num 30
```

当前产品重点仍是小红书内容调研，抖音命令的能力范围以 `socai dy --help` 为准。

## 扩展与开发

如果需要增加新的站点或自定义能力，请参考 [站点扩展指南](core/src/sites/creation/SKILL.md)。该文件包含需求确认、站点能力设计和实现步骤，适合由 Claude Code、Codex、Cursor 等编程 Agent 按流程执行。

开发环境、构建方式、项目约定和参考文档入口统一收录在 [DEVELOPMENT.md](DEVELOPMENT.md)。

## 社区交流

<img src="docs/assets/wechat-group-qr.jpg" alt="socai 小红书使用微信群二维码" width="280">

欢迎交流使用反馈、调研方法和功能建议。如果 socai 对你有帮助，也欢迎点击右上角的 Star 支持项目持续更新。

## 许可证

socai 采用 [Apache License 2.0](LICENSE) 开源。
