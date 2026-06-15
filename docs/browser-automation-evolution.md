# Browser Automation on CDP — A Knowledge Reference

*Written 2026-05-15. Captures the state of the browser-automation ecosystem
as of that date; revise when frameworks evolve.*

A working reference for how the Chrome DevTools Protocol (CDP) works and how
browser-automation frameworks have evolved on top of it through 2026.

This document is not a tutorial; it's a conceptual map. Read it linearly the
first time to build the mental model, then return for specific topics via the
table of contents.

## Contents

1. [Chrome DevTools Protocol (CDP)](#1-chrome-devtools-protocol-cdp)
2. [Client libraries: Puppeteer and Playwright](#2-client-libraries-puppeteer-and-playwright)
3. [The pivot to agents](#3-the-pivot-to-agents)
4. [Agent frameworks](#4-agent-frameworks)
5. [Cross-cutting axes](#5-cross-cutting-axes)
6. [Process lifecycle patterns](#6-process-lifecycle-patterns)
7. [Where socai lands](#7-where-socai-lands)
8. [Appendix A — Memorable framings](#appendix-a--memorable-framings)
9. [Appendix B — Open research questions](#appendix-b--open-research-questions)

---

## 1. Chrome DevTools Protocol (CDP)

Everything else in this document is, ultimately, a way of sending CDP messages
to Chrome. Get this layer right and the rest of the stack falls out cleanly.

### 1.1 What CDP is

CDP is a **protocol specification** — not a library, not a tool. It defines:

- **A JSON message shape**: `{"id": 1, "method": "Page.navigate", "params": {"url": "..."}}`
- **A set of domains** (~50), each grouping related operations and events.
  The big ones:
  - `Page` — navigation, lifecycle events, screenshots, PDF
  - `DOM` — query, mutate, observe the document tree
  - `Runtime` — evaluate JS in a frame's V8 context, console messages, exceptions
  - `Network` — every request/response, headers, timing, response bodies
  - `Input` — synthesize mouse, keyboard, touch events
  - `Target` — manage tabs, workers, out-of-process iframes, sessions
  - `Fetch` — pause, modify, mock, or abort requests in flight
  - `Accessibility` — read the a11y tree (ARIA roles, accessible names)
  - `Storage` — cookies, IndexedDB, localStorage, cache

Transport: WebSocket (default), or a Unix-style pipe via
`--remote-debugging-pipe` (lower overhead, used by Puppeteer/Playwright by
default). The transport is incidental; CDP is the JSON shape on the wire.

CDP was built originally for **Chrome DevTools itself** — the F12 panel
uses CDP to talk to the renderer. It predates Puppeteer (2017) by years.
Puppeteer's contribution was *exposing* this internal protocol to outside
scripts, not inventing it.

### 1.2 What CDP exposes

A comprehensive control surface. Anything DevTools can show or do, CDP
can do programmatically:

- **Inspection** — DOM tree, computed styles, layout boxes, JS heap snapshots
- **Control** — synthesize input events, scroll, focus, navigate, reload
- **Observation** — every network request/response, every console message,
  every runtime exception, GC events, frame lifecycle events
- **Modification** — intercept and rewrite requests, inject scripts before
  document load, emulate devices, throttle CPU and network
- **Execution** — evaluate arbitrary JS in any frame's V8 context, capture
  the return value as a remote object

There is **no permission gate**. If you can reach the WebSocket, every
domain is available.

### 1.3 How connecting works

Most people picture a CDP "server" running somewhere on their machine.
There isn't one — the server is **inside Chrome itself**.

1. **Chrome itself is the WebSocket server.** When Chrome is launched with
   `--remote-debugging-port=N` (or `=0` for a random free port), Chrome
   opens a WebSocket server on its own IO thread, baked into the browser
   binary. No separate daemon.
2. **Chrome writes `<user-data-dir>/DevToolsActivePort`** — a two-line file
   with the port on line 1 and the WebSocket URL on line 2. This is the
   bootstrap handshake: Chrome telling its parent process where to connect.
3. **Your client reads that URL and opens a WebSocket.** Could be Node
   (Puppeteer), TypeScript (Playwright), Python (`websockets`), Rust
   (`chromiumoxide`), curl, anything.
4. **You send and receive JSON.** Methods get replies keyed by `id`;
   events arrive asynchronously; sessions can be opened per-target so
   messages route to the right tab, frame, or worker.

Lifecycle: there's no long-running daemon. Chrome is spawned by whoever
needs it (test runner, agent CLI, daily-driver double-click) and dies
either when its parent kills it or when the user quits.

### 1.4 Security model

**There is no authentication, by design.** The whole model is "you must be
a process on the same machine that knows the port." Specifically:

- The port binds to `127.0.0.1` only — remote machines can't reach it
- The port is random unless fixed via `--remote-debugging-port=N`
- There is no token, no handshake, no permission prompt

Chrome 111+ disabled the `/json/*` HTTP discovery endpoints — closing
DNS-rebinding holes where a malicious local webpage could enumerate
debuggable targets via localhost HTTP. Direct WebSocket connections
(using the URL from `DevToolsActivePort`) still work and remain the
standard path.

This is why most production CDP-based tools launch a **fresh Chrome with
a clean user-data-dir** rather than attaching to the user's daily browser:
your real profile has cookies, sessions, password autofills — a CDP
client can read all of it. (browser-harness is the deliberate exception;
see §4.4.)

---

## 2. Client libraries: Puppeteer and Playwright

If §1 is the protocol, §2 is the question *"how much should we hide it
from the user?"*. Puppeteer and Playwright are two answers — both speak
CDP underneath; they differ in how thick their opinion layer is.

### 2.1 Puppeteer (2017)

Chrome team's first public CDP client.

- **Node.js only, Chromium only.** Optimized for control over portability.
- **Speaks CDP directly** over WebSocket (or `--remote-debugging-pipe`).
- **~10× faster** than HTTP-polling predecessors — no driver process, no
  polling loops.
- **Established the pattern**: speak the browser's own protocol directly.

Puppeteer was the inflection point — the moment "automation via the
browser's own internal protocol" became a viable mainstream approach.

### 2.2 Playwright (2020)

Microsoft's successor, built by the original Puppeteer team.

- **Multi-browser** via patched Firefox (Juggler protocol) and WebKit
  (Apple's remote inspector + Microsoft patches). The Playwright API
  hides which protocol it's speaking.
- **Multi-language** — Node, Python, .NET, Java.
- **Entire E2E testing platform** built on top of CDP.

"Firefox" in Playwright actually means "Microsoft's patched Firefox" —
worth knowing if multi-browser claims matter.

### 2.3 How Playwright spins up Chrome

Identical mechanic to raw CDP, just packaged:

1. **Spawn a bundled Chromium child process** — not your daily Chrome.
   `playwright install` downloads its own Chromium build into a cache
   directory (`~/Library/Caches/ms-playwright/` on macOS, similar
   elsewhere), version-matched to the Playwright library.
2. **Pass `--user-data-dir=<fresh temp dir>`** so the profile is empty:
   no cookies, no extensions, no logged-in accounts to leak.
3. **Pass `--remote-debugging-pipe`** (Playwright's default — faster than
   WebSocket because it skips the framing) or `--remote-debugging-port=0`.
4. **Read the URL/pipe handle, start the CDP connection.**
5. **Send CDP messages** for every API call the user makes.
6. **On `browser.close()` or process exit, kill the Chromium child.**

The test script *owns* the browser's lifecycle. Spawn, work, tear down.
No long-running browser persists.

### 2.4 The abstractions Playwright provides

This is the substantive answer to *"what does Playwright add on top of
CDP?"*. It's not "ergonomics" alone — that would be a thin justification
for an entire library.

- **API ergonomics.** A click is many CDP messages — `DOM.getDocument` →
  `DOM.querySelector` → `DOM.resolveNode` → `DOM.getBoxModel` →
  `Input.dispatchMouseEvent` ×2, with coordinates computed by hand.
  Playwright wraps this as `page.click('button')`.

- **Cross-target / cross-frame coordination.** A modern page is the main
  frame plus N iframes (some out-of-process due to Site Isolation) plus
  service workers plus popups. Each is a separate CDP target with its
  own session ID. Stitching them together so `page.locator('foo')`
  finds `foo` regardless of which frame holds it is non-trivial protocol
  orchestration.

- **Auto-wait / actionability.** The killer feature — see §2.5.

- **Locators, not handles.** A locator is a *recipe* for finding an
  element; it re-evaluates the selector on every call. Survives SPA
  re-renders. (Puppeteer's `elementHandle` is a snapshot — goes stale
  the moment the page re-renders.)

- **Selector engines beyond CSS/XPath.** `text="Sign in"`,
  `role=button[name="Submit"]` (queries CDP's `Accessibility` domain),
  chained with `>>`, filtered with `:has-text()`. Role-based selectors
  steer tests toward accessibility-aware, redesign-resilient assertions.

- **Network interception.** `page.route('**/*.png', r => r.abort())`
  wraps CDP's `Fetch` domain. Mocking, rewriting, blocking — all
  declarative.

- **BrowserContext.** An incognito-like isolated session within one
  browser process. Cheap parallelism: many contexts in one Chromium.

- **Trace Viewer.** Captures CDP events, screenshots, DOM snapshots,
  and network at every step into a portable replay file. The viewer
  is a real app with a scrubber. Genuinely unprecedented debuggability.

- **Test runner (`@playwright/test`).** Parallelism, sharding, fixtures,
  HTML reports, VS Code extension. Playwright stops being a library and
  becomes a platform.

### 2.5 Auto-wait — the killer feature

Every Playwright action waits for the target element to be:

- **Attached** to the DOM
- **Visible** (`opacity > 0`, not `display: none`, not `visibility: hidden`)
- **Stable** (bounding box unchanged across two animation frames)
- **Hittable** (receives pointer events at its center — nothing on top)
- **Enabled** (not `disabled`)
- **Editable** (for typing actions)

Retries silently for up to 30 seconds. The user never writes `waitFor`
for these conditions.

This single feature eliminated ~80% of the flakiness that defined the
prior testing era. None of it is in CDP — CDP only tells you whether
the mouse event dispatched. Playwright runs the whole checklist on top.

### 2.6 Why testing teams converged on Playwright

QA / E2E teams paid a flaky-test tax for 15 years. Puppeteer mostly
punted on the wait problem — you wrote your own `waitForSelector` calls.
Playwright said *"every action implicitly waits for actionability"* —
and that one design choice turned out to be worth more than every other
feature combined.

Once teams stopped seeing intermittent CI failures, the rest of the
platform (locators, trace viewer, test runner) compounded into a
complete solution. By 2024–2025, Playwright had eaten the E2E testing
market. **The load-bearing decision was auto-wait.**

Puppeteer is the spartan cousin — closer to "expose CDP cleanly in Node."
Playwright is "an entire E2E testing platform built on top of CDP." The
contrast matters: you can build Playwright-like features on top of
Puppeteer, but Playwright shipped them as defaults, and defaults define
markets.

---

## 3. The pivot to agents

### 3.1 Playwright was designed for humans writing tests

Every Playwright value-prop assumes a developer at a code editor:

- **Auto-wait** — so test code is shorter and less fragile
- **Locators** — so tests survive design churn
- **Trace Viewer** — so humans debug failures post-hoc
- **Test runner** — so humans organize test files

What happens when the consumer isn't a human anymore? Most of these
become *wrong* defaults:

- Auto-wait *hides state* from an agent — but the agent's job is to
  perceive state and decide when to act.
- Locators silently retry — an agent wants "this element disappeared"
  as signal to plan differently, not paper over it.
- Trace viewer is post-hoc human debugging, irrelevant to a live agent.
- Test runner is project organization, irrelevant.

Every Playwright superpower for a test author is the wrong default
for an LLM-driven agent.

### 3.2 The four walls an LLM hits with raw Playwright

Playwright assumes a human wrote the code who has already seen the page.
The LLM has not seen the page; it only knows a URL. Handed Playwright
with nothing else, it fails at four things:

1. **Can't see the page.** Playwright has no "describe what's on the
   screen" function.
2. **DOM is too big.** A typical page is 30k–200k tokens of HTML — way
   over context budget.
3. **Selectors are precise but the DOM is messy.** Brittle classes like
   `button.css-1xy7zab9.MuiButton-root` are exactly what LLMs hallucinate
   constantly. LLMs are bad at constructing precise selectors and good
   at picking from labeled lists.
4. **No feedback loop.** After clicking, did anything happen?

Every framework in §4 is a different stance on which walls to plug, and
how. The walls are the conceptual scaffolding for everything that follows.

---

## 4. Agent frameworks

A second wave of frameworks emerged 2023–2026 to bridge LLMs and the
browser. Each one is a different answer to *"what does an LLM lack when
you hand it raw Playwright?"*

Walls plugged, summarized:

| Wall | browser-use | Stagehand | agent-browser | browser-harness |
|---|---|---|---|---|
| Can't see | Element extractor → text list | Playwright + AI annotations | A11y tree with refs | Agent picks the representation |
| Too big | Filter to interactive nodes | Same | A11y tree is small by construction | Agent compresses as needed |
| Hallucination | Index-based action vocab | LLM-resolved selectors, cached | Stable refs from snapshot | Agent uses refs/selectors/CDP directly |
| No feedback | Built-in agent loop | Caller's responsibility | Caller's responsibility | Caller's responsibility |

### 4.1 browser-use — Playwright-wrapping, framework-driven

- **Stack:** browser-use → Playwright → CDP → Chrome
- **Language:** Python
- **Browser control:** wraps Playwright behind `BrowserSession`. Supports
  `connect_over_cdp` for cloud / remote Chromes.
- **Page representation:** extracts interactive elements (buttons, inputs,
  links, forms) into a compact structured text list (~1–5k tokens for a
  typical page).
- **Action interface:** LLM emits high-level structured actions:
  `click_element index=12`, `input_text index=4 text="..."`, `scroll_down`,
  `go_to_url`, `done`. Framework dispatches to Playwright.
- **Where planning lives:** the LLM. Framework provides eyes, hands, harness.
- **Stance:** *"give the LLM a clean view of the page, hand it a list of
  things it can do, let it drive."*

If you tried to use Playwright directly from an LLM, you'd end up writing
exactly browser-use's harness — prompt template, action vocabulary, JSON
output parsing, error recovery, LLM provider abstraction. browser-use is
that, packaged.

### 4.2 Stagehand — Playwright-wrapping, developer-driven

- **Stack:** Stagehand → Playwright (or Puppeteer / Patchright) → CDP → Chrome
- **Language:** TypeScript (Python port exists)
- **Browser control:** wraps Playwright; can also accept a custom page
  object so you mix Stagehand calls with raw Playwright.
- **Page representation:** Playwright accessibility tree + AI-derived
  element annotations.
- **Action interface — four primitives:**
  - `act("click the login button")` — natural-language action
  - `extract("the price", z.number())` — typed structured-data extraction
  - `observe("find submit buttons")` — discovery; returns candidate actions
  - `agent({mode: "cua", model: "..."})` — full autonomous loop using
    a computer-use model

```ts
await stagehand.act("click the checkout button");
await stagehand.extract("the price", z.number());
```

- **What it adds (the design-churn problem):** selectors like
  `button[data-testid="checkout"]` break when designers rename. Stagehand
  resolves "checkout button" to a real element at runtime via LLM,
  surviving rename / DOM-rearrangement / button-text-change. Once
  resolved, it **caches** the selector so subsequent runs skip the LLM call.
- **Where planning lives:** the *developer* (writing TypeScript). LLM is
  a *runtime resolver* for ambiguous lookups, not a planner.
- **Stance:** *"developer writes intent in natural language; LLM resolves
  to deterministic Playwright actions; framework caches and replays."*

**Contrast with browser-use:**
- **browser-use** — LLM is the *driver*; framework gives it eyes and hands.
- **Stagehand** — Developer is the driver; LLM is a *runtime resolver*.

This is why Stagehand attracts QA teams (write tests that survive design
churn) while browser-use attracts builders (ship an agent that uses the
web). Same primitives underneath, different consumers above.

### 4.3 Vercel agent-browser — CDP-direct, externally-driven

**This one breaks the pattern.**

- **Stack:** Rust CLI → Rust daemon → CDP directly → Chrome
  (no Playwright, no Node.js)
- **Language:** Rust

**Architecture (three processes):**
- **Rust CLI** (short-lived) — parses command, sends to daemon, prints
  result, exits.
- **Rust daemon** (long-lived) — owns the CDP WebSocket + a stateful map
  from refs (`@e1`) to backend DOM nodes. Started on first command,
  persists indefinitely (auto-adopted by init via `fork→setsid→fork`).
- **Chrome** — launched by the daemon (default: Chrome for Testing).

**The snapshot + refs mechanism:**

1. `agent-browser snapshot` → daemon sends `Accessibility.getFullAXTree`
2. Chrome returns the accessibility tree
3. Daemon filters to interactive + visible nodes
4. Daemon assigns refs (`@e1`, `@e2`, ...) in traversal order
5. Daemon stores `ref → backendNodeId` map for this snapshot
6. Output:
   ```
   button  "Sign In"          [ref=e1]
   textbox "Email"            [ref=e2]
   textbox "Password"         [ref=e3]
   ```
7. Then `agent-browser click @e1` → daemon looks up backend node, sends
   `DOM.scrollIntoViewIfNeeded` + `DOM.getBoxModel` + two
   `Input.dispatchMouseEvent`s.

**Eight strengths:**

1. **Token efficiency.** A11y tree with refs is 10–50× smaller than DOM
   serialization. ~200–400 tokens for a typical page vs ~10k+ for raw
   HTML. Real fit-in-context impact across long tasks.
2. **Refs as multimodal currency.** `snapshot` text labels and
   `screenshot --annotate` visual boxes use the *same* labels — text
   and image representations are interchangeable.
3. **Determinism within a snapshot.** Agent picks `@e1`; click resolves
   to *that specific element*. If page mutates between snapshot and
   action, fail cleanly rather than silently retry.
4. **No framework opinions about state.** No auto-wait magic. Agent
   decides when state is ready.
5. **CLI is the right interface for tool-use models.** Function-calling
   LLMs are trained on shell-like invocations.
6. **Unix composability.** Pipe `snapshot` → LLM → `click`. No SDK lock-in.
7. **Rust footprint.** Single binary, ~10MB. No Node, no Python, no
   Playwright cache.
8. **ARIA-based selectors age better.** Role + accessible name = page's
   semantic contract; changes less than CSS classes or div nesting.

**Honest critique:**
- No auto-wait → agent must handle in-flight state (navigation,
  animations, popups)
- Daemon adds operational complexity (IPC, lifecycle, restart logic)
- Small ecosystem vs Playwright's thousands of edge-case patches
- A11y tree quality depends on the page — sparse on poorly-marked-up sites

- **Where planning lives:** the external agent (Claude Code, Codex, etc.).
- **Stance:** *"compact, ref-based tools an agent can drive
  deterministically, with no framework opinions imposed."*

### 4.4 browser-harness — CDP-direct, externally-driven, self-extending

**The newest and most radical entry.** Released April 17, 2026 by the
browser-use team. Hit 4,100+ GitHub stars in the first four days.

- **Stack:** Python script → CDP directly → user's existing Chrome
  (no Playwright, no Node.js, no fresh Chromium by default)
- **Language:** Python — **~592 lines total**
- **Browser control:** **attaches to your already-running Chrome.** No
  fresh Chromium spawned by default — the agent inherits all your
  logged-in sessions.
- **Invocation pattern:** Python heredoc
  ```bash
  browser-harness <<'PY'
  new_tab("https://example.com")
  wait_for_load()
  print(page_info())
  PY
  ```
- **Helpers:** thin Python functions (`new_tab`, `wait_for_load`,
  `page_info`, `list_tabs`, `switch_tab`, ...) plus a raw
  `cdp("Method.name", **params)` escape hatch exposing the entire CDP
  surface.
- **Setup instruction (verbatim from README):** *"Paste into Claude Code
  or Codex."*

**Two genuinely novel design choices:**

**1. Self-healing — the framework rewrites itself.**
When the agent hits a missing capability mid-task (e.g., drag-and-drop
file upload), it writes a new Python function into
`agent-workspace/agent_helpers.py` and immediately uses it. The helper
**persists across runs.**

So the framework is **mutable** — it grows as the agent uses it. After a
month, your `agent_helpers.py` becomes a personalized library of "things
this user's agent has learned to do." A kind of meta-architecture that
only became possible because LLMs can write code.

**2. Real-Chrome attachment — inverts the security model.**
Every other framework launches a fresh Chromium with an empty profile.
browser-harness attaches to your daily-driver Chrome. The pitch: every
site you're already logged into is immediately accessible. The cost: the
agent inherits the full blast radius of your daily browser.

- **Where planning lives:** the external agent (Claude Code, Codex).
- **Stance:** *"thinnest possible CDP surface; let the agent grow the
  toolset as it goes."*

### 4.5 The bifurcation: two architectural camps

| | browser-use | Stagehand | Vercel agent-browser | browser-harness |
|---|---|---|---|---|
| Stack | Playwright | Playwright | Direct CDP (Rust) | Direct CDP (Python) |
| Size | ~10k+ LOC | ~10k+ LOC | ~10k+ LOC Rust | **~592 lines** |
| Language | Python | TypeScript | Rust | Python |
| Driver | Framework (own loop) | Developer (with LLM resolver) | External agent | External agent |
| Browser | Fresh spawn | Fresh spawn | Fresh spawn | **Your real Chrome** |
| Page repr | Extractor list | A11y + AI annotations | A11y tree with refs | Open — agent picks |
| Action vocab | Fixed | Fixed | Fixed CLI | **Self-extending Python** |
| Auto-wait | Built-in (via Playwright) | Built-in (via Playwright) | None | None |
| External-agent surface | MCP server | MCP server | CLI binary | Python heredoc |

**The two camps:**
- **Playwright-wrappers** (browser-use, Stagehand) — inherit Playwright's
  auto-wait, locators, multi-target coordination. Pay launch cost and
  Node runtime.
- **CDP-direct** (agent-browser, browser-harness) — build their own
  primitives, get predictability and smaller footprint, pay in DIY.

The team behind the most popular Playwright-wrapping framework
(browser-use) literally just launched a CDP-direct successor
(browser-harness). That's about as strong an industry signal as you can
get — even the leading practitioners of Playwright-wrapping are publicly
hedging toward CDP-direct.

### 4.6 The narrative arc

Each layer takes the one below it and reshapes it for a different
*consumer*:

1. **CDP** was built for **DevTools** (the F12 panel).
2. **Puppeteer / Playwright** reshape CDP for **humans writing tests**.
   Auto-wait is the killer feature.
3. **browser-use / Stagehand** reshape Playwright for **agents driving
   human-style test APIs**.
4. **Vercel agent-browser / browser-harness** reshape CDP directly for
   **agents**, with primitives designed for an LLM consumer from day one.

Each step is *a different consumer demanding a different abstraction*,
not *a newer tool than the last one*. The "evolution" is really the
story of those reshapings.

---

## 5. Cross-cutting axes

The frameworks differ on multiple independent axes. Three are worth
naming explicitly.

### 5.1 Axis 1 — Who drives the agent loop?

There are **three positions**, not two:

| Position | Who drives | Framework provides |
|---|---|---|
| **Framework drives** | The library's own LLM loop | Goal-in / result-out API. Hand it a task; it plans and executes. |
| **Developer drives** | Your code, with optional LLM help at runtime | Smart primitives (`act`, `extract`) that LLM resolves at runtime; *you* compose them. |
| **External agent drives** | A separate agent (Claude Code, Codex, custom) | A tool surface — CLI, MCP server, REST API, library to import. |

**Most frameworks span multiple positions.** Where each sits:

| Framework | Framework drives | Developer drives | External agent drives |
|---|---|---|---|
| browser-use | ✅ default (`Agent.run()`) | partial (`BrowserSession`) | ✅ via MCP server |
| Stagehand | ✅ (`agent({mode:"cua"})`) | ✅ **main mode** (`act/extract/observe`) | ⚠️ via Browserbase MCP server |
| Vercel agent-browser | ❌ | ❌ | ✅ **only mode** — CLI binary |
| browser-harness | ❌ | ❌ | ✅ **only mode** — Python heredoc |

agent-browser and browser-harness are the only ones that are *purely*
external-agent-driven. They explicitly don't ship an agent loop because
they don't want to bundle an opinion that will be obsolete in 6 months.

### 5.2 Axis 2 — One-shot vs multi-session (persistence vs lifecycle)

Two independent questions that get conflated constantly:

- **Lifecycle**: how long does Chrome stay alive?
- **Persistence**: do cookies / sessions survive Chrome restart?

These are **orthogonal**:

- **Cookies live on disk** in the Chrome user-data-dir
  (`~/Library/Application Support/Google/Chrome/...` or wherever
  `--user-data-dir` points). Always have, always will. Restart Chrome
  with the same user-data-dir → cookies reload.
- **Daemons don't preserve cookies** — they preserve the *boot cost* of
  launching Chrome. If you re-launch Chrome pointed at the same
  user-data-dir, you get the same cookies whether or not a daemon was
  involved.

So the real reason daemons matter is **fast startup**, not state
persistence. State is a user-data-dir concern; daemons are a
startup-cost concern.

### 5.3 Axis 3 — Agent CLI vs primitive CLI

A subtle paradigm split that's invisible because both surfaces are
called "CLI."

|  | Agent CLI | Primitive CLI |
|---|---|---|
| Examples | `browser-use --task "..."`, socai REPL | `agent-browser click @e1`, `agent-browser snapshot` |
| Granularity | One command = one whole task | One command = one atomic action |
| Who plans? | Framework's internal LLM | Caller (external agent or human) |
| Cost per call | ~$0.01–1.00 of LLM tokens + 30s–5min | A few ms (just IPC + CDP message) |
| Composability | Not composable — each call end-to-end | Pipe / loop / script freely |
| Stateful | One Chromium per CLI invocation (or REPL session) | All calls hit daemon-backed warm Chrome |
| Argument style | English sentences | URLs, selectors, refs |
| Mental model | *"do the thing for me"* | *"do this exact step"* |

**Rule of thumb:** if a CLI's arguments are English sentences, it's an
**agent CLI** for humans. If its arguments are URLs / selectors / refs,
it's a **primitive CLI** for external agents. They look similar; they're
built for opposite consumers.

browser-harness fits neither cleanly: its arguments are *Python heredocs*
of helper-function calls. It's a third paradigm — *"primitive CLI whose
primitive vocabulary can be extended at runtime by the agent itself."*

---

## 6. Process lifecycle patterns

Five patterns for "where does Chrome live in the process tree?" Each
pattern is a different answer to *"how do we keep Chrome warm across
calls without leaking state we shouldn't leak?"*

### 6.1 Pattern A — Detached daemon (Vercel agent-browser)

```
Claude Code (parent)
│
├── agent-browser CLI       ← short-lived, dies after each command
│
And SEPARATELY, detached from Claude Code's process tree:

└── agent-browser daemon    ← long-lived, started by first CLI call
    └── Chrome process
```

- CLI invocation either spawns the daemon (first time, via the classic
  `fork → setsid → fork` Unix daemon pattern) or connects via local socket.
- Daemon is adopted by init (PID 1), so it survives Claude Code exit.
- Browser persists across days.

**Survives Claude Code exit:** ✅
**Cleanup on agent quit:** manual (`agent-browser daemon stop`)
**Config:** none — daemon auto-starts on first command

### 6.2 Pattern B — MCP server (Playwright MCP, browser-use MCP, Stagehand MCP)

```
zsh (your shell)
└── claude (Claude Code, parent)
    │
    ├── npx @playwright/mcp@latest      ← MCP server #1 (child of Claude Code)
    │   └── chromium                     ← grandchild — Playwright spawns it on first browser tool call
    │       ├── renderer (tab 1)
    │       ├── renderer (tab 2)
    │       ├── GPU process
    │       └── network service
    │
    ├── uvx mcp-server-browser-use      ← MCP server #2
    │   └── chromium                     ← independent Chrome
    │
    └── ...other MCP servers
```

- Claude Code reads MCP config at startup; spawns each MCP server as a
  child process (standard `fork+exec`).
- MCP server stays alive for the duration of the Claude Code session;
  communicates over stdio (JSON-RPC).
- MCP server spawns Chrome on the first browser-related tool call. Tool
  calls reuse the existing stdio pipe — no new process per call.
- When Claude Code exits → SIGTERM cascades → MCP servers tear down
  their Chromes.

**Key clarification:** there is no "Playwright CLI" or "browser-use CLI"
that Claude Code shells out to. Both projects ship a *separate MCP
server binary* (`@playwright/mcp`, `mcp-server-browser-use`) specifically
so external agents like Claude Code can call them through the MCP
protocol. The SDK and the user-facing CLI are different artifacts.

**Survives Claude Code exit:** ❌
**Cleanup on agent quit:** automatic (cascading SIGTERM)
**Config:** declarative in agent settings file (`~/.claude.json` or equivalent)
**Cost:** each browser-related MCP server is its own Chromium →
500MB–1GB per server. Three browser MCP servers = 3GB RAM.

### 6.3 Pattern C — Attach to existing Chrome (browser-harness)

```
User's session (independent of any agent)
│
└── User's daily-driver Chrome        ← separately running with --remote-debugging-port


Claude Code (parent)
│
└── browser-harness invocation        ← short-lived, runs Python heredoc
    │
    └── (connects to Chrome via CDP WebSocket — NO parent-child relationship)
```

- User starts Chrome themselves (with debugging enabled).
- browser-harness script connects via CDP WebSocket, runs the heredoc, exits.
- Chrome is owned by the user's session.

**Survives Claude Code exit:** ✅ (Chrome was never tied to Claude Code)
**Cleanup on agent quit:** N/A — Chrome is the user's
**Blast radius:** full — agent inherits all user's cookies/sessions

### 6.4 Pattern D — Monolithic agent CLI (browser-use --task, socai TUI)

```
zsh (your shell)
│
└── browser-use --task "..." OR socai           ← agent loop runs in the CLI process
    │
    └── chromium                                  ← browser runtime / CDP connection
        ├── renderer (tab 1)
        ├── GPU process
        └── network service
```

- The CLI **is** the agent — LLM loop runs inside the CLI process.
- Chrome launched on startup, dies when CLI exits.
- Cookies persisted to disk if `user_data_dir` is set; lost otherwise.
- Two distinct sub-modes:
  - **One-shot** (`browser-use --task "..."`) — single task, Chrome lives
    only for that task.
  - **Interactive REPL** (`browser-use` no args, or socai REPL) — Chrome
    lives across many tasks in one CLI session.

**Survives CLI invocation:** ❌
**State across invocations:** only via `user_data_dir` on disk
**Suitable for:** one-shot terminal tasks; interactive REPL workflows

**Why Claude Code wouldn't want to call this as a tool:**
- 2–5s Chrome cold-start per invocation
- Each call is a fresh agent with no memory
- **Two LLMs in one task** — you pay for Claude Code's reasoning AND
  the inner agent's loop, duplicated. The MCP server pattern exists
  precisely to expose primitives without the inner loop.

### 6.5 Pattern E — Integrated product (socai desktop)

```
zsh or Finder (launcher)
│
└── socai.app (Tauri Rust shell)              ← the desktop product itself
    └── socai-core runtime                     ← agent loop + CDP + site tools
        └── Chrome CDP connection
            └── renderer / GPU / network
```

- Chrome lifetime = app window lifetime.
- Agent integrated into the product, not external.
- Multi-session is natural — the app stays open across days.
- No MCP boundary, no IPC overhead for tools.

**Survives invocation:** ✅ (until user quits app)
**State across sessions:** ✅ (user-data-dir lives in app's data directory)
**Unique to:** products, not libraries. Only viable if you own the
user-facing application.

### 6.6 Trade-offs across patterns

| Property | A: Daemon | B: MCP | C: Attach | D: Mono CLI | E: Integrated |
|---|---|---|---|---|---|
| Cold-start cost | Pay once ever | Pay once per Claude Code session | None | Pay every invocation | Pay once per app launch |
| Survives agent restart? | ✅ | ❌ | ✅ | ❌ | ✅ |
| State persistence | Disk + warm | Disk only | Disk (user's profile) | Disk (if user_data_dir set) | Disk + warm |
| Process complexity | High | Medium | Low | Low | Medium |
| Blast radius | Isolated profile | Isolated profile | **User's full profile** | Isolated profile | App's profile (your call) |
| External agent friendly? | ✅ (CLI) | ✅ (MCP) | ✅ (CLI/script) | ❌ (high per-call latency) | (only if exposed) |
| Best for | Power users w/ long-running flows | Production agent installs | Personal use with logged-in sessions | One-shot terminal tasks | Consumer desktop product |

---

## 7. Where socai lands

socai actually spans **three patterns** — and that's its differentiation.
Note the CLI is not one surface but two: an interactive *agent* REPL (TUI) and a
separate *agentless* tool CLI, which land in different patterns.

### 7.1 CLI agent mode — Pattern D (monolithic agent CLI)

`socai` with **no subcommand** drops into the Rust interactive TUI, which is
structurally a browser-use-style agent CLI:
- Single Rust process owns `SocaiRuntime`, including chrome's CDP connection.
- Interactive TUI — many tasks can share the process-local runtime.
- The LLM agent loop runs **inside** the process. It dies when the user exits
  the TUI.

### 7.2 CLI tool mode — Pattern A (detached daemon)

`socai search_notes` / `topic_scan` / `extract_note` are a *second, distinct*
CLI surface — **agentless** tool subcommands, not the agent TUI. Structurally they
are Vercel agent-browser's CLI+daemon split (§6.1), just at a higher
granularity (domain tool calls, not `click @e1`):
- The CLI invocation is short-lived: parse args → send one JSON request over
  `~/.socai/daemon.sock` → print result → exit.
- A long-lived **daemon** owns one persistent "tool tab" plus a cached
  `XhsRuntime`. It auto-spawns on the first tool call and auto-shuts after
  3h idle, so chrome stays warm across invocations (`socai stop` ends it early).
- **No internal LLM loop.** The caller — a human, a script, or an external
  agent (Claude Code, Codex) — decides which tool to invoke. This *is* socai's
  external-agent surface (see §7.4).
- Like the TUI, the daemon defaults to attaching to the user's already-logged-in
  browser/profile through `discover_existing_chrome_endpoint`. Users can opt into
  socai's managed chrome user-data-dir (`~/.socai/chrome-profile`) persistently
  with `socai config set chrome.profile managed`.

### 7.3 Desktop mode — Pattern E (integrated product)

The Tauri desktop app integrates everything:
- Rust shell + socai-core own chrome's CDP WebSocket, task state, site tools,
  and the agent loop.
- chrome lives as long as the app window is open.
- No MCP boundary; agent is part of the product.

### 7.4 Strategic differentiation

| Tool | Monolithic CLI | External-agent surface | Integrated product |
|---|---|---|---|
| browser-use | ✅ | ✅ (MCP server) | ❌ |
| Stagehand | ❌ | ✅ (MCP server) | ❌ |
| Vercel agent-browser | ❌ (the CLI *is* the tool surface) | ✅ (CLI + daemon) | ❌ |
| browser-harness | ✅ (the script *is* the harness) | ✅ (same script) | ❌ |
| **socai** | ✅ (REPL/TUI) | ✅ (daemon tool CLI) | ✅ |

**socai fills all three columns, and it's the only one with the
integrated-product column filled.**
That last column is the real strategic differentiation — libraries can't
build it because they're libraries, not products. The libraries' ceiling is
"be a great tool surface"; socai's ceiling is "be the product the user
opens to do work."

### 7.5 Open positioning questions

- **MCP server for external agents?** Should socai expose its tools to
  Claude Code / Codex via MCP? If yes, socai becomes a *platform* on top
  of being a product. If no, socai stays a pure product. Different
  roadmaps.
- **Profile model.** socai defaults to existing-browser attach for best login
  continuity, and now offers a persistent isolated user-data-dir
  (`~/.socai/chrome-profile`) as an opt-in config mode. The open question is
  whether to add per-task throwaway profiles for high-risk tasks.
- **Existing-browser attach blast radius.** Because the default can access the
  user's full browser profile over CDP, keep managed mode visible for users who
  prefer isolation.
- **Agent loop ownership.** socai ships its own loop in `core/src/agent/`.
  As external agent harnesses (Claude Code, Codex) grow more capable,
  will the integrated agent stay primary, or will users want socai's
  tools with their own loop?
- **Tool mutability.** browser-harness lets the agent extend its own
  helpers. Worth considering whether socai's tools should be agent-
  mutable in any way, or whether the integrated-product model means
  tools stay fixed and audited.

---

## Appendix A — Memorable framings

Aphorisms and mental models worth keeping in head, from across the doc.

- *"Chrome itself is the WebSocket server. There is no separate daemon."*

- *"CDP has zero authentication. The security model is: be the process
  that knows the port."*

- *"Every Playwright action waits for the element to be attached, visible,
  stable, hittable, enabled. That single feature ended the flaky-test
  era."*

- *"Playwright's value props were designed for humans writing tests. They
  are wrong defaults for agents."*

- *"The team that built the most popular Playwright-based agent framework
  just shipped a non-Playwright successor in 592 lines of Python."*

- *"agent-browser's `@refs` are a stable currency between text snapshot
  and annotated screenshot. Same labels, two modalities."*

- *"Auto-wait is great for tests, wrong for agents. The same retry-silently
  behavior that kills flakiness in CI hides state from a reasoning agent."*

- *"There is no Playwright CLI for Claude Code to call. There is a
  Playwright MCP server, spawned as a child of Claude Code, exposing
  Playwright's primitives over JSON-RPC over stdio."*

- *"Cookies live on disk. Daemons save you the boot cost, not the state."*

- *"If a CLI's arguments are English sentences, it's an agent CLI for
  humans. If its arguments are URLs and selectors, it's a primitive CLI
  for external agents."*

- *"Each era of browser automation is a different consumer reshaping the
  same protocol."*

---

## Appendix B — Open research questions

Things worth verifying or exploring further when the topic comes up again.

- **Concrete CDP-message example** — one click action, full CDP sequence
  side by side with the equivalent Playwright one-liner. The most
  effective "what does the abstraction buy you" visual.

- **A real `agent-browser snapshot` output** — concrete demonstration of
  token efficiency on a representative page (login form, search results,
  Gmail inbox).

- **socai profile lifecycle behavior** — verify how the opt-in managed
  `~/.socai/chrome-profile` behaves across crashes, multiple entrypoints, and
  explicit existing-browser attach endpoints.

- **Where Anthropic computer-use and Microsoft Playwright MCP fit** —
  two paths not deeply covered. computer-use is purely vision-based
  (screenshots + mouse coordinates, no a11y tree); Playwright MCP is
  Microsoft's MCP server wrapper around Playwright. Both are worth a
  closer pass when they intersect with socai's design choices.

- **WebDriver BiDi** — the W3C standard intended as CDP's cross-browser
  successor. Currently shipping in Chrome and Firefox. Watch whether it
  becomes the actual cross-browser CDP-equivalent or stays a parallel
  effort.

- **The two-LLMs cost issue** — when an external coding agent (Claude
  Code, Codex) calls a monolithic agent CLI, both reasoning layers are
  paid for. MCP servers are the architectural fix because they expose
  primitives without the inner agent loop. Worth tracking how this
  shapes the market.
