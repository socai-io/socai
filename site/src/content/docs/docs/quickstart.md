---
title: Quickstart
description: Run your first source-linked social media research task with socai.
sidebar:
  order: 2
---

## What you need

- Google Chrome with the social platforms you want to research already signed in.
- The socai desktop app or CLI.
- A concrete research question: market, audience, creator, customer language, or trend.

## 1. Install socai

For the lowest-friction start, download the desktop app:

- [macOS](https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg)
- [Windows](https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe)

CLI users can follow the complete [installation guide](/docs/installation/).

## 2. Connect your Chrome session

Open socai and follow the connection instructions. The first connection may ask you to enable Chrome remote debugging and confirm Chrome's permission prompt.

The default `existing` profile mode reuses your everyday Chrome session and its existing platform logins. See [Connect Chrome](/docs/browser-connection/) for managed and remote modes.

## 3. Ask a research question

In the desktop app or terminal interface, start with a bounded question and state the evidence you expect:

```text
Compare how people discuss sugar-free tea on RedNote, Douyin, and TikTok.
Identify recurring purchase criteria and cite specific posts, comments, and replies.
```

socai will choose the relevant platform operations, open real results, collect evidence, and write an answer with source links.

## 4. Run a CLI task

Every new CLI task begins with the original user question, followed by platform
commands. Register once per task; changing keywords or platforms within the same
task does not require another registration:

```bash
socai task begin "Research the gear purchases first-time campers regret."
socai xhs search "beginner camping gear mistakes" \
  --num-notes 10 \
  --num-comments 8 \
  --pretty

socai instagram search "camping gear regrets" --num 20 --pretty
```

The next user task starts with another `task begin`. The daemon automatically
groups intervening commands. See the [CLI workflow](/docs/cli/) for long-question
input and task boundaries.

Run `socai <platform> --help` to inspect the current command surface.

## 5. Review the evidence

Open the returned source URLs before using a high-impact claim. The desktop app keeps task history and artifacts; CLI runs can also preserve exact tool and model records in the configured runs directory.

Next, learn how to [shape an agent workflow](/docs/agent-workflows/) or inspect the [evidence model](/docs/evidence/).
