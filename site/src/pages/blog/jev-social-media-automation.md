---
layout: ../../layouts/BlogPost.astro
lang: en
title: "How to use Jev for social media"
description: "General-purpose AI agents and Computer Use can be painfully slow on social sites, getting stuck on logins, searches and comments. Pair Jev with socai to browse Instagram, TikTok and LinkedIn and get the content you need."
date: 2026-09-19
dateLabel: September 19, 2026
readingTime: 5 min read
alternates:
  - hreflang: zh
    href: /blog/zh/jev-social-media-automation
faq:
  - q: "Which social platforms can Jev operate?"
    a: "jev-social connects Jev to socai's Instagram, TikTok and LinkedIn tools. Jev can choose searches and select specific posts, videos or profiles to inspect next."
  - q: "What social media data can it collect?"
    a: "Depending on the platform and page, you can get post text, videos, author information, comments, source links and engagement data."
  - q: "Do I need an Instagram or TikTok developer API key?"
    a: "No. socai uses your local Chrome browser. You just need an OpenRouter key with Jev access and to log in to your social accounts."
---

[Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev) is a fast decision model from TypeSafe AI. Give it the current situation and a few options, and it picks what to do next. On social media, those choices might be what to search for, which post to open, or whose profile to check.

We wanted to put it to work finding social posts and reading comments. So we connected it to [socai](https://github.com/socai-io/socai) and built [jev-social](https://github.com/socai-io/jev-social). It works with Instagram, TikTok and LinkedIn.

## Getting agents to browse social media is still a pain

If you've tried a general-purpose agent or Computer Use for social research, you may have spent a while watching it think with the browser still sitting at the search box.

Search for a topic. Take a screenshot. Find the button. Click it. Wait for another screenshot. Eventually it opens a post, but the comments are still collapsed. At some point, it's tempting to take over and do it yourself.

Web search only gets you so far, too. Ask what people think of a product on TikTok and you might get a few links and snippets. The useful details are inside the videos, posts and comments: questions about price, complaints, or uses you hadn't considered.

Without that material, there's little for the agent to base its analysis on. You end up finding the posts and copying the comments into the chat yourself.

## Jev picks the action; socai does the browsing

socai already has operations for searching social platforms, opening posts, reading profiles and getting comments. Jev can choose which one to use.

Ask it to find handmade art and it starts with a search. Once results come back, it can pick a particular post or creator to look at. After reading that, it chooses again. Each step gives it new information to work with.

**Jev chooses the actual operation: which socai CLI command to call, which link to open, and which post's comments to read.** socai handles the navigation, clicks and content retrieval. The model doesn't have to locate the search box in a fresh screenshot every time.

You describe what you're looking for. The results bring together posts, authors, comments and links you can open yourself. Captured cards stay visible while the final report streams in, and accepted findings must cite one of those captured sources.

## Example: find out what's popular in your niche on Instagram

Say you want to find handmade art on Instagram and see which kinds of posts get people interested. Enter this in jev-social:

> Find handmade art on Instagram and read the comments on relevant posts.

Jev first chooses to search for `handmade art`. It gets the results, picks a post to open, and asks socai to read it and its comments. Then it decides what to look at next, until it finishes.

We've run this flow on Instagram. The results include the posts, authors, comments and original links, along with a record of what the agent did.

If you're looking for creators to work with, you can browse their pieces and see whether people ask where to buy them. If you sell handmade work yourself, those same comments might give you ideas: what people like, whether they ask about price or technique, and which questions go unanswered.

You'd usually have to open each post to collect this. Now you can let Jev and socai do a first pass, then go back to the posts that catch your attention.

## Use your own product, brand or topic

If you sell a product, search TikTok for similar ones and read what people say in the comments. If you make content, find creators in your niche and look at what they're posting and what gets people talking.

For competitor research, search a brand name and open the posts discussing it. For people or company research, switch to LinkedIn and follow the search results into profiles, posts and work experience.

All of these tasks involve a lot of searching, opening, reading and copying. Let the agent handle that part so you can spend your time deciding what's useful.

## How to use it

[jev-social is open source](https://github.com/socai-io/jev-social). With Node 20+ installed, you can onboard and open the local app without cloning the repository:

```bash
npx github:socai-io/jev-social#v0.1.8 onboard
npx github:socai-io/jev-social#v0.1.8
```

Onboarding lets you use OpenRouter Jev or a user-started local System One-compatible endpoint; the local path needs no OpenRouter key. It can also offer to install the current socai CLI when it is missing. You'll still need to log in to the social accounts you want to research. Version 0.1.8 adds a reproducible benchmark workflow, but does not publish a live speed result before the required runs exist. The [Jev Social project page](https://socai-io.github.io/jev-social/) has the recorded demo, architecture and source-checkout path.

Then look up a product, brand or creator you're interested in and see what people are saying in the comments.
