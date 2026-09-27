---
layout: ../../../layouts/BlogPost.astro
lang: zh
title: "本地浏览器 Agent 并不等于离线：Jev Social 的数据流"
description: "Jev Social 隐私边界详解：哪些数据留在本机，哪些会发给模型服务与社交平台，socai 子进程拿不到哪些凭证，以及如何关闭模型报告生成。"
date: 2026-09-28
dateLabel: 2026 年 9 月 28 日
readingTime: 6 分钟阅读
alternates:
  - hreflang: en
    href: /blog/jev-social-privacy-boundary
faq:
  - q: "本地浏览器调研是否意味着没有网络请求？"
    a: "不是。Chrome 与 socai 需要访问所选社交平台，默认的 Jev 决策与报告生成也会访问配置的模型服务。本地指应用、浏览器会话和运行产物由你控制，并不是完全不使用网络。"
  - q: "socai 子进程会拿到模型服务的 Key 吗？"
    a: "Jev Social 不会把 OPENROUTER_API_KEY、TYPESAFE_API_KEY、SOCAI_API_KEY 或会话 Token 环境变量放进 socai 子进程。子进程仍会得到操作所选浏览器会话所需的 Chrome、CDP 与 socai 目录设置，而且它不是文件系统沙箱。"
  - q: "报告生成可以不再请求一次模型服务吗？"
    a: "可以。设置 OPENROUTER_REPORT_MODEL=off 后会使用确定性的本地报告路径。除非同时选择本机 System One 端点，否则 Jev 决策仍会访问你配置的服务。"
---

“在本地运行”对于浏览器 Agent 来说太模糊了。界面可以开在你的电脑上，但提示词、Cookie、页面内容和报告仍可能经过多个服务。

真正应该问的是：**哪个进程会为了什么目的拿到哪些数据，结果最后保存在哪里？**

我们给 [Jev Social](https://github.com/socai-io/jev-social) 补了一份完整的数据流说明。它让 Jev 选择只读步骤，再由本机的 socai CLI 在已经登录的 Chrome 会话里搜索 Instagram、TikTok 和 LinkedIn。下面是这条链路里最重要的边界。

## 四条不同的数据边界

一次 Jev Social 调研涉及四个目的地：

1. **本机 Jev Social 进程**负责研究循环、校验 Jev 返回的类型化选择，并写入运行产物。
2. **配置的模型服务**接收受限的 Jev 决策输入；默认还会收到一份单独的报告生成请求。
3. **Chrome、socai 与社交平台**使用你选择的浏览器会话完成搜索或读取操作。
4. **本地磁盘**保存抓取证据、运行记录和最终报告，直到你手动删除。

把整条链路统称为“本地”会掩盖真正重要的区别。浏览器在本机，但 Instagram 或 TikTok 仍会收到正常的浏览器请求；应用在本机，但使用托管 Jev 时仍然需要访问模型服务。

## Jev Social 不会放进 socai 子进程环境的内容

Jev Social 启动每条 `socai` 命令时，只传递明确允许的环境变量。它不会把 `OPENROUTER_API_KEY`、`TYPESAFE_API_KEY`、`SOCAI_API_KEY` 或会话 Token 环境变量放进子进程。

这是凭据过滤，不是进程沙箱。子进程仍会获得操作所选浏览器会话所需的 Chrome、CDP 与 socai 目录设置，也可以访问当前系统用户有权读取的文件。

除非启动环境明确设置为 `SOCAI_TELEMETRY=1`，Jev Social 启动的子进程都会使用 `SOCAI_TELEMETRY=0`。这个默认值同时覆盖能力检查与浏览器操作，包括复用已经运行的 socai daemon 的命令。

这是一条进程边界，不是对整台机器的承诺。单独启动的 socai Desktop 有自己的环境，不会被 Jev Social 事后修改；如果同时使用桌面端，需要单独配置它。

## 哪些内容会发给 Jev 与报告模型

设置过程中，Jev Social 可能把 OpenRouter Key 发送到 `https://openrouter.ai/api/v1/auth/key` 做有效性校验。之后改用本地决策端点与确定性报告，并不会抹掉已经发生的这次校验请求。

每次做决策时，Jev 会收到调研目标、指定平台、当前可选的类型化动作、已经观察到的来源链接、前几步摘要，以及抓取记录中长度受限的文本片段。它不会有意接收浏览器 Cookie、下载的视频、原始 socai JSON 或本地文件路径。

报告生成是另一条独立请求。默认情况下，它只发送经过清理且受长度限制的证据投影：调研目标、平台、运行状态、覆盖数量，以及最多 40 条记录中的来源链接、标题、观点摘录、少量评论摘录、页面互动字段与抓取深度。

如果报告必须走确定性的本地路径，可以在项目环境中设置：

```bash
OPENROUTER_REPORT_MODEL=off
```

它只会关闭报告生成请求，不会改变 Jev 决策的目的地。如果决策也必须留在本机，需要为 Jev Social 配置明确的回环地址 `JEV_SOCIAL_SYSTEM_ONE_URL`，并由兼容的本地服务提供模型能力。

## 浏览器会话留在哪里

Chrome Cookie 和登录状态保留在已安装 socai CLI 所选择的 Chrome 或 socai 配置文件中。Jev Social 不会直接读取或复制浏览器 Cookie 存储。

浏览器仍然需要访问所选社交平台。当前支持的操作不会修改远端社交状态，主要是搜索、打开、查看和读取；但它们仍会产生网络请求和本地文件。对于中文目标，只有当用户明确要求下载、保存、留存、归档或抓取视频、媒体或文件时，TikTok 视频下载动作才会出现。该选择会以 `downloadMedia: true` 记录；明确请求之后不会再出现第二次确认。

如果调研对象比较敏感，建议使用独立浏览器配置文件或测试账号。平台登录墙、验证与限流仍然有效。

## 运行结束后还留下什么

本机会有两个独立的存储位置。`JEV_SOCIAL_HOME` 保存 `config.json` 和每次运行的 JSON；如果在设置过程中交互式输入并选择保存，OpenRouter Key 可能写入 `config.json`。socai 还有自己的运行目录，其中可能包含浏览器证据和已下载媒体。Jev Social 的媒体链接一小时后失效，也不会删除底层 socai 文件。

两个存储都没有自动清理计划，文件可能一直保留到你主动删除。停止 Jev Social 后，应分别检查并删除不再需要的具体运行 JSON 与具体 socai 运行目录，不要对任一配置根目录执行宽泛的递归删除。

模型服务端的保存策略属于另一条边界。OpenRouter 与所选模型提供方决定各自的存储和保留方式，本地模型服务也可能保留自己的日志。Jev Social 无法把这些外部策略变成本地磁盘承诺。

## 一套更严格的隐私配置

如果希望使用当前最收敛的数据路径：

1. 为调研使用独立 Chrome 配置文件。
2. 保留子进程默认的 `SOCAI_TELEMETRY=0`。
3. 如果 Jev 决策必须留在本机，使用回环 System One 端点。
4. 如果报告生成也必须留在本机，设置 `OPENROUTER_REPORT_MODEL=off`。
5. 分别检查 Jev Social 与 socai 产物，只删除不再需要的具体运行记录。

完整字段、当前限制和删除路径都在 [Jev Social 隐私与数据流说明](https://socai-io.github.io/jev-social/privacy/) 中。重点不是给产品贴上一个“本地”标签，而是让你在运行前就能检查清楚每条数据边界。
