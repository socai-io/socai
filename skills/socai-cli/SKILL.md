---
name: socai-cli
description: Use the installed socai CLI for research on Xiaohongshu, Douyin, TikTok, Instagram, and LinkedIn. Use when an external agent needs real posts, comments, creator profiles, or source-linked social research through the user's signed-in browser.
---

# socai CLI research

Use `socai --help` and `socai <site> <command> --help` as the authoritative
command reference. `socai status --json` checks readiness without connecting
Chrome. If the binary is missing, follow the installation instructions in the
repository README. Do not substitute scraping or private APIs for socai.

## Start each new task once

This is the standard workflow for all platform CLI operations. The command
reference examples are operations inside a task, not alternative entry points.

Before the first site command for a new user request, pass the user's original
question to `socai task begin`. Keep their wording; do not replace it with a
summary, search keywords, or your plan.

```bash
socai task begin "Find why first-time campers regret their gear purchases."
socai xhs search "camping gear regrets" --num-notes 10 > /tmp/camping-xhs.json
socai instagram search "camping gear regrets" --num 20 > /tmp/camping-instagram.json
```

Ordinary commands automatically join the daemon's current task. No ID or extra
argument is needed. A new user goal starts a new task; searching another keyword,
reading a profile, or retrying within the same task does not. Register again
after the daemon restarts. Multiple agents using the same daemon share its most
recent task boundary; separate `SOCAI_HOME` directories create independent daemon
sessions when needed.

For multiline or shell-sensitive text, write a task-specific UTF-8 JSON file using
a file tool or JSON serializer, then run `socai task begin --context-file <path>`:

```json
{
  "user_prompt": "The user's original question",
  "agent_host": "unknown"
}
```

`agent_host` is optional; use the actual host identifier when known. Stdin is
supported with `--context-file -`. Remove the temporary input file after use.
If the original question is no longer available, use `user_prompt: null` rather
than reconstructing it. Do not read other conversations or submit system prompts,
attachments, or credentials. Registration does not connect Chrome.

On an older CLI that lacks `task begin`, continue ordinary research without
registration. Do not repeatedly retry or automatically upgrade. Failed
registration leaves the previous daemon task unchanged.

## Gather and deliver evidence

Choose among `xhs`, `dy`, `tiktok`, `instagram`, and `linkedin` according to the
user's scope. Inspect command help before using platform-specific options.
Redirect large JSON results into task-specific files and read relevant fields
selectively. Keep source URLs and distinguish collected evidence from inference.
Treat login, permission prompts, and platform verification as access limitations;
do not loop indefinitely when blocked.

Deliver the requested analysis or artifact, with sources and material gaps.
Keep research read-only unless the user explicitly requested a specific write.
