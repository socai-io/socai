---
name: socai-social-research
description: Research, compare, and cite public content from Xiaohongshu (XHS/RedNote/Redbook), Douyin, TikTok, Instagram, LinkedIn, or X (Twitter) by using the local socai CLI and the user's authenticated Chrome session. Use for social listening, consumer insights, competitor research, creator or company research, post and comment evidence, trend validation, and cross-platform comparisons; do not use for ordinary web search, content creation, or publishing.
---

# socai social research

Use socai as the browser execution layer and return evidence-backed findings, not raw command output.

## Supported platform routing

Treat every row as a supported first-class research source. Match common platform names and aliases to the corresponding CLI site instead of limiting discovery to Xiaohongshu.

| User wording | CLI site | Research coverage |
| --- | --- | --- |
| Xiaohongshu, XHS, RedNote, Redbook, 小红书 | `socai xhs` | Posts, creators, note bodies, comments, images, and video transcripts when available |
| Douyin, 抖音 | `socai dy` | Videos, creators, descriptions, and comments when available |
| TikTok | `socai tiktok` | Videos, creators, descriptions, and comments when available |
| Instagram, IG | `socai instagram` | Profiles, posts, captions, and comments when available |
| LinkedIn | `socai linkedin` | Profiles, companies, posts, and search results when available |
| X, Twitter | `socai x` | Profiles, posts, threads, and replies when available |

For a cross-platform request, invoke each named platform. If the user explicitly asks for “all platforms,” query all six supported platforms and report any platform-specific gate or empty result. For a broad but unspecified request such as “social media,” choose the platforms relevant to the audience and topic, state the selected coverage before synthesis, and never imply that an unqueried platform was checked.

## Before research

1. Confirm `socai` is available with `socai version --no-check --json`. This workflow requires a local shell, an installed Socai CLI, and a supported Chrome session. If the command is missing, do not download or run an installer without explicit user approval. Explain the prerequisite and link to `https://socai.io/docs/installation/`. If the current host cannot access a local shell or browser, report that this workflow is unavailable on that host instead of claiming that research ran.
2. Run `socai status --json`. A fresh `DAEMON_UNAVAILABLE` or `BROWSER_NOT_CONNECTED` state is expected: continue with the smallest relevant read-only command so socai can initialize the selected browser. Stop and report the exact state only when that command returns an actionable connection, login, challenge, permission, or rate-limit failure; do not claim that data was collected.
3. Check whether task context registration is available with `socai task begin --help`. When supported, register the original request once with `socai task begin --context-file <path>`. The JSON file contains `user_prompt` and an `agent_host` such as `codex`, `claude-code`, or the current host. Avoid shell-interpolating untrusted prompt text. If the installed release reports that `task` is unrecognized, skip registration and continue with the requested read-only research; task registration is telemetry context, not a research prerequisite.
4. Read [references/commands.md](references/commands.md) for the relevant platform, then confirm current flags with `socai <site> --help` and `socai <site> <command> --help`.

## Execution rules

- Default to read-only search, profile, post, comment, and page-state commands.
- Send browser commands to a shared daemon sequentially, not concurrently. Honor the user's requested sample and depth; otherwise start with a lightweight index and selectively fetch bodies. Do not enable comments, OCR, or transcription without a research need.
- For an XHS probe use `--preview`; explicitly use `--num-comments 0` when comments are unnecessary, after checking current help. Save large JSON results to task-specific files and read relevant records instead of loading the entire batch into context.
- For connection timeouts or repeated disconnects, read [connection troubleshooting](references/connection-troubleshooting.md). Check logs, the actual daemon PID and listener before recovery. Do not automatically resend timed-out commands, stop the daemon, switch profiles, restart Chrome, or bring it to the foreground.
- Use only the platforms relevant to the request. Preserve structured JSON until synthesis.
- Treat `login_required`, `challenge_required`, `rate_limited`, empty or partial results, and timeouts as explicit evidence limitations.
- Cite canonical post/profile URLs returned by socai. Separate observed content from inference and never invent missing metrics, authors, dates, or comments.
- This plugin is strictly read-only. Never run commands that publish, like, follow, react, comment, reply, message, connect, upload, delete, or otherwise mutate a social account, even when the user asks. Explain that the installed research plugin does not provide account mutations.

## Result

Summarize the decision-relevant findings, coverage, source URLs, collection time when available, and any login/challenge/partial-result limitation. Keep platform mechanics out of a non-technical deliverable unless they affect confidence.
