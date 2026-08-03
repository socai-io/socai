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
- `get_notes` — revisit specific notes by previously collected note id + xsec token.

Stateful micro tools such as opening/closing a current note, scrolling a note,
extracting the current modal, or reading current page state are not part of the
normal app/TUI agent workflow.

## Anti-Bot / Rate-Limit Guardrails

`search` and `author_scan` keep using search/profile card clicks for first-time
discovery. `get_notes` may navigate directly only when each note has the xsec
token previously returned by one of those flows. Direct detail reads and rapid
repeats can still trigger Xiaohongshu security states. If a macro reports
security verification, captcha, login/session loss,
blank/404/app-only detail pages, or copy such as "帖子不见了" / "内容无法展示" /
"page unavailable", do not loop the same macro with identical inputs. Treat it
as a platform/session/rate-limit blocker: use whatever evidence was collected,
try at most one narrower or slower query/profile path if it materially changes
the route, and otherwise explain the blocker to the user. For
`reason:"rate_limited"`, follow the protocol below.

## Rate-Limit Recovery

On `reason:"rate_limited"`, call `wait_for_rate_limit` instead of retrying.
After it returns, retry the original tool once; if still limited, repeat this
wait-then-one-retry cycle until success or cancellation.

## Login Detection

All three macros run a pre-flight login gate: if logged out they return
`{ok:false, reason:"login_required"}` immediately (login is read from the
persistent sidebar, so a dismissed QR modal is never mistaken for a session).

**On `reason:"login_required"`, do NOT retry the tool.** Retrying just hits the
same wall. Instead: tell the user to scan the QR / sign in to Xiaohongshu in the
browser, and call `wait_for_login` (which opens the login page and blocks until
they're in). When it returns `logged_in:true`, re-run the original tool once and
continue. If it returns `logged_in:false`, remind the user and call
`wait_for_login` again.

**Exception — `remote_browser:true` on the result:** the session runs in socai's
remote hosted browser and its login is operated by socai, not the user. Tell the
user remote browsing is temporarily unavailable on socai's side and to try again
later. Do NOT ask them to scan a QR and do NOT call `wait_for_login`.

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

Both flows keep opening notes by clicking their cards. Their card/note URLs
carry the xsec token needed for a later `get_notes` call.

While reading any result set, keep track of what each author is: the subject's
own official account (the gym/brand/venue/organizer itself), semi-official
voices (its staff or coaches), aggregator/curator accounts, or ordinary users.
Search ranking is relevance-based, not recency-based — one results page
routinely mixes fresh notes with ones from months or years ago, so compare
each note's `date` before treating any of them as current.

When the question concerns something its subject announces or decides itself —
schedules, prices, opening hours, events, rules, openings/closures — and an
account that looks like the subject's official account appears in the results,
do not answer from the search sample alone.
Run `author_scan` on that author_id: the profile lists the account's notes
newest-first (pinned 置顶 notes may sit on top), it confirms official status
via the verified/认证 header fields, and it reveals announcements that search
ranking missed. It reads only the first screen of the grid unless you pass
`num_notes` — raise it when the newest matching note isn't in that first
screen. A stale-looking official notice in search results means "check the
profile", never "nothing newer exists". The one exception: when the sample
already contains the official account's own announcement and its date is
current for what is being asked, answer from it directly — the hop is for
official evidence that is stale, conflicting, or missing from the sample.

If no official account surfaces for such a question, keep the default search
ranking — that relevance ordering is what XHS search is good at; do not
re-search with recency filters just to freshen results. Answer from the dated
evidence you have: state the dates of the notes you relied on, and when the
newest relevant note looks old for what is being asked, say clearly that the
answer may be outdated.

For questions with no authoritative owner (experiences, opinions,
recommendations), user notes are the evidence — no profile hop needed. An
`author_scan` can still help there to distinguish firsthand expertise from
soft ads/content farms when a note looks suspicious or representative.

Shared options (same meaning for both):

- `num_notes=N` — how many notes to collect. `search` defaults to 10;
  `author_scan` defaults to the first unscrolled screen of the profile grid.
  Raise it only when the question needs broader evidence.
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

### `get_notes`

`get_notes(notes=[{note_id, xsec_token}, ...])` directly opens tokenized
full-screen detail pages and returns each note's body + top comments. Batch the
specific notes you need into one call. Only use tokens already collected from
`search` or `author_scan`; a bare note id is insufficient. It supports the same
`num_comments`, `download_media`, `ocr`, and `transcribe_audio` options as the
full scan macros.

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
- In answers and generated files (Excel, CSV, etc.), copy each note's `url`
  verbatim; never rebuild it from `note_id`. It must include `xsec_token=`. Otherwise a bare
  `xiaohongshu.com/explore/<id>` link without xsec_token cannot be opened on desktop web.
- Distinguish note body text, comments, author profile facts, engagement, and
  media observations.
- If a macro returns partial success, use the collected evidence and clearly say
  what was missing.
- If a macro reports login/session/security/route/DOM failures, do not pretend
  the data was collected; explain the blocker and what would be needed next.
- Reply in the same language as the user's task.
