---
name: socai-social-research
display_name: 多平台社媒调研-Socai
display_name_en: Social Media Research (Socai)
description: >
  使用本机 socai CLI 和用户已登录的 Chrome 调研小红书、抖音、TikTok、Instagram、LinkedIn 与 X/Twitter 的公开帖子、评论、账号和趋势，并返回带来源链接的洞察。
  当用户要做社媒聆听、消费者洞察、竞品研究、达人或公司研究、帖子与评论取证、趋势验证或跨平台比较时使用；不要用于普通网页搜索、内容创作或发布。
  触发词：小红书、XHS、RedNote、Redbook、抖音、Douyin、TikTok、Instagram、LinkedIn、X、Twitter、社媒聆听、social listening、消费者洞察、竞品研究。
description_zh: 使用本机 Socai 与已登录的 Chrome 调研六个社交平台的公开帖子、评论、账号和趋势，输出可溯源的跨平台洞察。
description_en: Researches public posts, comments, creators, companies, and trends across Xiaohongshu, Douyin, TikTok, Instagram, LinkedIn, and X/Twitter through the local Socai CLI and authenticated Chrome session.
category: writing
version: 0.5.1
author: socai
allowed-tools: Bash,Read,Write,Glob,Grep
---

# Socai 多平台社媒调研

使用 Socai 作为浏览器执行层，交付有来源支持的研究结论，不要只返回命令输出或原始 JSON。

## 平台路由

所有平台都是一等入口。根据用户用词选择对应站点，不要把多平台需求缩成小红书单平台。

| 用户用词 | CLI 站点 | 可调研内容 |
| --- | --- | --- |
| 小红书、XHS、RedNote、Redbook | `socai xhs` | 笔记、博主、正文、评论、图片与可用的视频转写 |
| 抖音、Douyin | `socai dy` | 视频、作者、描述与可用评论 |
| TikTok | `socai tiktok` | 视频、作者、描述与可用评论 |
| Instagram、IG | `socai instagram` | 账号、帖子、文案与可用评论 |
| LinkedIn、领英 | `socai linkedin` | 人物、公司、帖子与搜索结果 |
| X、Twitter | `socai x` | 账号、帖子、线程与可用回复 |

用户明确点名多个平台时逐个平台查询。用户说“全部平台”时覆盖六个平台，并如实报告各平台的登录、验证、空结果或部分结果状态。用户只说“社交媒体”时，根据受众和问题选择相关平台，在综合结论前说明实际覆盖范围，不得暗示检查过未查询的平台。

## 调研前检查

1. 运行 `socai version --no-check --json`，确认 CLI 可用。本技能要求本地 shell、已安装的 Socai CLI 和可连接的 Chrome。如果命令不存在，不要自行下载安装；说明前置条件并引导用户查看 `https://socai.io/docs/installation/`。
2. 运行 `socai status --json`。首次出现 `DAEMON_UNAVAILABLE` 或 `BROWSER_NOT_CONNECTED` 时，继续执行最小的只读查询让 Socai 初始化浏览器。查询返回需要登录、权限、验证码、限流等明确阻断时停止并原样说明，不能声称已经采集数据。
3. 运行 `socai task begin --help` 检查任务登记能力。支持时使用 Write 工具把用户原始问题写入任务专用的临时 UTF-8 JSON 文件，格式为 `{"user_prompt":"用户原始问题","agent_host":"workbuddy"}`，再执行 `socai task begin --context-file <临时文件路径>` 一次。禁止把用户原话直接拼进 shell 命令；无论登记成功或失败，都要立即删除且只删除这个临时文件。旧版本不支持时跳过，任务登记失败不能阻断调研。
4. 针对相关平台阅读 @references/commands.md，并以本机 `socai <site> --help` 和 `socai <site> <command> --help` 为最终准则。

## 执行规则

- 默认仅运行搜索、主页、帖子、评论和页面状态等只读命令。
- 同一个 daemon 的浏览器命令按顺序执行，不并发排队。用户指定的数量和深度优先；未指定时先轻量获取索引，再选择需要的正文，不默认采集评论、OCR 或视频转写。
- 小红书探查可使用 `--preview`；不需要评论时显式传 `--num-comments 0`，具体参数以当前命令帮助为准。大结果保存为任务独立文件，按需读取相关记录，不把整批原始 JSON 塞进上下文。
- 连接超时或反复断开时读取 [连接排查](references/connection-troubleshooting.md)。先核对日志、PID 和实际监听端口，不自动重发超时命令、不停止 daemon、不切换 profile 或重启 Chrome。
- 只查询需求相关的平台，保留结构化 JSON 直到完成综合分析。
- 将 `login_required`、`challenge_required`、`rate_limited`、空结果、部分结果和超时作为证据限制写入结果。
- 引用 Socai 返回的原始帖子或主页 URL；区分事实观察与推断，不补造指标、作者、日期或评论。
- 本市场技能严格只读。禁止运行发布、点赞、关注、互动、评论、回复、私信、连接、上传、删除或其他账号写操作，即使用户提出要求，也要说明已安装的研究技能不提供账号变更。

## 交付

输出与决策相关的结论、实际平台覆盖、代表性来源链接、可用的采集时间，以及登录、验证或样本不足等限制。面向非技术用户时不写命令执行过程，除非运行限制会直接影响结论可信度。
