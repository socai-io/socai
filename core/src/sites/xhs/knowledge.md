# Xiaohongshu Macro-Agent Knowledge

Xiaohongshu / 小红书 / XHS is a Chinese lifestyle social platform. Posts are
called notes (笔记) and usually include title/body text, hashtags, engagement
counts, comments, media, and author/profile context.

## Runtime Assumption

socai prepares the browser/session/login state before the app/TUI agent run.
You may assume an authenticated XHS session is available, but you must not
assume any current page, modal, scroll position, selected filter state, or
clicked card.

## Macro-First Rule

Use self-contained macro tools. Do not try to maintain browser UI state in your
reasoning. Each macro tool should contain all inputs needed to navigate and
collect evidence internally, then return structured results and artifact
references.

The default interactive XHS tools are intentionally high level:

- `search_scan` — the single topic/search macro. It replaces the older
  `search_notes` card-only path and `topic_scan` detail path.
- `author` — author/profile macro. This refactors the existing author scan
  behavior under the same name as the CLI command.

Stateful micro tools such as opening/closing a current note, scrolling a note,
extracting the current modal, or reading current page state are not part of the
normal app/TUI agent workflow.

## Anti-Bot / Rate-Limit Guardrails

Macro tools navigate through search/profile/card flows because direct detail
URLs and rapid repeated reads are more likely to trigger Xiaohongshu security
states. If a macro reports security verification, captcha, login/session loss,
blank/404/app-only detail pages, or copy such as "帖子不见了" / "内容无法展示" /
"page unavailable", do not loop the same macro with identical inputs. Treat it
as a platform/session/rate-limit blocker: use whatever evidence was collected,
try at most one narrower or slower query/profile path if it materially changes
the route, and otherwise explain the blocker to the user.

## Tool Use

### Topic / search research

Call `search_scan(query=..., num_notes=N, filters=..., preview=false)` when the
task asks about a topic, keyword, market, trend, product category, or group of
XHS posts.

`search_scan` searches, optionally applies filters, samples notes from results,
reads note bodies and top comments by default, writes artifacts, and returns a
compact bundle. Set `preview=true` only when card metadata is enough and note
bodies / comments are unnecessary. Default `num_notes` is modest; increase it
only when the question needs broader evidence. Use `download_media=true` when
the user explicitly needs local image/video files.

### Author / creator research

Call `author(author_id=..., profile_url=..., num_notes=N,
read_notes=true|false, download_media=...)` when the task asks about a specific
author/creator and you have an `author_id`, can pass a profile URL containing
`/user/profile/<id>`, or a previous search result surfaced an author id worth
expanding.

If the user only gives a display name or handle, first discover candidates with
a focused `search_scan` or ask the user for the profile URL/author id.

Use `read_notes=true` when you need the author's recent note bodies/comments;
otherwise the profile header plus note cards may be enough. For author media
downloads, `download_media=true` only matters when `read_notes=true`, because
media is downloaded while reading notes.

### Note details

A standalone note-snapshot macro is deferred for V1 because direct note opening
can require tokenized URLs or source card context. Prefer note details already
collected by `search_scan` or `author` artifacts. If note details are missing,
run a focused `search_scan` or `author` that can reach the note via
search/profile context.

## Artifact-First Reasoning

Reason from previous macro outputs and saved artifacts. Use returned counts,
ids, source URLs, warnings, and artifact references to decide whether more
evidence is needed. Do not repeat the same macro call unless the previous result
was insufficient, partial, or used the wrong query/author/depth.

Full comment objects and untrimmed bundles may live in run artifacts even when
the returned tool payload is compact. Media downloaded with `download_media`
lives locally in the run directory and is referenced from manifests.

Some cards or note entries may be marked as previously analyzed (for example
`already_analyzed`, `history_level`, `history_include_media`, `skipped`, or a
`history` object). That means socai has durable cached evidence for that note
from this or an earlier run. Use the returned cached entity/history as evidence
instead of assuming the note was ignored. Only rerun with deeper settings when
the cached level/media setting is insufficient for the current task.

## Evidence Rules

- Ground final answers in collected XHS evidence.
- Distinguish note body text, comments, author profile facts, engagement, and
  media observations.
- If a macro returns partial success, use the collected evidence and clearly say
  what was missing.
- If a macro reports login/session/security/route/DOM failures, do not pretend
  the data was collected; explain the blocker and what would be needed next.
- Reply in the same language as the user's task.
