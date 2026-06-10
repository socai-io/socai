这份文档指导你如何添加一个新网站能力，或者在已有网站里添加新能力。

socai是一个通用browser use agent，每个网站沉淀了自己特定的操作在sites下面的文件夹，从而能更快、更准、更省token地操作成熟流程。

每个网站目录包含以下几类文件：
- tools.rs：定义了网站的工具函数，接口层，可以是偏底层的原子操作，也可以有偏工作流的连续操作，便于完成用户高频流程。工具实现 `Tool` trait（`core/src/agent/tool.rs`）：`name`/`description`/`input_schema`/`call`，返回 JSON 文本。description 里写清楚成本与耗时（是否打开页面、是否耗 token/需要本地 ASR 等），agent 靠它做性价比决策。
- page.rs：tools背后的实现，包含业务逻辑和 CDP 操作编排。选择器尽量稳定，优先结构性选择器、`data-*`、aria 属性；不要依赖构建生成的 hash class。
- page_scripts.js：页面内 DOM 逻辑，每次 `run_script()` 时注入页面执行。JS 只做提取，返回 JSON。page_scripts.js 是一个 IIFE，在 window 上挂一个命名函数表（参考 xhs 的 `SocaiXhsPageScripts`）。每个函数接收一个 JSON 参数、返回可序列化的 JSON——不返回 DOM 节点、不在 JS 里做多步编排。点击/滚动/等待等编排都在 Rust 侧（page.rs），通过 `run_script(name, arg)` 注入调用并校验结果。
- entities.rs：定义网站数据类型
- knowledge.md：关于这个网站的know-hows，包括工具信息、网页功能、布局、页面动态、跳转、登录、风控等信息，便于后续agent快速上手
- mod.rs：模块声明和export
- 其他文件可以按需增加

接线：
1. 在站点 tools.rs 里定义 `pub static <ID>_SITE: SiteSpec`（参考 xhs 的 `XHS_SITE`）：声明 id、home_url、agent 工具工厂、instructions，以及要暴露成 CLI/daemon 命令的 `SiteCommand` 列表（含参数声明）。
2. 在 `core/src/sites/mod.rs` 声明模块，并把它加进 `core/src/sites/registry.rs` 的 `SITES` 数组。

完成后 `socai <site_id> <command>` 子命令、daemon 分发、TUI/桌面端的工具注册全部自动生效，不需要改 cli/ 或 app/ 的代码。

当用户想要增加新网站或新能力时，遵循以下步骤：
1. 首先，让用户给出网页url、并尽可能详细地描述每一步的操作流程、页面布局、如何操作按钮等等，比如点击还是悬浮鼠标、在哪个区域上下滑滚轮，是否有键盘按键能更准确地操作（比如上下左右键切换帖子、esc键退出当前页面等等）。
2. 然后，如果sites目录下面还没有关于这个网站的文件夹，则增加一个；如果已有，则在它的文件夹下面增加代码。按以下的流程不断循环，一步一步地添加：
  1) 首先，添加下一步单点操作的代码。如果是第一步则是打开网页的操作，如果是后面的步骤，则是新增一个鼠标或键盘操作或页面js操作等等，可以封装成工具，也可以多个步骤最后一起封装。支持上--debug-snapshot参数，保存每步变化的snapshots。
  2) 然后，你调用目前代码已有的所有操作，比如 `cargo run -p socai-cli -- <site_id> <command> ... --debug-snapshot`。它会开始操作Chrome，每次命令在 stderr 打印 `run_dir`，snapshots 保存在 `<run_dir>/snapshots/`（位于 ~/.socai/runs/ 下）。
  3) 等代码操作完成，你查看最新一步的增量snapshot，包括DOM和截图，对截图做多模态理解。确认当前操作是否符合预期。如果符合预期，则继续实现下一步的代码。如果不符合，说明上一步的代码有问题，你排查snapshot，修改上一步的代码。
3. 重复以上3步循环，直到新加的工具能完成用户的需求。当用户首次描述不清楚的时候，你可以在上面的开发和操作过程中和用户交互对话，来理清楚操作细节。
4. 把工具信息和发现的有价值的网站知识写到knowledge.md

注意事项：
- 确保你是用daemon启动的，这样调试过程中能复用CDP连接。首次运行时连接Chrome会弹窗，用户需要确认allow debugging。如果用户没有确认导致超时，请提醒用户。
- 可以参考sites目录下已有的其他网站代码
- 如果网站需要登录，提醒用户需要先登录。
