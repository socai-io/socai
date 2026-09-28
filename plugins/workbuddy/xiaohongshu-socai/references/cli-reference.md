# 小红书命令完整参考

所有命令形如 `socai xhs <command> [options]`。结果是 JSON，打到 stdout；`run_dir` 和进度打到 stderr。
每条命令都额外支持 `--pretty`（美化输出）和 `--debug-snapshot`（把 DOM、无障碍树、截图存到 run 目录，仅排障用）。
每个新任务先完成一次[任务登记](task-context.md)，之后照常调用下列命令，daemon 自动关联。

> 参数带引号的值如果含空格或特殊字符，务必用单引号包裹。

---

## search — 关键词搜索

```bash
socai xhs search <QUERY> [options]
```

`<QUERY>` 是位置参数，必填。

| 参数 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `<QUERY>` | 字符串 | 必填 | 搜索词 |
| `--num-notes N` | 整数 | 10 | 采集多少篇，会滚动信息流直到够数。省略则取固定 10 篇，不是"只要第一页" |
| `--num-comments N` | 整数 | 8 | 每篇加载多少条评论，回复计入 N。`0` 跳过评论。`--preview` 下忽略 |
| `--preview` | 开关 | 关 | 只返回结果卡片（标题/点赞/封面），不点开笔记 |
| `--filter group=option` | 键值 | 无 | 搜索页筛选器，可重复传 |
| `--download-media` | 开关 | 关 | 下载图片视频到 run 目录并带上 `local_path`。`--preview` 下忽略 |
| `--ocr` | 开关 | 关 | 本地 OCR 每篇的每张轮播图（视频笔记读封面）。`--preview` 下改为 OCR 每张卡片的封面 |
| `--transcribe-audio` | 开关 | 关 | 下载视频并转写语音。`--preview` 下忽略 |

### --filter 可用取值

值必须照抄小红书页面上的中文文案：

| group | 可选值 |
|---|---|
| `sort` | 综合、最新、最多点赞、最多评论、最多收藏 |
| `note_type` | 不限、视频、图文 |
| `publish_time` | 不限、一天内、一周内、半年内 |
| `search_scope` | 不限、已看过、未看过、已关注 |
| `distance` | 不限、同城、附近 |

```bash
socai xhs search "上海周末活动" --filter publish_time=一周内 --filter note_type=图文 --filter sort=最新
```

---

## author — 博主主页扫描

```bash
socai xhs author <AUTHOR_ID> [options]
```

| 参数 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `<AUTHOR_ID>` | 字符串 | 必填 | `/user/profile/<id>` 的最后一段 |
| `--num-notes N` | 整数 | 10 | 滚动主页网格采集多少篇 |
| `--num-comments N` | 整数 | 8 | 同上，`0` 跳过，`--preview` 下忽略 |
| `--preview` | 开关 | 关 | 只返回作品卡片，不点开笔记 |
| `--download-media` / `--ocr` / `--transcribe-audio` | 开关 | 关 | 同 search |

返回里一定包含 `profile` 对象：认证状态（企业认证 / 个人认证）、简介、IP 属地、粉丝/关注/获赞收藏数、作品卡片列表。

`author_id` 只给了昵称时用一次聚焦的 `search` 先找到它，或直接向用户要主页链接。

---

## get-notes — 重读已知笔记

```bash
socai xhs get-notes --note '<note_id>=<xsec_token>' [--note ...] [options]
```

| 参数 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `--note ID=TOKEN` | 字符串 | 必填 | 可重复传，一次批量读多篇 |
| `--num-comments N` | 整数 | 8 | 同上 |
| `--download-media` / `--ocr` / `--transcribe-audio` | 开关 | 关 | 同 search |

**必须传 xsec_token**，且只能用 `search` 或 `author` 已经返回过的 token。裸 note_id 会被拒绝——直接打开详情页容易触发小红书风控。

```bash
socai xhs get-notes \
  --note '6909c2ff0000000003012cb4=ABCxYz...' \
  --note '6910aa11000000000301abcd=DEFxYz...' \
  --num-comments 20
```

---

## 写操作命令

需要用户显式确认后才可调用，详见 SKILL.md「写操作红线」。

```bash
# 发一条评论（必须传完整 URL，保留 xsec_token）
socai xhs comment '<完整笔记URL>' --text '要发的评论原文'

# 发笔记：先备稿，拿到 action_id，再确认提交
socai xhs prepare-publish --media ./a.jpg --media ./b.jpg --title '标题' --text '正文'
socai xhs commit-action --action-id '<action_id>'
socai xhs reconcile-action --action-id '<action_id>'   # 提交结果不确定时核对
```

| 命令 | 参数 | 约束 |
|---|---|---|
| `comment` | `<NOTE_URL>`（位置参数）、`--text TEXT`、`--wait-seconds N`（默认 30） | URL 必须带 `xsec_token`；已有草稿或已存在完全相同评论时会拒绝 |
| `prepare-publish` | `--media PATH`（可重复，1-18 张，JPG/JPEG/PNG/WebP）、`--title`（1-20 字）、`--text`（1-1000 字） | 只备稿不发布，返回 `action_id` |
| `commit-action` | `--action-id` | 校验后只点一次发布按钮 |
| `reconcile-action` | `--action-id` | 读取笔记管理器核对发布结果 |

---

## 环境与配置命令

| 命令 | 用途 |
|---|---|
| `socai status --json` | 隐私安全的 CLI 与浏览器就绪状态，不连接 Chrome |
| `socai version` | 已装版本与最新 release 对比 |
| `socai update` | 升级 macOS release 二进制安装 |
| `socai config get` / `set <key> <value>` / `unset <key>` / `list` / `path` | 读写持久配置 |
| `socai stop` | 停掉后台 daemon |

常用配置：

```bash
socai config set chrome.profile managed   # 用 ~/.socai/chrome-profile 隔离档案，需登录一次
socai config set chrome.profile existing  # 回到日常 Chrome（默认）
socai config set runs.dir "$(pwd)/xhs-runs"  # 改 run 目录
```

改了 `chrome.*` 之后若 daemon 已在跑，要执行一次 `socai stop` 让新设置生效。

## run 目录

每次运行的产物默认落在 `~/.socai/runs/<时间戳>_<任务>/`：

- 结构化结果与 run 元数据
- 搜索、笔记、博主数据
- 下载的图片视频与 OCR 输出
- `media_manifest.json` 媒体清单
- agent 生成的报告、表格等交付物

`run_dir` 会打在 stderr，也在结果 JSON 里。
