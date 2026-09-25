---
layout: ../../../layouts/BlogPost.astro
lang: zh
title: "Jev 如何操作社交媒体？"
description: "通用 AI agent 和Computer Use功能经常很慢很傻，还会卡在社交平台的登录、搜索，也不知道如何高效地读帖子和看评论区。Jev 非常快，用Jev配合 socai 可以极快地操作 Instagram、TikTok 和 LinkedIn，获取社媒内容。"
date: 2026-09-19
dateLabel: 2026 年 9 月 19 日
readingTime: 5 分钟阅读
alternates:
  - hreflang: en
    href: /blog/jev-social-media-automation
faq:
  - q: "Jev 可以操作哪些社交平台？"
    a: "jev-social 将 Jev 接入 socai，目前提供 Instagram、TikTok 和 LinkedIn 的搜索与内容读取操作。Jev 可以选择具体帖子、视频或主页继续查看。"
  - q: "它能拿到哪些社交媒体数据？"
    a: "根据平台与页面可访问情况，可以取得帖子正文、视频、作者信息、评论、来源链接，以及页面返回的互动数据。"
  - q: "需要申请 Instagram 或 TikTok 的开发者 API 吗？"
    a: "它通过 socai CLI 操作你本地的 Chrome，无需 API。你需要能调用 Jev 的 OpenRouter Key，然后在本地登录社媒平台就够。"
---

[Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev) 是 TypeSafe AI 推出的一个决策模型，特点是快。把当前情况和几个选项给它，它就能选出下一步做什么。放到社交媒体上，就是搜什么关键词、点开哪条帖子、接着看哪个作者。

我们想拿它试一件很实用的事：让 agent 自己去社交平台里找内容、看评论。于是把 Jev 和 [socai](https://github.com/socai-io/socai) 接在一起，做了 [jev-social](https://github.com/socai-io/jev-social)。现在可以用它操作 Instagram、TikTok 和 LinkedIn。

## 让 agent 去刷社交媒体，现有方式都很差

如果你用过通用 agent 或 Computer Use 做社媒调研，应该熟悉这种感觉：它已经想了半天，浏览器还停在搜索框。

搜一个词，要截图、找按钮、点击，再等下一张截图。好不容易点进帖子，评论还没展开。你看着它一步步折腾，最后忍不住自己上手。

换成网页搜索也不太够。你想知道一个产品在 TikTok 上大家怎么评价，搜索引擎可能只给你几个链接和摘要。真正有用的东西在视频里、帖子里，尤其在评论区：有人问价格，有人吐槽，也有人提到了你没想到的使用场景。

这些内容拿不到，后面的分析写得再漂亮也没什么用。你还是得自己搜、自己翻，再把内容复制给 AI。

## 让 Jev 决定点哪里，用 socai 去操作

socai 已经有搜索、打开帖子、读主页、获取评论这些社交平台操作。Jev 可以直接挑一个来用。

比如你让它找手工作品，它会先选搜索操作。结果出来后，再决定打开哪条帖子、看哪个创作者。读完一条，接着选下一步。每一步拿到的新内容，都会交给 Jev 继续判断。

**Jev 选到的是具体操作：调用哪个 socai CLI 命令、打开哪个链接、读取哪条帖子的评论。** socai 负责把浏览器里的导航、点击和内容读取做完。这样就不用让模型每次从一张截图里重新找搜索框、猜按钮在哪。

你只需要说想找什么，等它把帖子、作者、评论和原帖链接带回来。已抓取的卡片会一直保留，最终报告从上到下流式生成；报告里的结论必须引用这些已抓取的来源。

## 案例：在 Instagram 找某个方向火的帖子

比如你要在Instagram上找手工艺术作品，然后知道什么帖子内容火。在 jev-social 里输入：

> Find handmade art on Instagram and read the comments on relevant posts.


跑起来以后，Jev 先选择搜索 `handmade art`，拿到结果，再挑具体帖子打开。socai 把正文和评论读回来，Jev 再决定继续看哪条，直到结束。

这套流程已经在真实的 Instagram 上跑过。结果里能看到作品的帖子、作者、评论和链接，也能看到它刚才做了哪些操作。

如果你在找创作者合作，可以先看看作品，再看评论里有没有人问怎么买。如果你自己卖手工作品，也可以用这些帖子找灵感：大家喜欢什么、关心价格还是制作过程、有什么问题一直没人回答。

以前这些都要一条条点开看。现在可以先让 Jev 和 socai 跑一遍，把你感兴趣的内容找出来，再点回原帖细看。

## 换成你自己的产品和行业

做电商，可以去 TikTok 搜同类产品的视频，读读评论里的真实反馈。做内容，可以找一批相关创作者，看看他们最近在发什么，哪类话题有人讨论。

想了解竞品，也可以直接搜品牌名，打开相关帖子看用户怎么说。要找行业里的人或公司，就换到 LinkedIn，搜索后继续看主页、帖子和工作经历。

这些任务都有一大段重复劳动：搜、点开、读、复制。让 agent 把这部分做掉，你就能把时间花在挑选和判断上。

## 如何用

[jev-social 是开源的](https://github.com/socai-io/jev-social)。安装 Node 20+ 后，不用克隆仓库，运行两条命令即可完成设置并打开本地界面：

```bash
npx github:socai-io/jev-social#v0.1.8 onboard
npx github:socai-io/jev-social#v0.1.8
```

设置时可选择 OpenRouter Jev，或你自己启动的本地 System One 兼容端点；本地方式不需要 OpenRouter Key。如果没有找到 socai CLI，设置过程也会询问是否安装官方版本。你仍然需要登录要调研的社媒账号。0.1.8 加入了可复现的基准测试流程，但在跑满要求的数据前不会发布速度结论。[Jev Social 项目页](https://socai-io.github.io/jev-social/)提供演示、架构说明和源码安装方式。

可以找你想研究的产品、品牌或者创作者，看看评论区里能找到什么。
