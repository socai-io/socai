<div align="center">

<h1>
  <a href="https://socai.io/?utm_source=github&amp;utm_medium=readme&amp;utm_campaign=main-repo-logo"><img src="site/public/icon-192.png" width="36" alt="socai 图标" align="absmiddle"></a>
  socai
</h1>

**让你的 Agent 真正理解社交平台。**

[English](README.md) · **简体中文** · [日本語](README.ja.md) · [한국어](README.ko.md)

<p>
  <img src="site/public/platforms/xiaohongshu.png" height="32" alt="小红书">
  &nbsp;&nbsp;
  <img src="site/public/platforms/tiktok.png" height="32" alt="抖音 / TikTok">
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

社交平台对 Agent 很难进入。爬虫容易被封。通用的计算机操作又慢、又贵，还缺少各平台的专门知识。

socai 为每个平台建立深层本体，让 Agent 真正理解其中的实体、状态和流程。然后 socai 复用你已登录的 Chrome，保住你的社交账号状态，像人一样去搜索、打开帖子、展开评论、阅读主页、OCR 图片、转写视频。

[![在 Instagram、X 和 TikTok 调研美妆趋势与寻找跑步服饰达人](docs/assets/research-demo-zh.gif)](https://socai.io/?utm_source=github&utm_medium=readme&utm_campaign=research-demo#demo)

| Instagram 调研 | 小红书调研 |
| --- | --- |
| https://github.com/user-attachments/assets/4849e0f3-87d5-4a0d-8e0b-2a58e3d0267a | https://github.com/user-attachments/assets/8aebcded-f365-4f12-b9c4-102cc1fa964d |

## 支持的平台

**小红书** — 搜索、作者、笔记、评论、素材、OCR 和转写。

```bash
socai xhs search "sugar-free tea" --num-notes 10 --num-comments 8
socai xhs author <author_id> --num-notes 10
socai xhs get-notes --note '<note_id>=<xsec_token>' --num-comments 8
socai xhs comment '<note_url>' --text 'Exact comment text'
```

**X** — 搜索、主页、帖子和回复。

```bash
socai x search "open source" --num 10
socai x profile <handle>
socai x get-posts --post https://x.com/<handle>/status/<id> --num-comments 8
socai x reply https://x.com/<handle>/status/<id> --text 'Exact reply text'
```

**抖音** — 搜索、视频、作者和评论。

```bash
socai dy search "coffee" --num 20
socai dy author <author_id> --num 10
socai dy get-videos --video <url> --num-comments 8
```

**TikTok** — 搜索、视频、主页和评论。

```bash
socai tiktok search "coffee" --num 20
socai tiktok author <handle> --num 10
socai tiktok get-videos --video <url> --num-comments 8
```

**Instagram** — 搜索、主页、帖子、Reels 和评论。

```bash
socai instagram search "coffee" --num 10
socai instagram profile nike
socai instagram get-posts --post https://www.instagram.com/p/<shortcode>/ --num-comments 8
socai instagram search_accounts "nike"
socai instagram comment https://www.instagram.com/p/<shortcode>/ --text 'Exact comment text'
```

**LinkedIn** — 人物、公司、帖子和评论。

```bash
socai linkedin search "product designer" --type people --num 10
socai linkedin profile <id>
socai linkedin company <company>
socai linkedin get-posts --post https://www.linkedin.com/posts/<id> --num-comments 8
socai linkedin comment https://www.linkedin.com/posts/<id> --text 'Exact comment text'
```

筛选、评论和其余命令见[使用指南](docs/guide.md#platform-command-reference)。

## 开始使用

桌面端支持 macOS 和 Windows：

- [下载 macOS 版](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [下载 Windows 版](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

macOS 命令行：

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows 安装、Agent 接入、平台命令、Chrome 配置、运行产物和本地开发流程统一见[使用与开发指南](docs/guide.md)。

## 用 socai 做的

[Jev Social](https://github.com/socai-io/jev-social) 是一个本地优先的演示：Jev 选择有边界的 socai CLI 操作，覆盖 Instagram、TikTok 和 LinkedIn。抓到的帖子卡片保持可见，带出处的调研报告同时出现在浏览器里。

[![Jev Social demo](https://raw.githubusercontent.com/socai-io/jev-social/main/docs/jev-social.gif)](https://github.com/socai-io/jev-social)

[安装 Agent Skill](https://github.com/socai-io/jev-social/tree/v0.1.13/skills/jev-social)

## 社区

[加入 Discord](https://discord.gg/CpQdA7bwt8)。

加入微信群：

<img src="docs/assets/wechat-group-qr.jpg" alt="socai 社交媒体调研微信群二维码" width="280">

如果 socai 有用，请给这个仓库点一个 Star。
