# socai 的 WorkBuddy 生态包

WorkBuddy / CodeBuddy 市场用的技能与专家包。每个目录是一个可独立上架的单元。

| 包 | 类型 | 上架位置 | 状态 |
|---|---|---|---|
| `xiaohongshu-socai` | 技能 | 【技能】→【添加技能】 | 就绪 |
| `xiaohongshu-research-expert` | 专家 | 【专家】→【我的专家】→【创建专家】 | 就绪 |

专家包内联了技能包（打包时自动注入），所以只上专家也能用；技能单独上架则覆盖所有 agent 场景。

## 打包

```bash
cd plugins/workbuddy
./build.sh              # 全部
./build.sh skill        # 只打技能
./build.sh expert       # 只打专家
```

`build.sh` 会先做规范校验再打包，不合规直接失败：

- 技能：`SKILL.md` frontmatter 必填 `description` / `description_zh` / `description_en` / `version` / `author`
- 专家：`plugin.json` 必填 16 个字段、`displayDescription.zh` 必须 40–50 字、`tags` 与 `quickPrompts` 各正好 3 条、`defaultInitPrompt` 必须等于 `quickPrompts[0]`、头像 ≤500KB

产出在 `dist/`（不进 git，随用随打）：

- `dist/xiaohongshu-socai.zip`
- `dist/xiaohongshu-research-expert.zip`

## 目录约定

```
plugins/workbuddy/
├── build.sh
├── dist/                             # 构建产物，gitignored
├── xiaohongshu-socai/                # 技能源码（唯一真源）
│   ├── SKILL.md
│   ├── references/
│   ├── scripts/
│   └── templates/
└── xiaohongshu-research-expert/      # 专家源码
    ├── .codebuddy-plugin/plugin.json
    ├── agents/xiaohongshu-research-expert.md
    ├── avatars/expert.png
    ├── README.md
    └── skills/                       # 构建产物，勿手工编辑
```

**技能源码只在 `xiaohongshu-socai/`。** 专家目录下的 `skills/` 由 `build.sh` 从源码拷贝，直接改它会在下次打包时被覆盖。

### 技能结构

```
<skill-name>/
├── SKILL.md           # 必须，YAML frontmatter + 指令正文
├── references/        # 可选，SKILL.md 用 @references/xxx.md 引用
├── scripts/           # 可选，由 Bash 执行
└── templates/         # 可选，可复用模板
```

SKILL.md 正文控制在 5000 词以内，细节一律下沉到 `references/`。

### 专家结构

```
<expert-name>/
├── .codebuddy-plugin/plugin.json    # 运行配置 + 市场展示信息
├── agents/<name>.md                 # 系统提示词（YAML frontmatter + 正文）
├── avatars/expert.png               # 512×512，≤500KB
└── skills/                          # 可选，内联技能
```

行业分类见[专家文档](https://open.workbuddy.cn/docs/expert)第九节。小红书调研归 `05-MarketingGrowth`（营销增长）——**不要选 `02-Engineering`**，WorkBuddy 的用户群是办公人群，归到技术工程会掉进没人逛的分类。

## 两个辅助脚本

都在 `xiaohongshu-socai/scripts/`，由 SKILL.md 指导下调用：

| 脚本 | 用途 |
|---|---|
| `summarize.py` | 把结果 JSON 压成单行摘要，或直接取某篇全文。避免几十 KB JSON 进上下文 |
| `export_csv.py` | 导出来源清单 CSV（utf-8-sig BOM，Excel 打开不乱码）。依赖同目录的 `summarize.py` |

## 交付约定

技能必须向用户交付报告，不能只丢一个 run 目录。默认 Markdown 写到 `./socai-reports/`，来源超过 8 条时额外导出 CSV，用户点名要别的格式（HTML / xlsx / docx / pptx）就按指定来，并且必须用 present_files 把报告绝对路径交回给用户。

## 上架路径

**技能**：WorkBuddy 左侧【专家·技能·连接器】→【技能】→ 右上角【添加技能】→【创建技能】，按提示提交 zip。

**专家**：同一入口 →【专家】→【我的专家】→【创建专家】，补全创建提示词后上传 zip。

解析失败时对照开放平台文档的「基础结构 / 配置文件」排查；仍失败则邮件 `openworkbuddy@tencent.com`，或扫开放平台首页二维码进开发者群。

## 改技能后务必重新打包

技能源码更新不会自动同步到专家包。改完跑一次 `./build.sh`，两个 zip 一起刷新。
