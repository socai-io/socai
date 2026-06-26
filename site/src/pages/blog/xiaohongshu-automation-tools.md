---
layout: ../../layouts/BlogPost.astro
lang: en
title: "Will Xiaohongshu automation get you banned? I read the source of 7 open-source tools (2026.6)"
description: "Does automating Xiaohongshu (RedNote) get your account banned? I read the source of 7 representative open-source tools, scrapers, reverse-engineered APIs, MCP servers, CDP browser automation, and broke down how they work, what they can do, the real ban risk, and how to choose."
date: 2026-06-07
dateLabel: June 2026
readingTime: 20 min read
alternates:
  - hreflang: zh
    href: /blog/zh/xiaohongshu-automation-tools
footnote: This is a snapshot of the open-source Xiaohongshu automation and data-collection landscape as of June 2026. The platform and these projects keep changing, so treat conclusions as dated, check the source.
---

If you work with Xiaohongshu (RedNote), sooner or later you ask: can a program search notes, scrape comments, even post for me automatically? It can, GitHub is full of tools that do exactly this. But the very next question is: will doing this get my account banned?

I wanted to answer both, so I read the source code of seven representative open-source Xiaohongshu automation projects end to end. This post is what I took away: how they actually operate Xiaohongshu, how far each can go, which is most likely to get banned, and where socai, the one I'm building, sits among them.

Here's the map first, ordered newest to oldest, so socai is at the top, the other seven tools follow, and the oldest is at the bottom. Star counts are live; the start dates are mine, to give you a feel for the timeline. Click a project name to jump to its GitHub.

| # | Project | Stars | Start | Lang | Route | One-liner |
|---|---|---|---|---|---|---|
| 1 | [socai-io/socai](https://github.com/socai-io/socai) | ![](https://img.shields.io/github/stars/socai-io/socai?style=flat&label=&color=555) | 2026/6 | Rust | browser (raw CDP) | XHS content research + multimodal enrichment, as a product |
| 2 | [autoclaw-cc/xiaohongshu-skills](https://github.com/autoclaw-cc/xiaohongshu-skills) | ![](https://img.shields.io/github/stars/autoclaw-cc/xiaohongshu-skills?style=flat&label=&color=555) | 2026/3 | Python | browser (CDP/ext) | extension bridge + SKILL.md skill set |
| 3 | [jackwener/xiaohongshu-cli](https://github.com/jackwener/xiaohongshu-cli) | ![](https://img.shields.io/github/stars/jackwener/xiaohongshu-cli?style=flat&label=&color=555) | 2026/3 | Python | reverse API | agent-friendly reverse-API CLI |
| 4 | [white0dew/XiaohongshuSkills](https://github.com/white0dew/XiaohongshuSkills) | ![](https://img.shields.io/github/stars/white0dew/XiaohongshuSkills?style=flat&label=&color=555) | 2026/2 | Python | browser (raw CDP) | CDP-direct publishing / ops tool |
| 5 | [xpzouying/x-mcp](https://github.com/xpzouying/x-mcp) | ![](https://img.shields.io/github/stars/xpzouying/x-mcp?style=flat&label=&color=555) | 2025/10 | ext + cloud | browser (extension) | zero-deploy version of xiaohongshu-mcp (cloud relay + local extension) |
| 6 | [xpzouying/xiaohongshu-mcp](https://github.com/xpzouying/xiaohongshu-mcp) | ![](https://img.shields.io/github/stars/xpzouying/xiaohongshu-mcp?style=flat&label=&color=555) | 2025/8 | Go | browser (go-rod) | self-hosted XHS MCP server |
| 7 | [NanmiCoder/MediaCrawler](https://github.com/NanmiCoder/MediaCrawler) | ![](https://img.shields.io/github/stars/NanmiCoder/MediaCrawler?style=flat&label=&color=555) | 2023/6 | Python | reverse API* | multi-platform data-collection framework (XHS is one of many) |
| 8 | [ReaJason/xhs](https://github.com/ReaJason/xhs) | ![](https://img.shields.io/github/stars/ReaJason/xhs?style=flat&label=&color=555) | 2023/4 *(inactive)* | Python | reverse API | pluggable-signing HTTP client library, the ancestor |

\* MediaCrawler now fetches data mainly through the reverse API, but still uses a browser / CDP for login state and environment.

## 1. Xiaohongshu automation has exactly two technical routes: reverse API vs browser automation

The seven tools look wildly different, there's a Python library, a CLI, MCP servers, browser extensions. But strip it all down and there's only one real difference: who talks to Xiaohongshu's servers.

One option is the tool does it itself. Reimplement Xiaohongshu's request-signing algorithm in Python or Go, assemble the headers yourself, and hit the API directly with something like requests / httpx. I'll call this the reverse API. This route doesn't touch the page during normal operation, though it may still borrow a browser to grab login state, initialize cookies, or for anti-detection.

The other option is to let the browser do it for you. Open a real browser you've already logged into, and let the page's own JS handle searching and sending requests, including the signing. The tool never touches the signature itself. I'll call this browser automation.

Both routes have to clear the same wall: Xiaohongshu's signing plus risk control. The only difference is who computes the signature, the reverse API does it itself, browser automation lets the page do it. Don't underestimate that single choice: whether a tool can scale, how painful it is to maintain, how easily it gets banned, and whether it can run inside your everyday browser are basically all decided by it.

### The reverse route: the signing is already a shared library

Reverse-engineering is an endless tug-of-war. Xiaohongshu changes the signing algorithm every so often, and whoever reverse-engineers it has to rewrite it. That dirty, exhausting work has by now been pulled out of the individual projects into a shared library everyone uses: [xhshow](https://github.com/Cloxl/xhshow) (by Cloxl, MIT). It reimplements the whole `x-s` / `x-s-common` algorithm, the custom Base64 alphabet, a CRC variant, packing the runtime environment into JSON, `a3_hash`, and so on, in pure Python, and exposes `sign_headers_get` / `sign_headers_post`. jackwener/xiaohongshu-cli is basically "xhshow + anti-detection + a nice CLI"; MediaCrawler now outsources its signing to it too (and even patched a GET-request `a3_hash` bug for it).

The older ReaJason/xhs plays it differently: it doesn't bundle signing at all, but leaves `sign` as a constructor argument (`XhsClient(cookie, sign=...)`) for you to pass in, its official example uses Playwright to call the page's `window._webmsxyw`. Both approaches are the same admission: reverse-engineering rots fast, so either everyone pools effort into one shared library, or you punt it entirely to the caller.

Either way, the fate of the reverse route is sealed: Xiaohongshu changes the signing and the whole batch of tools stops working at once. That's a big structural reason ReaJason/xhs and xiaohongshu-cli built up so much maintenance pressure and eventually went inactive (here "inactive" means the author stopped adding features; whether they still answer usage questions is anyone's guess).

### The browser route: pull it apart along three axes

People often treat "use an extension" and "use CDP" as two competing routes. That's a mix-up. First, to be clear:

> Extension, CDP, and Playwright aren't three routes, they're three ways of "controlling the browser" within the browser-automation route. All three require you to be really logged in, all three work by injecting JS into the page (evaluate), and what they can do overlaps heavily. What an extension can do (hook fetch, read cookies, operate in your real profile), CDP can mostly do too (intercept `Fetch` / `Network`, `Network.getAllCookies`, attach to a Chrome with a debug port open). The real differences are deployment, when you attach, how much access you have, and how much of the page lifecycle you can observe.

The browser route really has three axes worth separating, and they're independent, you can mix and match. Put them together and you get a project's full picture.

Axis one: what you use to control the browser.

| Control surface | Examples | Trade-off |
|---|---|---|
| Playwright | MediaCrawler (early) | built-in auto-wait, locators; stable to write, but drags in Node and a few hundred MB of Chromium, heavy |
| CDP direct (raw ws / go-rod / chromiumoxide) | xiaohongshu-mcp, XiaohongshuSkills, autoclaw, socai | light, single binary, fine-grained control, but you handle waiting and retries yourself |
| Browser extension (MV3) | autoclaw, x-mcp | runs natively in your everyday browser, no debug port needed, but users must install it and you're bound by the store |

The real difference between Playwright and CDP direct is "how thick the wrapper is": Playwright adds, on top of CDP, auto-wait (actions wait for elements to be clickable) and locators (selectors that survive re-renders), a whole toolkit built for humans writing tests, at the cost of a Node runtime plus a few hundred MB of version-pinned Chromium. CDP direct wraps the protocol thinly and gives you lightness plus full control over timing. For tools that stay resident and get called over and over, CDP direct usually pays off better, which is why xiaohongshu-mcp and socai both pick it.

Compared with CDP, an extension's hard difference isn't just capability, it's where it lives and how much it sees: an extension doesn't make you restart Chrome with `--remote-debugging-port`, and it can sit and watch the page across `document_start`, the MAIN world, and `webRequest`. x-mcp pushes this advantage to the extreme (zero deploy); socai and xiaohongshu-mcp accept "you need a debug port open" in exchange for not depending on an extension.

Axis two: how you read the data out (increasing robustness).

1. Scrape the DOM: `querySelector` field by field, most direct, but breaks the moment the page changes, and fields are often missing.
2. Read the page's own state: `window.__INITIAL_STATE__`, the big JSON Xiaohongshu injects during server-side rendering, more complete and more stable than the DOM. xiaohongshu-mcp, autoclaw, and socai all prefer this.
3. Intercept network requests: hook `fetch` / `XHR` (in the MAIN world for an extension, or via CDP's `Fetch` domain) and grab the raw response of the page's already-signed requests, the most complete and most stable. autoclaw's `interceptor.js` is the textbook example.

Axis three: whose browser profile you use.

- Attach to your everyday Chrome (real login, real fingerprint, hardest to detect): socai by default (`discover_existing_chrome_endpoint`); autoclaw and x-mcp naturally, since they're extensions.
- Launch a separate Chrome (a dedicated user-data-dir, you scan to log in yourself): good for isolation and running multiple accounts in parallel. Supported by socai (`socai config set chrome.profile managed`), xiaohongshu-mcp (headless + cookie file), and XiaohongshuSkills (per-account).

### How these tools branched out over time

The two routes didn't appear side by side out of nowhere, they grew along the timeline as "who's using it, and for what" shifted, and you can trace who copied or forked whom. Here's the family tree:

```
Reverse-API lineage
   ReaJason/xhs (2023/4, ancestor: data model + pluggable signing)
        │ mutual credits
        ▼
   MediaCrawler (2023/6, multi-platform)     Cloxl/xhshow (shared signing lib, MIT)
        └──────── now outsources to ───────►  ◄─────── thin wrapper ───────┐
                                            jackwener/xiaohongshu-cli (2026/3)

Browser lineage
   xpzouying/xiaohongshu-mcp (2025/8, Go·go-rod, the MCP benchmark)
        │ same author·zero-deploy            ╲ file-by-file Python port + extension
        ▼                                    ▼
   xpzouying/x-mcp (2025/10)            autoclaw-cc/xiaohongshu-skills (2026/3)
   (cloud relay + local extension)      (CDP/extension dual backend, SKILL.md, risk analysis)

   white0dew/XiaohongshuSkills (2026/2, standalone raw-CDP publishing tool)
   socai (raw CDP + multimodal + integrated product)
```

A few facts you can confirm from the source: xhshow is effectively the bedrock of the reverse route (both MediaCrawler and xiaohongshu-cli use it; the former even fixed a bug in it); ReaJason/xhs set down the early endpoints, enums and helper conventions that everyone after kept reusing; x-mcp is the same author's zero-deploy version of xiaohongshu-mcp (the original README recommends it directly); autoclaw is essentially a file-by-file port of xiaohongshu-mcp from Go to Python, plus an extension. xpzouying alone contributed two key nodes, and you could say a good half of the MCP-era Xiaohongshu tools grew around his work.

Straighten out the timeline and you get one through-line, "the user changed, so the tools changed": in 2023 it was developers scraping data for read-only analysis, reverse-engineering the signing themselves; through 2024 to mid-2025, pure reverse-engineering got too hard to maintain and everyone moved to browser automation plus anti-detection; in the second half of 2025 came the MCP era, where the user shifted from developers to AI agents and the goal shifted from reading to writing / publishing; and by 2026 the execution environment is literally your own real browser, data extraction has leveled up to intercepting network requests, and the reverse-engineering veterans have gone quiet one by one. Three trends run through it all: from "reverse-scraping" toward "operating as a real person"; from "compute the signature yourself" to "let the page sign" to "intercept the page's finished output"; and from "a library for developers" to "a tool for agents" to "a zero-deploy product for ordinary users".

## 2. Xiaohongshu's signing and risk control: what x-s and xsec_token actually are

Signing and risk control aren't the root of every difference, the root is the "who operates it" mechanism above. But it's the one wall both routes have to climb, and the basis for understanding ban risk. This section drills in a bit; if you don't want the details, skip to section 3.

### Which signing headers each request carries

Every Xiaohongshu web API call carries a set of custom headers, four of them core:

- `x-s`: the main signature. Computed by a heavily obfuscated piece of frontend JS (the early entry point was `window._webmsxyw(url, data)`, later renamed) from the request path, body, timestamp, and the `a1` in your cookie. Internally it uses a shuffled custom Base64 alphabet, a CRC32 variant (the `mrc()` you'll see in the code), and several rounds of bit manipulation.
- `x-t`: a millisecond timestamp, fed into the `x-s` computation, also used server-side to check freshness.
- `x-s-common`: a JSON blob describing "what your client looks like", custom-Base64-encoded into the header, it contains the SDK version, OS, the `xhs-pc-web` marker, the `a1` from your cookie, the `b1` from localStorage, plus a CRC over those fields. It packs "who you are and what your environment looks like" right into the signature, so forging it means making the whole environment add up.
- `x-b3-traceid`: a tracing ID; random is fine.

Two identity variables are the reverse route's soft spots: `a1` (the visitor fingerprint in the cookie, which the signature leans on heavily, the reverse side either steals a real one or fabricates a valid-looking one) and `b1` (the environment fingerprint in localStorage, note it's not in the cookie, so plain HTTP can't get it at all, and reverse tools often have to leave it blank or fake an approximation). Not having b1 is one of the pure-HTTP route's built-in weaknesses.

There's also a key point that's easy to miss: different parts of the site use different signing schemes.

| Surface | host | Signing | Difficulty |
|---|---|---|---|
| browse / search / interact | `edith.xiaohongshu.com` | the `x-s` family | main battlefield, reversed the most thoroughly |
| create / publish | `creator.xiaohongshu.com` | a separate scheme (see the `XYW_` prefix in code) | reversed separately, little public info |
| support / commerce | `customer.xiaohongshu.com` | yet another, relatively simple | occasionally used |

So for the reverse route to be "fully featured", it has to crack several signing schemes separately, another structural reason its maintenance cost is high.

### xsec_token: a ticket that comes with each note

`xsec_token` is a ticket bound to one specific note. Viewing details, scraping comments, liking, and saving all require it. A few traits:

- It comes paired: the token arrives together with the note, from the previous step's list or search results (it's in the note URL's query, like `…/explore/{note_id}?xsec_token=…&xsec_source=pc_feed`).
- It carries a source marker `xsec_source`: values like `pc_feed`, `pc_search`, marking "which entry point you saw this note through". This is an anti-scraping design, it ties "reading this note" to "you got here through normal browsing", so you can't conjure a note_id out of thin air and read its detail directly; you have to go through a list first.
- It's reusable for a short while: once you have it you can cache it for a bit (xiaohongshu-cli does), but it expires and becomes invalid.

This constraint deeply shapes every tool's interface design (detail and interaction endpoints almost all require both feed_id and xsec_token), and explains why navigating straight to `/explore/<id>` (without a token) often gets bounced to a blank or verification page, which is exactly why socai's anti-scraping rules say "click in from a search or profile card, don't assemble the URL yourself".

### A valid signature isn't enough: behavioral risk control

A valid signature is just the entry ticket. Even if you sign every request correctly, the platform still judges, from several angles, whether you "look like a bot":

- Frequency and rhythm: how many requests per unit time, and whether the intervals are too regular to be human (real people have jitter).
- Device-fingerprint consistency: whether `a1` / `b1`, the UA, the `sec-ch-ua` trio, timezone, and screen parameters are internally consistent and match your history. Pure-HTTP tools are the easiest to expose here, however well you fake the headers, you're still missing the TLS fingerprint and JS-runtime traits a real browser has.
- The server's risk verdict: after a few calls, the frontend SDK reports the server's conclusion (something like `isRiskUser`) via APM. autoclaw's `risk_analyzer.py` intercepts and parses exactly this report.

The consequences escalate step by step: captcha (slider, click-to-select) → throttling (slow you down or just return empty results) → feature restrictions → a ban. The pattern: low-frequency, restrained reads are safest; repeated bulk reads, opening a pile of notes in a short window, and jumping straight to detail pages can all trip risk control; and high-frequency writes are where the bans really happen (more in the next two sections).

## 3. Why "auto-publishing" gets you banned fastest

Treating "operating Xiaohongshu" as one undifferentiated thing misses an important distinction. Split it into two buckets by how involved it is: searching, viewing, and liking are "light" single-step actions; publishing a note is a "heavy" multi-step flow.

### Search, view detail, like — the light stuff

Search, view detail, scrape comments, like, save, follow, they share "one step, small change" (reads have no side effects, an interaction is one action at a time), all need xsec_token, and the ban risk is relatively low. The two routes implement them differently:

- Reverse API: sign the request and hit the endpoint, GET for search / detail, POST for like / comment, and take the data from the JSON the API returns.
- Browser automation: drive the browser to search and open notes, read the data from `__INITIAL_STATE__` or an intercepted response; interactions click the buttons on the page (or trigger the page's own requests).

### Publishing a note — the heavy stuff

Posting images or video is a whole different magnitude:

- A multi-step, stateful flow: get the upload credential → upload the images / video (large files get chunked) → create the note.
- Reverse API: you have to switch to the creator domain, reverse a separate signing scheme, and reimplement the entire upload protocol (ReaJason/xhs's `upload_file_with_slice` and xiaohongshu-cli's `get_upload_permit` prove pure-API publishing is possible, but it's far more work).
- Browser automation: always goes through the Creator Center's web flow, fill in the title and body, upload, write hashtags, hit publish. This path depends heavily on the Creator Center's page structure, and breaks whenever the platform redesigns it (XiaohongshuSkills' README specifically mentions reworking selectors for the "Feb–Mar 2026 Creator Center redesign").

In short: publishing is the headline selling point of almost every MCP / Skill tool, yet it's precisely the most fragile (multi-step and tied to redesigns) and the most ban-prone class. That's also why a read-only "research" tool (like socai) and a publish-heavy "ops" tool (xiaohongshu-mcp / x-mcp / Skills) end up looking completely different.

## 4. The 7 open-source Xiaohongshu automation projects, one by one (plus socai)

Going through them newest to oldest.

### socai-io/socai — content research + multimodal enrichment, as a product

- Browser automation via raw CDP, written in Rust, reading `__INITIAL_STATE__`, attached by default to your real Chrome (`discover_existing_chrome_endpoint`; you can also switch permanently to a separate profile with `socai config set chrome.profile managed`, default path `~/.socai/chrome-profile`). All JS extractors live in `page_scripts.js`, injected via `Runtime.evaluate` and returning JSON, the same data-extraction philosophy as xiaohongshu-mcp and autoclaw.
- The focus is "read / research", not "write / ops": `search_notes`, `topic_scan`, `extract_note`, `extract_comments`, `extract_profile`, `scroll_in_note`, `collect_carousel_images`, and so on, there are no publish / interact write tools for now, the mirror image of the publish-heavy MCP / Skill tools.
- The only one doing real content understanding: image OCR, vision descriptions, and video transcription / summary / keyframe descriptions. The others basically stop at "grab text and count things"; socai takes "actually understand a note" down to the content layer.
- Three delivery surfaces in one: an agentless tool CLI (a resident daemon, warm across calls), an agent TUI, and a Tauri desktop app, a local integrated product, rare in public repos, not just a library / CLI / server / extension.

### autoclaw-cc/xiaohongshu-skills — extension bridge + skill set

Browser automation, and it implements both control surfaces: an abstraction with the same interface as CDP's `Page`, with a switchable backend:

- raw CDP (`cdp.py`), or
- an extension bridge (`bridge.py` + `extension/`): an MV3 extension connects over `ws://localhost:9333` to a local `bridge_server.py` and executes in your real, logged-in browser; `interceptor.js` hooks `fetch` / `XHR` early in the `document_start` MAIN world and grabs the responses of the page's already-signed requests directly.

Both backends share one upper layer, and that layer is essentially a file-by-file port of xiaohongshu-mcp (Go) into Python (each file's docstring notes "corresponds to Go's xiaohongshu/xxx.go"). Its extra depth is in `risk_analyzer.py`: it parses the intercepted APM report and outputs a structured risk-control report, the only one of the seven that turns "reading the platform's risk verdict on you" into a feature. The interface is five `SKILL.md` files (auth / publish / explore / interact / content-ops) plus a unified CLI.

### jackwener/xiaohongshu-cli — an agent-friendly reverse CLI

Reverse API; `signing.py` is a thin wrapper over xhshow, `creator_signing.py` bundles creator-side signing, and it starts no browser at runtime. Two highlights. First, the most careful anti-detection (to offset the inherent pure-HTTP disadvantage): a fixed macOS Chrome fingerprint, `sec-ch-ua` alignment, a stable session-long identity, Gaussian jitter between requests, and exponential-backoff cooldown when it hits a captcha. Second, agent as a first-class citizen: commands support `--yaml` / `--json`, default to YAML when not a TTY; responses are wrapped uniformly as `ok / schema_version / data / error` with enum error codes; and there's short-index navigation (`xhs read 1` reuses item 1 of the last list). It covers search, read, interact, publish, and notifications, the most fully featured of the reverse route and the best suited to be called by scripts and agents. Also inactive now (March 2026).

### white0dew/XiaohongshuSkills — a CDP-direct publishing / ops tool

Browser automation via raw CDP, `scripts/cdp_publish.py` (a single 192KB file) uses `websockets` directly to send and receive `Page.*`, `Runtime.*`, `Input.*` CDP commands, with no Playwright or go-rod. `chrome_launcher.py` starts Chrome with a debug port and a per-account user-data-dir (multi-account), and also supports connecting to a remote CDP and headless mode. It started as publishing-only and has since expanded to search, detail, comments, like / save, profiles, notification scraping, and exporting a content dashboard to CSV. It ships a CLI + `SKILL.md` + Claude Code onboarding docs, usable by humans and agents alike. Its heavy reliance on page structure makes "Creator Center redesigns" its number-one maintenance burden.

### xpzouying/x-mcp — the zero-deploy version (cloud relay + local extension)

Same author as xiaohongshu-mcp, built for "non-technical users put off by the original's deployment". The key is to see its architecture clearly, it doesn't move go-rod to the cloud, it swaps in a different mechanism:

- You install a (closed-source) Chrome extension in your own browser;
- The extension connects to the cloud over WebSocket (`wss://mcp.aredink.com/ws`);
- The cloud exposes MCP over HTTP (`/mcp` + `X-API-Key`) to AI clients;
- AI calls → the cloud relays → the extension executes in your local real browser → the result is sent back.

So all the actions that actually operate the page happen in your local browser (via the extension), and the cloud only does two things: host the MCP endpoint, and handle multi-tenant auth and forwarding. "SaaS" here means the MCP server / relay is hosted in the cloud, not that the automation runs in the cloud. For contrast: xiaohongshu-mcp is "spin up a local headless browser + go-rod", x-mcp is "your real browser + extension" with the MCP server end moved to the cloud, different mechanism, but both belong to browser automation. Its tool interface (`xhs_*`) mirrors xiaohongshu-mcp. The pitch is zero deployment, reusing your everyday login state, and being able to watch and even intervene in the browser; onboarding is a single `claude mcp add --transport http`. This GitHub repo has almost no source (just README, SKILL.md, an onboarding guide, and a privacy policy), the one commercialized form in this comparison.

### xpzouying/xiaohongshu-mcp — the self-hosted MCP server benchmark

Browser automation, specifically go-rod (CDP) + stealth + a separate headless browser. It reads data via `MustEval` on `__INITIAL_STATE__`, writes through the DOM, and does search filtering by clicking filter tags by position. The interface is MCP over StreamableHTTP (Gin mounted at `/mcp`, default port 18060, and notably not stdio, so it can be containerized, there's a Docker image). The toolset is complete: publishing supports scheduling, original-content declaration, visibility range, and product binding; detail supports scrolling to load all comments and expanding nested replies. It's the most influential Xiaohongshu tool of the second-half-2025 MCP wave, and x-mcp and autoclaw above are both its descendants. Aimed at developers who can self-host a Go service / Docker, and their AI clients.

### NanmiCoder/MediaCrawler — a multi-platform collection framework

Not Xiaohongshu-only, it's a multi-platform scraping framework spanning Xiaohongshu, Douyin, Kuaishou, Bilibili, Weibo, Tieba, and Zhihu, with Xiaohongshu just a submodule under `media_platform/xhs/`. Its route has changed several times: early on it injected via Playwright to get signatures, now it outsources signing to xhshow's pure algorithm, while still keeping a CDP mode that connects to a local Chrome for stronger anti-detection, the several signing implementations coexisting in the code are fossils of that history. Its engineering completeness is the highest of the seven: an IP proxy pool, seven storage backends, login-state caching, comment word clouds, concurrency control, and a FastAPI + Vite WebUI. It's positioned for developers / data analysts doing bulk offline collection, essentially read-only. There's a closed-source MediaCrawler Pro alongside it (no Playwright, resumable crawls, multi-account, a content-deconstruction agent), the classic "open-source for funnel, closed-source for money".

### ReaJason/xhs — the ancestor, an HTTP client library

The oldest of the seven (April 2023), the `XhsClient` library on PyPI, which the author describes as "practicing Python". Reverse API, pluggable signing. Its historical significance is that it defined a shared vocabulary for everyone after it: the `Note` / `FeedType` / `SearchSortType` enums, helpers like `get_imgs_url_from_note`, widely reused, with MediaCrawler crediting it mutually. It covers search, detail, comments, profiles, and a full publishing chain (including chunked upload). Now inactive.

## 5. Ranking the ban risk, and the manual work you still have to do

After all the technical talk, users really care about just two things: will I get banned, and how much do I have to do by hand.

### What actually gets you banned

Ban risk comes down to two factors, both tied to "which route you take":

1. Whether your requests look human, pure HTTP (reverse API) lacks b1 and lacks a real browser's TLS and JS-runtime fingerprint, so it's the easiest to spot; letting a real browser sign (browser automation) comes with the full set of environment traits and is markedly less likely to be flagged. This is one of the deeper reasons the reverse route went quiet and the browser route rose.
2. Action type and frequency, read < interact < publish, low-frequency < high-frequency. Bans almost always happen on high-frequency writes, and that has little to do with which route you took.

From that, a ban-risk ranking (low to high), all assuming low frequency, restraint, and a healthy account:

- Lowest: real browser + read-only (socai's current form; autoclaw's read mode), closest to a real person browsing, though repeatedly opening notes in bulk can still trip risk control.
- Lower: moderate writes on the browser route (xiaohongshu-mcp / XiaohongshuSkills, assuming you control frequency).
- Medium: high-frequency writes from any tool (bulk publishing / interaction are both dangerous).
- Higher: pure-HTTP reverse (ReaJason/xhs, xiaohongshu-cli), fingerprint and rhythm expose you most easily, which is why xiaohongshu-cli had to invest so heavily in Gaussian jitter and cooldowns to compensate.

> In one line: ban risk is roughly "how human your requests look" times "how often you write". Which route you take affects the first; how restrained you are affects the second.

### Once it's set up, what you still have to do by hand

Now that everyone runs things through an agent, "can you code / do you know Docker" is no longer the main barrier, the agent can run commands and read errors for you. What really decides the experience is how many steps in the whole flow still require you, the human:

- Can you get it working in one go: do the dependencies install, do the port / extension connect on the first try, or do you fiddle endlessly.
- Do you have to scan / verify by hand often: how often the login expires, whether expiry means pulling out your phone to scan again and clearing a slider, the most frequent, most flow-breaking manual bit.
- Do you have to click a confirmation each time: whether an automated action triggers a browser / page confirm dialog that a human has to click.
- Can it run unattended on repeat: can tasks queue and run one after another, or does it get cut off by risk control after a few and need a human to reset.

By that standard the differences are big: pure-HTTP tools have low manual overhead once working, but post-inactivity "getting it working" is itself hard and brittle; the browser route requires a first scan-to-login (then reuses login state), and a separate profile or an attached real browser keeps login state longest and scans least; and any tool, the moment it hits a captcha / risk control, sees its "remaining manual work" spike, which loops back to the previous section: fewer high-frequency writes, fewer interruptions. socai attaches to your existing Chrome profile by default, so login state is the smoothest; for isolation, `socai config set chrome.profile managed` switches to a separate profile (you scan to log in there once first).

### These tools and gray-market account farms are not the same thing

Zoom out a bit: this batch of open-source tools and "industrial-scale gray-market operation of Xiaohongshu" are two different worlds, the latter is who the platform's risk control is really up against. Drawing this line clearly helps you see the ceiling on what this batch of tools (socai included) can do and how risky they are.

| Dimension | This batch of open-source tools | Gray-market farms |
|---|---|---|
| Protocol layer | web (`xiaohongshu.com`, x-s signing) | mostly app-side protocol (reverse the APK, protobuf, stronger hardening) |
| Devices | one real device / one browser | device farms (group / cloud control), device-spoofing frameworks, hooked fingerprints |
| Accounts | one or a few | account pools (hundreds to thousands) + SMS platforms + account warming |
| Network | local IP or a few proxies | residential proxy pools, 4G SIM banks, one IP per device |
| Goal | personal research / ops / learning | inflating numbers, warming accounts, bulk ads, data resale |

Three key differences: (1) web vs app, the open-source tools almost all target the web (more info, signing relatively reversible), while the gray market more often cracks the app protocol; (2) single point vs farm, open source is "one real person's automation", the gray market is "thousands of fake people industrialized", whose core assets are the scaled supply of devices / accounts / IPs and device-spoofing ability, not the signature itself; (3) intensity, the gray market is in a full-time arms race with platform risk control, while the open-source tools merely "try to look human". This batch all sits on the "personal scale, real identity" side, which means they shouldn't touch the device-spoofing / account-pool stuff, and their product boundary should be designed around "low frequency, explainable, human-can-take-over".

## 6. socai: a low-ban-risk Xiaohongshu research tool

In short, socai is the rare tool in this batch built for reading and research rather than publishing and ops. It attaches to your real, logged-in Chrome by default and stays read-first, so it sits in the lowest tier of the ban-risk ranking above; it's also the only one that truly understands content, beyond grabbing text, it does image OCR and video transcription, exactly the capability that makes topic research and competitor analysis valuable. It's not just a library or a script but a local product that unifies a CLI, a TUI, and a desktop app. Looking ahead, multimodal plus parallel multi-agent work can turn "research a Xiaohongshu topic" from reading notes one by one into deep-reading N in parallel and handing you an insight. If you want to read a topic thoroughly at low risk, that's exactly what it's for, [see socai on GitHub →](https://github.com/socai-io/socai).
