---
title: CLI reference
description: Use socai's structured platform commands from scripts and coding agents.
---

The CLI writes the final machine-readable command result to standard output. Interactive progress is rendered separately, so scripts and agents can parse the result without stripping progress lines.

## CLI workflow: begin a task, then call platform commands

Every new CLI task starts with the user's original question. This applies to
all platform commands, regardless of which agent invokes them.

```bash
socai task begin "Research why consumers repurchase sugar-free tea."
socai xhs search "sugar-free tea repeat purchase" --num-notes 10
```

Call `task begin` once per new user task, not once per command. All later
site commands automatically join the daemon's current task until the next begin
or daemon restart. For long or multiline prompts use `--context-file <path>` with
the following UTF-8 JSON, or `--context-file -` for stdin:

```json
{
  "user_prompt": "The original question"
}
```

The optional `agent_host` field identifies the caller. The file is an alternative
input method for the original question; it contains no summary or chat history.
See [Agent workflows](/docs/agent-workflows/) and `socai task begin --help`.

## Search commands

The examples below show individual operations within an already registered task.
Start a new task when the user's goal changes; do not register before every call.

```bash
socai xhs search "content marketing ideas" --num-notes 30 --num-comments 20 --pretty
socai dy search "coffee" --num 30
socai tiktok search "coffee" --num 30 --pretty
socai instagram search "coffee" --num 20 --pretty
socai linkedin search "product designer" --type people --num 20 --pretty
```

## Platform entry points

| Command | Typical operations |
| --- | --- |
| `socai xhs` | Search, authors, selected notes, comments, media, OCR, transcription |
| `socai dy` | Search, videos, authors, comments, media |
| `socai tiktok` | Search, videos, profiles, comments, media |
| `socai instagram` | Search, profiles, posts, Reels, comments, media |
| `socai linkedin` | People, company and content search; profiles, history, posts, comments |

The command surface evolves with platform changes. Treat built-in help as authoritative:

```bash
socai xhs --help
socai instagram --help
socai linkedin --help
```

## Xiaohongshu example

```bash
socai xhs search "Shanghai weekend activities" \
  --num-notes 20 \
  --num-comments 12 \
  --filter publish_time=一周内 \
  --filter note_type=图文 \
  --filter sort=最新 \
  --download-media \
  --ocr \
  --pretty
```

Use `--preview` when result-card metadata is enough and you do not want to open every post. Use `--debug-snapshot` only for development diagnostics because it writes page snapshots and screenshots.

## Preserve runs

Set a durable run directory when another process needs to inspect exact tool evidence:

```bash
socai config set runs.dir /path/to/socai-runs
```

See [Evidence and artifacts](/docs/evidence/) for the stored layout.
