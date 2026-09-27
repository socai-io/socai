---
layout: ../../layouts/BlogPost.astro
lang: en
title: "What local means for a social research agent"
description: "A concrete data-flow map for Jev Social: what stays on your machine, what reaches the model provider and social platform, which credentials socai never receives, and how to keep report synthesis local."
date: 2026-09-28
dateLabel: September 28, 2026
readingTime: 6 min read
alternates:
  - hreflang: zh
    href: /blog/zh/jev-social-privacy-boundary
faq:
  - q: "Does local browser research mean there are no network calls?"
    a: "No. Chrome and socai contact the selected social platform, and the default Jev and report paths contact the configured model provider. Local means the application, browser session and saved run artifacts remain under your control; it does not mean the network is unused."
  - q: "Does the socai child process receive my model-provider key?"
    a: "Jev Social does not place OPENROUTER_API_KEY, TYPESAFE_API_KEY, SOCAI_API_KEY or session-token variables in the socai child environment. The child still receives the Chrome, CDP and socai-directory settings required to operate the selected browser session and is not a filesystem sandbox."
  - q: "Can report synthesis avoid a second model-provider request?"
    a: "Yes. Set OPENROUTER_REPORT_MODEL=off to use the deterministic local report path. Jev decisions still use the provider you configured unless you also select a loopback System One endpoint."
---

“Runs locally” is too vague for a browser agent. It can mean the UI opens on your laptop while prompts, cookies, page content and reports still travel through several services.

The useful question is not whether an app is local. It is **which process receives which data, for what purpose, and where the result is kept**.

We documented that boundary for [Jev Social](https://github.com/socai-io/jev-social), the open-source research app that lets Jev choose read-only steps while the local socai CLI searches Instagram, TikTok and LinkedIn in your signed-in Chrome session. Here is the shorter map.

## Four separate data boundaries

A Jev Social run has four relevant destinations:

1. **The local Jev Social process** owns the research loop, validates Jev's typed choice and writes run artifacts.
2. **The configured model provider** receives bounded decision input and, by default, a separate report-synthesis request.
3. **Chrome, socai and the social platform** perform the requested search or read operation using the browser session you selected.
4. **Local disk** keeps the captured evidence, run history and generated report until you remove them.

Calling the whole path “local” hides the boundaries that matter. The browser remains local, but Instagram or TikTok still receives ordinary browser requests. The app runs locally, but hosted Jev still needs a model-provider request.

## What Jev Social excludes from the socai child environment

Jev Social launches each `socai` command with an explicit environment allowlist. It does not place `OPENROUTER_API_KEY`, `TYPESAFE_API_KEY`, `SOCAI_API_KEY` or session-token variables in the child environment.

That is credential filtering, not a process sandbox. The child still receives the Chrome, CDP and socai-directory settings needed to operate the selected browser session, and it can access files allowed to the current user.

It also starts with `SOCAI_TELEMETRY=0` unless the launching environment contains the exact opt-in value `1`. That setting covers capability checks and browser operations started by Jev Social, including commands that reuse an existing socai daemon.

This process boundary is narrower than a machine-wide promise. A separately launched socai Desktop process has its own environment and is not changed retroactively by Jev Social. Configure that process independently if you use it alongside the demo.

## What reaches Jev and the report model

During onboarding, Jev Social can send an OpenRouter key to `https://openrouter.ai/api/v1/auth/key` for validation. Using a local decision endpoint and a deterministic report later does not erase that earlier validation request.

For each decision, Jev receives the research goal, requested platform, the current typed action labels, observed source URLs, summaries of previous steps and a bounded excerpt from captured records. It does not intentionally receive browser cookies, downloaded media, raw socai JSON or local filesystem paths.

Report synthesis is a separate request. By default it sends a bounded, sanitized evidence projection: the goal, platform, run status, coverage counts and up to 40 records with source URLs, titles, claim excerpts, a small number of comment excerpts, engagement fields and capture depth.

Set this in the project environment when the report should stay on the deterministic local path:

```bash
OPENROUTER_REPORT_MODEL=off
```

That disables the synthesis request. It does not change the Jev decision destination. To keep those decisions on the machine too, configure Jev Social with an explicit loopback `JEV_SOCIAL_SYSTEM_ONE_URL` backed by a compatible local server.

## Where the browser session stays

Chrome cookies and login state stay in the Chrome or socai profile chosen by the installed socai CLI. Jev Social does not read or copy the browser cookie store directly.

The browser still contacts the selected social site. Supported operations are read-only with respect to remote social state: search, open, inspect and read. They can still make network requests and write local files. TikTok media download is exposed only when the user's goal explicitly asks to download, save, archive, grab, capture or record media, or to keep a local or offline copy. That choice is recorded as `downloadMedia: true`; there is no second confirmation after the explicit request.

For sensitive research, use a separate browser profile or test account. Browser login gates, challenges and platform rate limits still apply.

## What remains after the run

Jev Social can write two separate local stores. `JEV_SOCIAL_HOME` contains `config.json` and per-run JSON; a key entered interactively during onboarding may be saved in `config.json`. socai keeps its own run directories, which can contain browser evidence and downloaded media. The one-hour expiry of a Jev Social media URL does not delete the underlying socai file.

Neither store has automatic cleanup. Files can remain until you delete them. After stopping Jev Social, inspect the specific Jev Social run JSON and specific socai run directories you no longer need; do not use a broad recursive deletion against either configured state root.

Provider-side storage is a different boundary. OpenRouter and the selected model provider govern their own storage and retention. A local model server can also have its own logs. Jev Social cannot turn those policies into local-disk guarantees.

## A practical privacy setup

For the narrowest current path:

1. Use a dedicated Chrome profile for research.
2. Keep the default `SOCAI_TELEMETRY=0` child-process setting.
3. Use a loopback System One endpoint if Jev decisions must stay local.
4. Set `OPENROUTER_REPORT_MODEL=off` if report synthesis must stay local too.
5. Review both Jev Social and socai artifacts, then delete only the specific runs you no longer need.

The complete field-level contract, current limitations and removal paths live in the [Jev Social privacy and data-flow guide](https://socai-io.github.io/jev-social/privacy/). The point is not a one-word privacy label. It is a boundary you can inspect before running the research.
