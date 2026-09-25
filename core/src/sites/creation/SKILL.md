---
name: create-site-skill
description: 为 socai 新增或维护按域名发现的站点 skill；用于页面探索、DOM browser tool、站点知识沉淀以及确有必要的 native 工具接入。
---

# Socai site skills

Socai 的浏览器/CDP runtime 是稳定执行面。站点能力是独立的 learning package，由 `manifest.json` 自描述并在运行时按 id 或域名发现；不要从其他平台复制 Rust 目录结构，也不要把 `NativeSiteAdapter` 当作 capability manifest。

参考语义：

- browser-use：通用 runtime 不内置站点做法；进入域名后才发现并读取匹配的 domain skill。
- ego-lite：manifest 声明 domains、notes 和 browser tools；notes 与脚本按需加载，路径必须留在 package 内。
- Socai：内置 package 在构建时从 `core/src/sites/*/manifest.json` 自动收集，发布时嵌入二进制；`$SOCAI_SITE_SKILLS_DIR/<id>` 或 `$SOCAI_HOME/site-skills/<id>` 中的完整同名 package 可在运行时覆盖内置版本。

## Package contract

`manifest.json` 是唯一必需入口：

```json
{
  "id": "example",
  "name": "Example",
  "domains": ["example.com", "*.example.com"],
  "notes": ["knowledge.md"],
  "browserTools": {
    "readPage": {
      "description": "Read the active page.",
      "path": "page-tools.js",
      "binding": "window.ExamplePageTools",
      "callable": "readPage",
      "args": {},
      "returns": {"type": "object", "description": "Page state."}
    }
  }
}
```

- `id` 必须和 package 目录名一致。
- `domains` 只写 hostname；根域名和 `*.subdomain` 分开声明。
- `notes` 是需要按需加载的 Markdown 资源列表。已有 `knowledge.md` 必须保留；没有验证过的新知识时允许保持为空。
- `browserTools` 是页面上下文能力表。每项声明 description、相对 path、args、returns；现有 IIFE bundle 用安全的 binding + callable，独立脚本也可以直接导出一个匿名 async function。
- 所有资源路径必须是 package 内的相对路径，不允许绝对路径、反斜杠、`..` 或符号链接逃逸。
- manifest 没有声明的 browser tool 不可执行。

文件名和 Rust 模块布局不是协议。一个 package 可以按页面、工作流或维护边界组织多个 notes/scripts，也可以只有一个脚本。不要要求每个平台都具备 `tools.rs`、`page.rs`、`page_scripts.js`、`entities.rs` 或相同依赖方向。

Agent 通过三个通用工具消费 package，而不是为平台注册一套固定 host 工具：

- `navigate_site`：只进入已安装 skill 覆盖的 HTTPS 域名，并在跳转后返回匹配的 notes 与 tool schemas；
- `read_site_skills`：按当前页面真实 hostname 重新发现和读取 skill；
- `run_site_browser_tool`：按 `site_id + tool_name` 执行 manifest 声明的页面工具，同时校验当前域名、输入 schema 和返回类型。

XHS、Douyin、TikTok 与后续平台都走同一个发现入口；平台差异只存在各自 package 的 manifest、notes 和脚本内容中。

## Native adapter boundary

只有能力必须使用已编译 Rust 组件时才增加 native adapter，例如：

- 多轮 CDP 状态机、长等待和快照编排；
- 媒体下载、OCR、ASR；
- 需要成为 `socai <site> <command>` 的稳定 CLI/daemon 工作流；
- 多个 native 工具共享的强类型结果。

`NativeSiteAdapter` 仅绑定 Rust 函数指针、agent tool factory 和 CLI command handler。它不是发现入口，不声明 domains/notes/browser scripts，也不决定 package 文件结构。纯 DOM 能力应只增加 manifest browser tool，不创建空 Rust 模块。

## Development loop

1. 先确认目标 URL、用户操作流程、输出字段、登录状态与成功条件。
2. 查看当前页面真实 snapshot：截图、a11y tree 和精简 DOM。禁止凭其他平台的 selector 或 class 猜测。
3. 只实现下一项最小页面动作或提取，并在 manifest 中声明对应 browser tool。
4. 运行当前半成品命令并开启 `--debug-snapshot`：
   ```bash
   cargo run -p socai-cli -- <site_id> <command> ... --debug-snapshot
   ```
5. 检查最新 snapshot 与 JSON 结果。状态不符合预期时先修复当前动作；验证通过后再增加下一步。
6. 循环直到搜索、详情、评论、作者或媒体流程达到明确成功条件。

返回给模型的结果形状，以及读取或关闭前要等多久，见同目录 [best-practices.md](./best-practices.md)。

开发时如需避免反复确认 remote debugging，可临时使用 managed profile：

```bash
cargo run -p socai-cli -- config set chrome.profile managed
cargo run -p socai-cli -- stop
```

完成后恢复默认：

```bash
cargo run -p socai-cli -- config unset chrome.profile
cargo run -p socai-cli -- stop
```

## Validation

- 使用 3–5 组真实参数运行完整流程，不只做编译检查。
- 确认 manifest id/domains、资源路径和所有 browser tool callable。
- 对每次页面跳转验证真实 URL、页面状态和内容身份。
- 请求的详情、评论、媒体或 ASR 不完整时必须返回明确错误，不得伪装成功。
- 运行 core/CLI/desktop 检查、页面脚本语法检查和 `git diff --check`。

## Learning update

完成真实页面验证后，才把跨任务仍然有效、且无法由 tool schema 表达的页面知识写入 manifest 已列出的 note。不要复制 schema，不记录一次性 snapshot ref、会话数据、账号信息或未验证猜测；没有新知识时保留现有 `knowledge.md` 原样。
