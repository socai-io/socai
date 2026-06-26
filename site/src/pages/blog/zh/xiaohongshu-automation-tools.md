---
layout: ../../../layouts/BlogPost.astro
lang: zh
title: "小红书自动化会被封号吗？我把 7 个开源工具的源码都读了一遍（2026.6）"
description: "小红书自动化会不会被封号？我把 7 个有代表性的开源工具（爬虫、逆向 API、MCP、CDP 浏览器自动化）的源码都读了一遍，讲清楚它们的原理、能力边界、封号风险，以及该怎么选。"
date: 2026-06-07
dateLabel: 2026 年 6 月
readingTime: 约 20 分钟
alternates:
  - hreflang: en
    href: /blog/xiaohongshu-automation-tools
footnote: 这篇记录的是截至 2026 年 6 月的小红书自动化与数据采集开源生态；平台和这些项目都在变，结论可能过时，具体请以源码为准。
---

做小红书的人迟早会问一个问题：能不能让程序帮我自动搜笔记、抓评论、甚至自动发？能，GitHub 上这类工具一抓一大把。但紧接着第二个问题就来了：这么搞会不会被封号？

这两个问题我都想弄明白，干脆把七个有代表性的小红书（XHS / RedNote）自动化开源项目的源码从头到尾翻了一遍。这篇就是翻完之后的笔记：它们到底怎么操作小红书、能做到什么程度、哪种最容易被封，以及我自己在做的 socai 跟它们比是个什么位置。

先放一张总表当地图，按出现时间从新到老排，所以最上面是 socai，往下依次是其它七个工具，最老的在最底下。Star 数是实时的，起始时间是我标的，方便你感受这条时间轴。点项目名能直接跳到它的 GitHub。

| # | 项目 | Stars | 起始 | 语言 | 路线 | 一句话定位 |
|---|---|---|---|---|---|---|
| 1 | [socai-io/socai](https://github.com/socai-io/socai) | ![](https://img.shields.io/github/stars/socai-io/socai?style=flat&label=&color=555) | 2026/6 | Rust | 浏览器(裸 CDP) | XHS 内容研究 + 多模态富集的集成产品 |
| 2 | [autoclaw-cc/xiaohongshu-skills](https://github.com/autoclaw-cc/xiaohongshu-skills) | ![](https://img.shields.io/github/stars/autoclaw-cc/xiaohongshu-skills?style=flat&label=&color=555) | 2026/3 | Python | 浏览器(CDP/插件) | 插件 Bridge + SKILL.md 技能集 |
| 3 | [jackwener/xiaohongshu-cli](https://github.com/jackwener/xiaohongshu-cli) | ![](https://img.shields.io/github/stars/jackwener/xiaohongshu-cli?style=flat&label=&color=555) | 2026/3 | Python | 逆向 API | agent 友好的逆向 API CLI |
| 4 | [white0dew/XiaohongshuSkills](https://github.com/white0dew/XiaohongshuSkills) | ![](https://img.shields.io/github/stars/white0dew/XiaohongshuSkills?style=flat&label=&color=555) | 2026/2 | Python | 浏览器(裸 CDP) | CDP 直连的发布/运营工具 |
| 5 | [xpzouying/x-mcp](https://github.com/xpzouying/x-mcp) | ![](https://img.shields.io/github/stars/xpzouying/x-mcp?style=flat&label=&color=555) | 2025/10 | 插件+云 | 浏览器(插件) | xiaohongshu-mcp 的零部署版（云中继 + 本地插件） |
| 6 | [xpzouying/xiaohongshu-mcp](https://github.com/xpzouying/xiaohongshu-mcp) | ![](https://img.shields.io/github/stars/xpzouying/xiaohongshu-mcp?style=flat&label=&color=555) | 2025/8 | Go | 浏览器(go-rod) | 自托管的 XHS MCP server |
| 7 | [NanmiCoder/MediaCrawler](https://github.com/NanmiCoder/MediaCrawler) | ![](https://img.shields.io/github/stars/NanmiCoder/MediaCrawler?style=flat&label=&color=555) | 2023/6 | Python | 逆向 API* | 多平台数据采集框架（XHS 只是其一） |
| 8 | [ReaJason/xhs](https://github.com/ReaJason/xhs) | ![](https://img.shields.io/github/stars/ReaJason/xhs?style=flat&label=&color=555) | 2023/4 *(停更)* | Python | 逆向 API | 可插拔签名的 HTTP 客户端库，生态鼻祖 |

\* MediaCrawler 现在主要靠逆向 API 取数，但仍然会用浏览器 / CDP 处理登录态和环境。

## 一、小红书自动化只有两条技术路线：逆向 API vs 浏览器自动化

七个工具看着五花八门，有 Python 库、有命令行、有 MCP server、还有浏览器插件。但拆到底，区别只有一个：到底谁去跟小红书的服务器打交道。

一种是工具自己上。把小红书网页里那套请求签名算法用 Python 或 Go 照着重写一遍，自己拼好请求头，用 requests / httpx 这类库直接打小红书的接口。我后面都叫它逆向 API。这条路平时不碰网页，但要拿登录状态、初始化 cookie 或者反检测的时候，可能还得借一下浏览器。

另一种是让浏览器替你上。开一个你已经登录好的真实浏览器，搜索、发请求这些动作都交给网页自己的 JS 去做，签名也是网页算的，工具自己根本不碰。我后面叫它浏览器自动化。

两条路最后都得过小红书的签名加风控这一关，差别只在于签名是谁算出来的：逆向 API 自己算，浏览器自动化让网页算。别小看这一个选择，一个工具能不能规模化、维护起来累不累、容不容易被封、能不能直接跑在你日常用的浏览器里，基本都被它决定了。

### 逆向这条路：签名其实早就被做成了一个公共库

逆向是个没完没了的拉锯战。小红书隔三差五改一次签名算法，逆向的人就得跟着重写一遍。这种又脏又累的活，现在已经被从各个项目里抽出来、变成了一个大家共用的库：[xhshow](https://github.com/Cloxl/xhshow)（作者 Cloxl，MIT 协议）。它把 `x-s`、`x-s-common` 这套签名算法，自定义的 Base64 码表、CRC 变种、把运行环境打包成 JSON、`a3_hash` 等等，用纯 Python 重写了一遍，对外就给你 `sign_headers_get` / `sign_headers_post` 两个函数。jackwener/xiaohongshu-cli 基本就是「xhshow + 反检测 + 一个好看的命令行」；MediaCrawler 现在也把签名外包给它（还反过来给它修过 GET 请求 `a3_hash` 的 bug）。

更早的 ReaJason/xhs 玩法不太一样：它干脆不自带签名，而是把 sign 留成一个构造参数（`XhsClient(cookie, sign=...)`）让你自己传进去，它官方例子是用 Playwright 去调网页里的 `window._webmsxyw` 来签。两种做法其实是同一个意思：逆向这玩意儿太容易过期了，要么大家凑钱养一个公共库，要么干脆甩锅给调用方。

但不管哪种，走逆向这条路的命运是绑死的：小红书一改签名，这一票工具集体歇菜。ReaJason/xhs 和 xiaohongshu-cli 后面维护压力大、最后停更，很大一部分就是这个原因（这里说的「停更」是作者不再加新功能了，还回不回答使用问题就不好说了）。

### 浏览器这条路：可以从三个方面拆开看

很多人会把「用插件」和「用 CDP」当成两种不同的路线，这其实搞混了。先说清楚一件事：

> 插件、CDP、Playwright，这三个不是三条路线，而是浏览器自动化这条路里「怎么控制浏览器」的三种方式。它们都要你真实登录、都靠往页面里注入 JS（evaluate）来干活，能干的事高度重叠。插件能做的（hook fetch、读 cookie、在你真实的浏览器配置里操作），CDP 基本也能做（拦 `Fetch` / `Network`、`Network.getAllCookies`、连到开了调试端口的 Chrome）。真正的区别在于：怎么部署、什么时候挂上去、权限多大、能看到页面多少生命周期。

浏览器这条路真正值得分开看的是三个方面，而且它们互不相干、可以随便组合，拼在一起才是一个项目完整的样子。

第一个方面，用什么去控制浏览器：

| 控制方式 | 代表 | 取舍 |
|---|---|---|
| Playwright | MediaCrawler（早期） | 自带 auto-wait、locator，写起来稳；但要拖一个 Node 加几百兆的 Chromium，重 |
| CDP 直连（裸 ws / go-rod / chromiumoxide） | xiaohongshu-mcp、XiaohongshuSkills、autoclaw、socai | 轻、单个二进制、控制粒度细；但等待、重试都得自己处理 |
| 浏览器插件（MV3） | autoclaw、x-mcp | 天生跑在你日常浏览器里、不用开调试端口；但要让用户装插件、还受应用商店约束 |

Playwright 和 CDP 直连的本质区别就是「包了多厚」：Playwright 在 CDP 之上加了 auto-wait（动作会自动等元素可点）、locator（选择器抗页面重渲染）这一整套为「人写测试」准备的东西，代价是一个 Node 运行时加几百兆绑定版本的 Chromium；CDP 直连只把协议薄薄包一层，换来轻量和对时序的完全掌控。对那种要长期挂着、被反复调用的工具，CDP 直连通常更划算，这也是 xiaohongshu-mcp、socai 都选它的原因。

插件相比 CDP，硬区别不只是能力，更在于「装在哪、能看到多少」：插件不用你拿 `--remote-debugging-port` 重启 Chrome，而且能在 `document_start`、MAIN world、`webRequest` 这些生命周期里一直盯着页面。x-mcp 把这条优势用到了极致（零部署），socai 和 xiaohongshu-mcp 则接受「得开个调试端口」来换「不依赖插件」。

第二个方面，数据怎么读出来（越往后越靠谱）：

1. 直接抓 DOM：用 `querySelector` 一个个抠字段，最直观，但页面一改就废，字段还经常缺。
2. 读页面里现成的状态，`window.__INITIAL_STATE__` 就是小红书服务端渲染时塞进页面的一大坨 JSON，比 DOM 全、也更稳。xiaohongshu-mcp、autoclaw、socai 都优先读这个。
3. 拦网络请求：hook `fetch` / `XHR`（插件在 MAIN world 里，或者用 CDP 的 `Fetch` 域），直接把页面已经签好名的请求响应抓下来，最全最稳。autoclaw 的 `interceptor.js` 就是这么干的。

第三个方面，用谁的浏览器配置（profile）：

- 挂在你日常用的 Chrome 上（真实登录、真实指纹，最不容易被发现）：socai 默认就是这样（`discover_existing_chrome_endpoint`），autoclaw、x-mcp 因为是插件天然如此。
- 自己另起一个独立 Chrome（专用配置目录，要自己扫码登录）：好处是能隔离、能多账号并行。socai（`socai config set chrome.profile managed`）、xiaohongshu-mcp（无头加 cookie 文件）、XiaohongshuSkills（按账号分开）都支持。

### 这些工具是怎么一步步分化出来的

这两条路不是凭空一起冒出来的，而是随着「谁在用、用来干嘛」的变化，沿着时间慢慢长出来的，而且谁抄谁、谁是谁改的，都有迹可循。我画了张图：

```
逆向 API 一脉
   ReaJason/xhs (2023/4, 鼻祖：数据模型 + 可插拔签名)
        │ 互相致谢
        ▼
   MediaCrawler (2023/6, 多平台)        Cloxl/xhshow (签名公共库, MIT)
        └──────── 现在外包给 ──────►  ◄─────── 薄封装 ────────┐
                                            jackwener/xiaohongshu-cli (2026/3)

浏览器一脉
   xpzouying/xiaohongshu-mcp (2025/8, Go·go-rod, MCP 标杆)
        │ 同作者·零部署版             ╲ 一个文件一个文件翻成 Python + 加插件
        ▼                            ▼
   xpzouying/x-mcp (2025/10)     autoclaw-cc/xiaohongshu-skills (2026/3)
   (云中继 + 本地插件)            (CDP/插件双后端, SKILL.md, 风控分析)

   white0dew/XiaohongshuSkills (2026/2, 独立的裸 CDP 发布工具)
   socai (裸 CDP + 多模态 + 集成产品)
```

几个有源码能对上的事实：xhshow 是逆向这条路实际上的地基（MediaCrawler、xiaohongshu-cli 都用它，前者还给它修过 bug）；ReaJason/xhs 立下了早期那套接口、枚举和 helper 写法，后来者一直在沿用；x-mcp 是 xiaohongshu-mcp 同一个作者出的零部署版（原版 README 直接推荐）；autoclaw 基本是把 xiaohongshu-mcp 一个文件一个文件从 Go 翻成 Python、再加了插件。xpzouying 一个人贡献了两个关键节点，可以说 MCP 时代的小红书工具有一大半是围着他这套东西长起来的。

把时间线拉直，就是一条「用的人变了，工具也跟着变」的主线：2023 年是开发者在爬数据做只读分析，自己逆向签名；2024 到 2025 上半年，纯逆向太难维护，大家转向浏览器自动化加反检测；2025 下半年进入 MCP 时代，用的人从开发者变成了 AI agent，目标也从「读」转向「写 / 发布」；到 2026，执行环境直接变成你本人的真实浏览器，取数也升级到拦截网络请求，几个逆向老将陆续停更。三条贯穿始终的趋势：从「逆向爬取」走向「以真人身份操作」；从「自己造签名」到「让网页代签」再到「直接拦页面成品」；从「开发者用的库」到「agent 用的工具」再到「普通人用的零部署产品」。

## 二、小红书的签名和风控：x-s、xsec_token 到底是什么

签名和风控不是所有差异的根源，根源是上面那个「谁来操作」的机制。但它是两条路都绕不开的同一堵墙，也是看懂封号风险的基础。这一节往里钻一点，不想看细节的可以直接跳到第三节。

### 每个请求要带哪些签名头

小红书网页端每次调接口，都得带一组它自己定义的请求头，核心是四个：

- `x-s`：签名的主体。由一段混淆得很厉害的前端 JS（早期入口叫 `window._webmsxyw(url, data)`，后来改过名）算出来，输入是请求路径、body、时间戳，还有 cookie 里的 `a1`。里面用了打乱过的自定义 Base64 码表、CRC32 的变种（代码里常见的 `mrc()`）和好几轮位运算。
- `x-t`：毫秒级时间戳，参与 `x-s` 的计算，服务端也拿它校验有没有过期。
- `x-s-common`：一段描述「你这个客户端长什么样」的 JSON，用自定义 Base64 编码后塞进请求头，里面有 SDK 版本、操作系统、`xhs-pc-web` 标识、cookie 里的 `a1`、localStorage 里的 `b1`，还有给前面这些字段算的 CRC 校验值。等于把「你是谁、你的环境长啥样」一起打包进了签名，想伪造就得连环境一块编圆。
- `x-b3-traceid`：链路追踪用的 ID，随机生成就行。

走逆向这条路，有两个变量是软肋：`a1`（cookie 里的访客指纹，签名重度依赖它，逆向方要么偷一个真的、要么自己造一个格式合法的）和 `b1`（localStorage 里的环境指纹，注意它不在 cookie 里，纯 HTTP 根本拿不到，逆向工具经常只能留空或者糊弄一个近似值）。b1 拿不到，就是纯 HTTP 这条路天生的一个破绽。

还有个容易被忽略但很关键的点：不同业务用的签名方案是不一样的。

| 业务 | host | 签名 | 难度 |
|---|---|---|---|
| 浏览 / 搜索 / 互动 | `edith.xiaohongshu.com` | `x-s` 这一套 | 主战场，逆向得最透 |
| 创作 / 发布 | `creator.xiaohongshu.com` | 另一套（代码里见 `XYW_` 前缀） | 单独逆向，资料少 |
| 客服 / 商业 | `customer.xiaohongshu.com` | 又一套，相对简单 | 偶尔用到 |

所以逆向想做到「功能全」，得把好几套签名分别都啃下来，这是它维护成本高的又一个原因。

### xsec_token：每篇笔记自带的一张门票

`xsec_token` 是一张绑定到具体某篇笔记的门票，看详情、抓评论、点赞收藏都得带着它。它有几个特点：

- 成对出现：它跟笔记是一起给你的，来自上一步的列表或搜索结果（就在笔记 URL 的 query 里，像 `…/explore/{note_id}?xsec_token=…&xsec_source=pc_feed`）。
- 带个来源标记 `xsec_source`：值是 `pc_feed`、`pc_search` 之类，标明「你是从哪个入口看到这篇笔记的」。这是一种反爬设计，它把「读这篇笔记」和「你是经过正常浏览才看到它的」绑在一起，你没法凭空拿一个 note_id 直接去读详情，必须先走一遍列表。
- 能短时间复用：拿到后可以缓存一会儿（xiaohongshu-cli 就缓存了），但会过期、会失效。

这条限制几乎影响了所有工具的接口设计（详情、互动这些接口基本都要求你同时传 feed_id 和 xsec_token），也解释了为什么你直接去访问 `/explore/<id>`（不带 token）经常会被怼成一个空白页或者验证页，这正是 socai 的反爬规则里要求「从搜索 / 个人主页的卡片点进去，别自己拼 URL」的原因。

### 光签名对还不够：行为风控

签名合法只是拿到入场券。就算你每个请求都签对了，平台还会从好几个角度判断你「像不像机器人」：

- 频率和节奏：单位时间发了多少请求、间隔是不是规律得不像人（真人是有抖动的）。
- 设备指纹对不对得上：`a1` / `b1`、UA、`sec-ch-ua` 这三件套、时区、屏幕参数，彼此自不自洽、跟历史一不一致。纯 HTTP 的工具最容易在这儿露馅，请求头拼得再像，也少了真浏览器才有的 TLS 指纹和 JS 运行时特征。
- 服务端的风控结论：前端 SDK 跑几次之后，会把服务端给你的判定（类似 `isRiskUser`）通过 APM 上报。autoclaw 的 `risk_analyzer.py` 就把这个上报拦下来解析了。

被风控之后是一级一级来的：先是验证码（滑块、点选），再是限流（降速或者直接给你空结果），然后功能受限，最后封号。规律是：低频、克制的「读」最安全；反复批量读、短时间打开一堆笔记、直接跳详情页，都可能触发风控；而高频的「写」才是封号的重灾区（下面两节细说）。

## 三、为什么「自动发布」最容易被封号

把「操作小红书」笼统看成一回事，会漏掉一个很重要的区别。其实按麻烦程度分两类就够了：搜索、看、点赞这类是「轻」的单步操作；发笔记是「重」的多步流程。

### 搜索、看详情、点赞这类轻操作

搜索、看详情、抓评论、点赞、收藏、关注，共同点是一步就完、改动很小（读没有副作用，互动一次就一个动作），都要带 xsec_token，封号风险相对低。两条路实现方式不同：

- 逆向 API：直接签好请求打接口，GET 搜索 / 详情、POST 点赞评论，数据从接口返回的 JSON 里取。
- 浏览器自动化：开浏览器去搜、去点开笔记，数据从 `__INITIAL_STATE__` 或者拦下来的响应里读；互动就是去点页面上的按钮（或者触发页面自己的请求）。

### 发笔记这类重操作

发图文、发视频就完全是另一个量级了：

- 多步、还带状态：先拿上传凭证 → 上传图片 / 视频（大文件还得切片）→ 创建笔记。
- 逆向 API：得切到 creator 这个域、再逆向另一套签名，还要把整个上传协议照着实现一遍（ReaJason/xhs 的 `upload_file_with_slice`、xiaohongshu-cli 的 `get_upload_permit` 证明纯接口发布是能做的，但工作量大得多）。
- 浏览器自动化：一律走创作者中心的网页流程，填标题正文、上传、写话题标签、点发布。这条路死死依赖创作者中心的页面结构，平台一改版就坏（XiaohongshuSkills 的 README 专门提到要为「2026 年 2 到 3 月创作者中心改版」重新改选择器）。

一句话：发布几乎是所有 MCP / Skill 工具的主打卖点，但它偏偏是最容易坏（多步又跟着改版）、也最容易被封的一类。这也是为什么纯读的「研究型」工具（比如 socai）和重发布的「运营型」工具（xiaohongshu-mcp / x-mcp / Skills 这种）会长成完全不同的样子。

## 四、7 个小红书自动化开源项目逐个点评（外加 socai）

下面按出现时间从新到老，挨个说一遍。

### socai-io/socai — 内容研究 + 多模态富集的集成产品

- 走浏览器自动化里的裸 CDP，用 Rust 写，读 `__INITIAL_STATE__`，默认挂在你真实的 Chrome 上（`discover_existing_chrome_endpoint`；也能用 `socai config set chrome.profile managed` 永久切到独立配置，默认路径 `~/.socai/chrome-profile`）。所有 JS 抽取脚本集中在 `page_scripts.js`，通过 `Runtime.evaluate` 注入、返回 JSON，取数思路跟 xiaohongshu-mcp、autoclaw 是一类。
- 重心在「读 / 研究」而不是「写 / 运营」：`search_notes`、`topic_scan`、`extract_note`、`extract_comments`、`extract_profile`、`scroll_in_note`、`collect_carousel_images` 这些，目前没有发布 / 互动的写工具，跟那些主打发布的 MCP / Skill 工具正好相反。
- 唯一一个真正做内容理解的：图片 OCR、用 vision 描述图片，视频转写 / 摘要 / 抽帧描述。另外几个基本都停在「取文字加数数」，socai 把「把一篇笔记真正读懂」做到了内容这一层。
- 三种用法合一：不带 agent 的工具命令行（daemon 常驻、跨调用还热着）、agent TUI、还有 Tauri 桌面应用，公开仓库里少见的本地集成产品，不只是一个库 / 命令行 / server / 插件。

### autoclaw-cc/xiaohongshu-skills — 插件 Bridge + 技能集

走浏览器自动化，而且两种控制方式都做了：上层抽象出一个跟 CDP `Page` 一样接口的东西，底下可以切，

- 裸 CDP（`cdp.py`），或者
- 插件 Bridge（`bridge.py` 加 `extension/`）：MV3 插件通过 `ws://localhost:9333` 连本地的 `bridge_server.py`，在你真实已登录的浏览器里执行；`interceptor.js` 在 `document_start` 的 MAIN world 里抢先 hook `fetch` / `XHR`，直接把页面已经签好名的请求响应抓下来。

两种后端共用同一套上层逻辑，而这套逻辑基本是把 xiaohongshu-mcp（Go）一个文件一个文件翻成 Python（每个文件的 docstring 都标了「对应 Go 的 xiaohongshu/xxx.go」）。它额外有个深度功能在 `risk_analyzer.py`：把拦下来的 APM 上报解析了，输出一份结构化的风控报告，这是七个里唯一一个把「读出平台对你的风控判定」做成功能的。接口是 5 个 `SKILL.md`（auth / publish / explore / interact / content-ops）加一个统一命令行。

### jackwener/xiaohongshu-cli — 为 agent 设计的逆向 CLI

走逆向 API，`signing.py` 是 xhshow 的薄封装、`creator_signing.py` 自带创作者端签名，跑的时候不开浏览器。两个亮点。一是反检测做得最用心（毕竟纯 HTTP 先天吃亏）：固定一套 macOS Chrome 指纹、`sec-ch-ua` 对齐、整个会话身份稳定、请求之间加高斯抖动、撞到验证码就指数退避冷却。二是天生为 agent 设计：命令支持 `--yaml` / `--json`，不是终端就默认输出 YAML；返回值统一包成 `ok / schema_version / data / error` 加枚举错误码；还能用短编号导航（`xhs read 1` 直接复用上次列表的第 1 条）。功能覆盖搜索、阅读、互动、发布、通知，是逆向这条路里功能最全、最适合被脚本和 agent 调用的一个。也已经停更了（2026 年 3 月）。

### white0dew/XiaohongshuSkills — CDP 直连的发布 / 运营工具

走浏览器自动化里的裸 CDP，`scripts/cdp_publish.py`（单文件 192KB）直接用 `websockets` 收发 `Page.*`、`Runtime.*`、`Input.*` 这些 CDP 命令，不依赖 Playwright 或 go-rod。`chrome_launcher.py` 负责起 Chrome，带调试端口，而且按账号分开 user-data-dir（支持多账号），也能连远程 CDP 和无头模式。一开始只做发布，现在扩到了搜索、详情、评论、点赞收藏、个人主页、通知抓取，还能把内容数据看板导出成 CSV。提供命令行加 `SKILL.md` 加 Claude Code 接入文档，人用 agent 用都行。强依赖页面结构，所以「创作者中心改版」是它最大的维护负担。

### xpzouying/x-mcp — 零部署版（云中继 + 本地插件）

跟 xiaohongshu-mcp 同一个作者，专门为「被原版部署劝退的非技术用户」做的。这里要把它的架构看清楚，它不是把 go-rod 搬到云上跑，而是换了一套玩法：

- 你在自己浏览器里装一个（闭源的）Chrome 插件；
- 插件通过 WebSocket 连到云端（`wss://mcp.aredink.com/ws`）；
- 云端把 MCP over HTTP（`/mcp` 加 `X-API-Key`）暴露给 AI 客户端；
- AI 一调用 → 云端转发 → 插件在你本地真实浏览器里执行 → 结果再传回去。

也就是说，真正操作网页的所有动作都发生在你本地浏览器里（靠插件），云端只干两件事：托管 MCP 端点、做多租户鉴权和转发。所谓「SaaS」指的是 MCP server 这个中继被托管在云上，而不是自动化在云上跑。对比一下：xiaohongshu-mcp 是「本地自己起无头浏览器加 go-rod」，x-mcp 是「你的真实浏览器加插件」再把 MCP server 端搬上云，玩法不同，但都属于浏览器自动化。工具接口（`xhs_*`）跟 xiaohongshu-mcp 一一对应。卖点是零部署、复用你日常登录状态、还能在浏览器里实时看到甚至插手，接入只要 `claude mcp add --transport http` 一行。它这个 GitHub 仓库基本没源码（就 README、SKILL.md、接入文档、隐私政策），是这次对比里唯一一个商业化形态的样本。

### xpzouying/xiaohongshu-mcp — 自托管 MCP server 标杆

走浏览器自动化，具体是 go-rod（CDP）加 stealth 加一个独立的无头浏览器。读数据用 `MustEval` 取 `__INITIAL_STATE__`，写操作走 DOM，搜索筛选靠按位置点筛选标签。接口是 MCP over StreamableHTTP（用 Gin 挂在 `/mcp`，默认端口 18060，注意不走 stdio，所以能装进容器，有 Docker 镜像）。功能很全：发布支持定时、原创声明、可见范围、带货商品绑定，看详情能滚动加载全部评论、展开二级回复。它是 2025 下半年 MCP 这波里最有影响力的小红书工具，上面的 x-mcp 和 autoclaw 都是它的衍生。面向的是能自己部署 Go 服务 / Docker 的开发者和他们的 AI 客户端。

### NanmiCoder/MediaCrawler — 多平台采集框架

它不是只做小红书的，而是把小红书、抖音、快手、B 站、微博、贴吧、知乎一锅端的多平台爬虫框架，小红书只是 `media_platform/xhs/` 下面的一个子模块。它的路线变过好几次：早期用 Playwright 注入拿签名，现在把签名外包给 xhshow 纯算法，同时还留着 CDP 模式连本地 Chrome 来增强反检测，代码里好几套签名实现并存，就是这段历史留下的化石。工程完成度是七个里最高的：IP 代理池、七种存储后端、登录态缓存、评论词云、并发控制、还有 FastAPI 加 Vite 的 WebUI。定位是给开发者、数据分析做批量离线采集，基本纯读。它还有个闭源的 MediaCrawler Pro（去掉 Playwright、断点续爬、多账号、内容解构 agent），典型的「开源引流、闭源赚钱」。

### ReaJason/xhs — 生态鼻祖，一个 HTTP 客户端库

七个里最老的（2023 年 4 月），就是 PyPI 上那个 `XhsClient` 库，作者自己说是「练 Python」写的。走逆向 API，签名可以自己插。它的历史意义在于给后来者定了一套公共词汇：`Note` / `FeedType` / `SearchSortType` 这些枚举、`get_imgs_url_from_note` 这类 helper，被后面一堆项目沿用，MediaCrawler 跟它互相致谢。功能覆盖搜索、详情、评论、个人主页，还有完整的发布链路（含切片上传）。现在已经停更。

## 五、封号风险排个序，再说说你还得自己动手的部分

讲了一堆技术，但用户真正关心的其实就两件事：会不会被封，以及用起来要我自己动多少手。

### 到底什么情况会被封

封号风险主要看两点，而且都跟「走哪条路」有关：

1. 请求像不像真人发的，纯 HTTP（逆向 API）缺 b1、缺真浏览器的 TLS 和 JS 运行时指纹，最容易被识破；让真浏览器去签名（浏览器自动化）天生带齐全套环境特征，被发现的概率明显低。这也是逆向那条路陆续停更、浏览器那条路起来的深层原因之一。
2. 操作类型和频率，读 < 互动 < 发布，低频 < 高频。封号几乎都发生在高频写，跟你走哪条路关系不大。

按这个给个封号风险排序（从低到高，前提都是低频、克制、账号本身正常）：

- 最低：挂真实浏览器加只读（socai 现在的形态，autoclaw 的读模式），最接近真人看内容，但反复批量打开笔记还是可能触发风控。
- 较低：浏览器路线的适度写（xiaohongshu-mcp / XiaohongshuSkills，前提是控制频率）。
- 中等：任何工具的高频写（批量发布、批量互动都危险）。
- 较高：纯 HTTP 逆向（ReaJason/xhs、xiaohongshu-cli），指纹和节奏最容易露馅，所以 xiaohongshu-cli 才不得不下血本做高斯抖动加冷却来补救。

> 一句话总结：封号风险差不多等于「请求像不像真人」乘以「写操作的频率」。走哪条路影响前者，用得克不克制影响后者。

### 配好之后，你还得自己动手做什么

现在人人都让 agent 帮忙跑，「会不会写代码、懂不懂 Docker」已经不是主要门槛了，agent 能替你敲命令、看报错。真正决定体验的，是整个流程里还剩多少非你本人不可的环节：

- 能不能一次配通：依赖装不装得上、端口 / 扩展能不能一次连上，还是要反复折腾。
- 要不要老是手动扫码、过验证：登录多久失效一次、失效了是不是又得掏手机扫码、过滑块，这是最高频、最打断状态的人工活。
- 要不要每次点确认弹窗：自动化的动作会不会触发浏览器 / 页面的确认框，得人去点。
- 能不能放着让它无人值守地连续跑：任务能不能排队一条条执行，还是跑几条就被风控打断、得人来重置。

按这个标准，差别挺大：纯 HTTP 工具一旦配通，人工成分很低，但停更之后「配通」这一步本身就又难又容易碎；浏览器路线第一次得扫码登录（之后复用登录状态），独立配置或挂真实浏览器的登录状态最持久、扫码最少；而任何工具只要一撞上验证码 / 风控，「剩余人工」立刻飙升，又绕回上一节：少做高频写，才能少被打断。socai 默认挂你已有的 Chrome 配置，登录状态最省心；想隔离的话，用 `socai config set chrome.profile managed` 切到独立配置（要在新配置里先扫码登录一次）。

### 这些工具，和灰产群控不是一回事

把视野再放大一点：这批开源工具，跟「大规模灰产、工业化操作小红书」完全是两个世界，后者才是平台风控真正的对手。把这条界划清楚，能帮你看清这批工具（包括 socai）的能力和风险上限。

| 维度 | 这批开源工具 | 灰产群控 |
|---|---|---|
| 协议层 | 网页端（`xiaohongshu.com`，x-s 签名） | 多为 App 端协议（逆向 APK、protobuf、更强加固） |
| 设备 | 单台真机 / 单浏览器 | 真机农场（群控 / 云控）、改机框架、hook 改设备指纹 |
| 账号 | 单账号或少量 | 账号池（成百上千）加接码平台加养号 |
| 网络 | 本机 IP 或少量代理 | 住宅代理池、4G 卡池、一机一 IP |
| 目标 | 个人研究 / 运营 / 学习 | 刷量、养号、批量广告、数据倒卖 |

三个关键区别：一是网页端还是 App 端，开源工具几乎都打网页端（资料多、签名相对好逆），灰产更多去啃 App 协议；二是单点还是农场，开源是「一个真人的自动化」，灰产是「上千个假人的工业化」，它的核心资产是设备、账号、IP 的规模化供给和改机能力，根本不是签名本身；三是对抗强度，灰产跟平台风控是全职在打军备竞赛，开源工具只是「尽量装得像真人」。这批工具都站在「个人尺度、真人身份」这一边，这决定了它们不该去碰改机、账号池那一套，也决定了产品边界应该围着「低频、能解释、能随时人工接手」来设计。

## 六、socai：一个低封号风险的小红书研究工具

简单说，socai 就是这批工具里少见的、专门为「读和研究」而不是「发布运营」做的那一个。它默认挂在你真实登录的 Chrome 上、以只读为主，所以在上一节的封号风险排序里处在最低那一档；它也是唯一一个真正把内容读懂的，除了取文字，还做图片 OCR、视频转写和理解，这恰好是做选题、竞品研究最值钱的能力。形态上它不是一个库或一段脚本，而是命令行、TUI 和桌面 app 合一的本地产品。再往前一步，多模态加多 agent 并行，可以把「研究一个小红书话题」从一篇篇读，变成并行深读 N 篇再给你一份洞察。如果你正想安全、低风险地把一个话题读透、做选题或竞品研究，它就是为这个场景做的，[在 GitHub 上看 socai →](https://github.com/socai-io/socai)。
