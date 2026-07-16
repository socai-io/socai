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

- `search` — the topic/keyword search macro, and the single XHS search tool.
- `author_scan` — author/profile macro.

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

## Login Detection

Both macros run a pre-flight login gate: if logged out they return
`{ok:false, reason:"login_required"}` immediately (login is read from the
persistent sidebar, so a dismissed QR modal is never mistaken for a session).

**On `reason:"login_required"`, do NOT retry the tool.** Retrying just hits the
same wall. Instead: tell the user to scan the QR / sign in to Xiaohongshu in the
browser, and call `wait_for_login` (which opens the login page and blocks until
they're in). When it returns `logged_in:true`, re-run the original tool once and
continue. If it returns `logged_in:false`, remind the user and call
`wait_for_login` again.

## Tool Use

### `search` and `author_scan`

These two macros work the same way — collect up to `num_notes` note cards
(scrolling as needed), open each note for its body + top comments, write
artifacts, and return a compact bundle. They differ only in where they enter:

- `search(query=..., filters=...)` — enters from a keyword search. Use for a
  topic, keyword, market, trend, product category, or group of posts. `filters`
  are search-result filters (sort, note_type, publish_time, search_scope,
  distance).
- `author_scan(author_id=...)` — enters from one author's profile (also returns
  the profile header). Use for a specific creator. Needs an `author_id` (trailing
  segment of `/user/profile/<id>`); if the user only gives a display name/handle,
  discover it with a focused `search` first or ask for the profile URL.

For deep, complex topic research only, consider `author_scan` when a search
finds a high-quality, suspicious, or representative note. A quick profile check
can distinguish firsthand expertise from soft ads/content farms, uncover related
notes, reveal the author's recurring style and so on. Skip this extra step for routine
searches.

Shared options (same meaning for both):

- `num_notes=N` — how many notes to collect (default 10); raise only when the
  question needs broader evidence.
- `num_comments=N` — how many comments to load per note (default 5; replies
  count toward N). Use the default unless the question is about the discussion itself.
  N ≤ 12 usually loads without extra scrolling, larger values add scroll/expand rounds 
  and latency. `0` skips comments. Ignored in `preview` mode.
- `preview=true` — fast cards-only pass (titles/likes/covers), without opening
  any note. `download_media` is ignored in this mode.
- `download_media=true` — download note images/videos into the run dir; use when
  the user needs local files.
- `ocr=true` — also read text inside the images (local PP-OCRv6, offline, run
  pipelined behind the browse loop so it's near-free). Each note gets `ocr_text`
  as a per-image array (cover first); implies `download_media`; in `preview` mode
  it OCRs each card's cover only.
- `transcribe_audio=true` — transcribe a video note's audio in the cloud
  (socai pro), attaching `video.transcript`; a
  `transcript_error` saying no speech was detected means the video genuinely
  has no narration — report that as the answer.

### Note details

A standalone note-snapshot macro is deferred for V1 because direct note opening
can require tokenized URLs or source card context. Prefer note details already
collected by `search` or `author_scan` artifacts. If note details are
missing, run a focused `search` or `author_scan` that can reach the note via
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
- When citing a note link, including the `xsec_token=…` part. Otherwise a bare 
  `xiaohongshu.com/explore/<id>` link without xsec_token cannot be opened on desktop web.
- Distinguish note body text, comments, author profile facts, engagement, and
  media observations.
- If a macro returns partial success, use the collected evidence and clearly say
  what was missing.
- If a macro reports login/session/security/route/DOM failures, do not pretend
  the data was collected; explain the blocker and what would be needed next.
- Reply in the same language as the user's task.
