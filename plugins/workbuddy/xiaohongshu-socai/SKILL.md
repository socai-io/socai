---
name: xiaohongshu-socai
display_name: 小红书调研-Socai
display_name_en: Xiaohongshu Research (Socai)
description: >
  用 socai 驱动本机已登录的 Chrome 真实浏览小红书，完成关键词搜索、博主主页扫描、笔记正文与评论采集、图片 OCR 与视频转写，返回带 xsec_token 可溯源链接的结构化结果。
  当用户要研究小红书上的话题、选题、竞品、口碑、达人、评论区观点，或询问某篇笔记、某个博主在小红书上的情况时使用；也用于采集小红书选题与文案素材。
  触发词：小红书、红书、RedNote、xhs、xiaohongshu、小红书笔记、小红书博主、种草、小红书评论。
description_zh: 在本机已登录的 Chrome 中真实浏览小红书，完成关键词搜索、博主主页分析、笔记正文与评论采集，返回可溯源的结构化结果。
description_en: Drives your signed-in Chrome to research Xiaohongshu (RedNote) — keyword search, creator profiles, note bodies and comments, image OCR and video transcription — returning structured, source-linked results.
category: writing
version: 1.1.0
author: socai
allowed-tools: Bash,Read,Write,Glob,Grep
---

# 小红书调研（socai）

## 这个技能做什么

socai 不是爬虫，也不调用逆向 API。它接管用户本机**已经登录的 Chrome**，像真人一样打开小红书页面：搜索、点开笔记、展开评论、读博主主页、识别图片文字、转写视频语音，然后把结构化结果和原始素材落盘。

能力边界要清楚：

- 能读：搜索结果、笔记正文、评论与回复、博主主页与作品列表、图片 OCR、视频转写、媒体下载。
- 能写：发评论、发笔记（默认关闭，见「写操作红线」）。
- 不能：绕过登录、验证码、风控；不能在无 Chrome 的环境的运行。

## 前置检查（每次任务开始时做一次）

先确认命令存在，再确认浏览器就绪。两步都必须在执行研究命令之前完成。

```bash
command -v socai || echo "NOT_INSTALLED"
socai status --json
```

`socai status --json` 的关键字段：

| 字段 | 含义 | 期望值 |
|---|---|---|
| `browser_state` | 浏览器连接状态 | `connected` |
| `browser_connected` | 是否已连上 | `true` |
| `profile_mode` | 当前 Chrome 档案模式 | `existing` / `managed` / `remote` |
| `error_code` | 失败原因码 | `null` |
| `next_step` | 官方给出的下一步动作 | `null` |

### socai 未安装

`socai` 不在 PATH 时，不要猜测替代方案，直接给用户安装命令并说明它需要 Chrome：

macOS：

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

Windows PowerShell：

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'; Invoke-WebRequest -UseBasicParsing https://github.com/socai-io/socai/releases/latest/download/install.ps1 -OutFile $installer; Unblock-File $installer; & $installer
```

安装后需要重新打开终端让 PATH 生效，然后重跑 `socai status --json`。

### 浏览器未就绪

按 `error_code` 对应处理，不要重试同一条命令硬碰：

- `BROWSER_PERMISSION_REQUIRED` — 用户需在系统设置里允许 Chrome 数据访问与远程调试，确认后重试一次。
- `BROWSER_ENDPOINT_UNREACHABLE` — 启动一个受支持的 Chrome 窗口，或切到托管档案：`socai config set chrome.profile managed` 然后 `socai stop`。
- `REMOTE_SESSION_UNAVAILABLE` — 托管云端浏览器不可用，改回 `existing` 或稍后再试。
- 其它 — 照 `next_step` 字段里给的动作执行。

首次连接时 Chrome 会弹权限确认框，**必须由用户手动点确认**，这一步无法自动化。提醒用户去看屏幕。

## 核心工作流

### 任务登记（首次研究命令前）

每个新任务先调用一次 `socai task begin`，传入用户原始问题，
具体输入方式见 [任务上下文登记](references/task-context.md)。随后照常运行站点命令，
daemon 会自动把它们归入当前任务，不需要额外参数。新的用户需求再次登记；
同一任务的搜索、深挖和重试不重复登记。原话不可得时传 `null`，不要用摘要代替。
旧 CLI 不支持登记或登记失败时继续研究，不要因此阻断用户任务。

### 第一步：选入口

| 用户想要 | 用哪条命令 |
|---|---|
| 某个话题、品类、关键词下的讨论 | `socai xhs search "<关键词>"` |
| 只要结果卡片（标题/点赞/封面），不点开笔记 | 上面的命令加 `--preview` |
| 某个具体博主的资料和作品 | `socai xhs author <author_id>` |
| 重新读取之前已知的若干篇笔记 | `socai xhs get-notes --note <id>=<token>` |

`author` 需要 `author_id`（`/user/profile/<id>` 的最后一段）。用户只给了昵称时，先用一次聚焦的 `search` 找到它，或直接向用户要主页链接，不要凭空拼 id。

### 第二步：定深度

- `--num-notes N` — 采多少篇。`search` 默认 10。需要更广证据才调大。
- `--num-comments N` — 每篇读多少条评论，默认 8，回复计入 N。**只有在问题本身就是关于评论讨论时才调大**；N ≤ 12 通常不用额外滚动，更大的值会显著变慢。
- `--preview` — 只拿卡片，快很多。用于先探一眼再决定深挖谁。
- `--ocr` — 读图片里的文字（本地离线 OCR）。图里才有关键信息时开。
- `--transcribe-audio` — 转写视频语音。只有用户明确要视频内容时开，且需要已登录并选中 socai agent。
- `--download-media` — 把图片视频下载到 run 目录。用户要本地文件时开。

`--filter` 是搜索页筛选器，可重复传，取值见 @references/cli-reference.md。

### 第三步：执行并落盘（必做）

**不要把 socai 的输出直接读进上下文。** 一篇笔记的完整记录可达数十 KB，十篇就是几百 KB，会撑爆上下文。必须重定向到文件：

```bash
socai xhs search "无糖茶 选购" --num-notes 10 --num-comments 8 > /tmp/xhs-search.json 2>/tmp/xhs-search.err
echo "exit=$?"; cat /tmp/xhs-search.err
```

`run_dir` 打在 stderr，进度也走 stderr，stdout 只有结果 JSON。命令失败时错误信息也在 stderr。

### 第零步：定位技能目录

下面的辅助脚本就装在这个技能包里。**先跑一次把路径解析出来，本次任务后续直接复用**：

```bash
SKILL_DIR=$(ls -d ~/.workbuddy/skills/xiaohongshu-socai 2>/dev/null \
  || find "$HOME" ~/.workbuddy ~/.claude ~/.codebuddy -maxdepth 4 -type d \
       -name xiaohongshu-socai -not -path '*/node_modules/*' 2>/dev/null | head -1)
echo "$SKILL_DIR"
```

找不到就说明技能没装好，提示用户重装。**不要把 `<skill-dir>` 当成真路径去执行。**

### 第四步：读结果

先看整体，再决定要不要深挖：

```bash
python3 "$SKILL_DIR/scripts/summarize.py" /tmp/xhs-search.json
```

这个脚本把每篇笔记压成一行（标题、作者、日期、点赞、评论数、URL），几十篇也只占几 KB。拿到摘要后再用 Read 或 Grep 精确取某篇的正文。

需要某篇的完整内容时：

```bash
python3 "$SKILL_DIR/scripts/summarize.py" /tmp/xhs-search.json --note-id '<note_id>'
```

字段含义、输出结构、错误码处理见 @references/output-schema.md。

### 第五步：向用户交付报告（每次必做）

**跑完必须交付一份报告，把材料甩给用户看 raw JSON 或 run 目录是没交付。** 这一步不是可选的。

**写到哪儿**——当前工作目录下的 `socai-reports/`，用户点开就能找到：

```bash
mkdir -p ./socai-reports
```

**默认交付 Markdown**，文件名 `<主题>-<日期>.md`，结构套 @templates/report-outline.md：

```bash
# socai-reports/无糖茶研究-2026-09-28.md
```

**来源超过 8 条时，额外导出一份 CSV**，让用户能自己筛、排序、贴进表格：

```bash
python3 "$SKILL_DIR/scripts/export_csv.py" /tmp/xhs-search.json \
  -o ./socai-reports/无糖茶研究-2026-09-28.csv
```

**用户要别的格式就换**——HTML 单页、Excel（.xlsx）、Word（.docx）、PPT 都可以，按用户指定的来。用户没说就给 Markdown，不要自作主张加一堆格式。

**交付动作：写完必须在对话里把报告交到用户手上。** 用 present_files 传报告文件的绝对路径，让用户在结果面板里直接预览或点开；同时用一两句说明报告的核心结论。不要只在回复里提一句"报告在某个目录下"。

多份产物时按重要性排序一起交付：Markdown 报告在前，CSV 在后。

写作约束见 @templates/report-outline.md 末尾「写作约束」。

## 命令速查

```bash
# 关键词搜索（默认点开每篇笔记读正文+评论）
socai xhs search "露营新手装备" --num-notes 10 --num-comments 8

# 只看结果卡片
socai xhs search "露营新手装备" --num-notes 30 --preview

# 带筛选器
socai xhs search "上海周末去处" --filter publish_time=一周内 --filter note_type=图文 --filter sort=最新

# 博主主页（含认证信息、粉丝数、作品列表）
socai xhs author 6382aaca000000001f017dc4 --num-notes 20

# 重读已知笔记（token 来自上面两条命令的返回）
socai xhs get-notes --note '<note_id>=<xsec_token>' --num-comments 20

# 需要图片文字时
socai xhs search "健身房年卡价格" --ocr

# 需要本地素材时
socai xhs search "露营装备" --download-media --num-notes 5
```

完整参数表见 @references/cli-reference.md。

## 研究策略

小红书搜索是**相关性排序，不是时间排序**，一页结果里常常混着几个月甚至几年前的笔记。因此：

1. 引用任何笔记前先核它的 `published`（`at` 为带 `+08:00` 偏移的精确发布时间，`date` 为北京日期；`precision` 为 `day`/`unknown` 时只有日期）或兼容字段 `date`，别把旧内容当现状。
2. 问题涉及**由主体自己宣布或决定的事**（营业时间、价格、活动、规则、开闭店），且结果里出现了看起来像官方账号的博主时，不要只凭搜索样本下结论——用 `socai xhs author <author_id>` 进主页确认认证状态和最新公告。主页按时间倒序，搜索排序漏掉的新公告会在这里。
3. 问题没有权威主体（体验、观点、推荐）时，用户笔记本身就是证据，不需要跳主页。
4. 样本里出现 `already_analyzed`、`history` 等标记，说明 socai 有该笔记的历史缓存证据，直接用，别当它是被跳过了。

选题、竞品、口碑等常见研究套路的完整打法见 @references/research-playbook.md。

## 错误处理

- `reason:"login_required"` — **不要重试**。让用户在浏览器里扫码登录小红书，登录好后再跑一次。
- `reason:"rate_limited"` — 触发平台限流。换更窄的查询，或等一会儿再试，不要同参数反复重试。
- 安全验证 / 验证码 / "帖子不见了" / "内容无法展示" — 平台或会话级阻断，用已采到的证据作答并如实说明缺口，不要循环重试。

## 写操作红线

`socai xhs comment`、`prepare-publish`、`commit-action` 会**真实改动用户的小红书账号**。遵守：

1. 只在用户明确指定"给这篇笔记发这条评论"或"发这篇笔记"时才执行，绝不推断、绝不批量。
2. 执行前把**目标笔记和完整内容**复述给用户并得到确认。
3. `comment` 必须传 search/author 返回的完整 URL（带 `xsec_token`），裸 note_id 会被拒绝。
4. 返回 `commit_unknown` **不是重试许可**。如实告知结果不确定，重新读一次笔记再决定。

默认所有任务都是只读研究。用户没说要发，就不发。

## 证据规则

- 引用来源时**原样复制结果里的 `url`**，不要拿 `note_id` 自己拼。不带 `xsec_token` 的 `xiaohongshu.com/explore/<id>` 在桌面端打不开。
- **`xsec_token` 必须复制粘贴，不能手打。** 它是一串长度 40 以上的随机串，肉眼校对不出串行，手打过的实测会写错字符、链接直接废掉。这是顺手的事，不需要事后专门复核。
- 区分笔记正文、评论、博主资料、互动数据、图片 OCR 这五种证据，别混为一谈。
- 结果不完整就如实说明缺了什么，不要假装数据已采集。
- 用用户提问所用的语言回答。

## 交付检查清单

收工前逐条对照，任何一条没做到都要补：

1. 报告文件已写到 `./socai-reports/`，不是停留在想法里。
2. 来源超过 8 条时 CSV 也导出了。
3. 用 present_files 把报告绝对路径交给了用户，用户可以直接预览。
4. 回复里讲了核心结论，不是只丢一个路径。
5. **结论**附带了能打开的原始链接；**线索和猜想**单独成节并明确标注，不因为没有出处就被删掉。
