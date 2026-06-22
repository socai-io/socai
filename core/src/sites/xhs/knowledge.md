# Xiaohongshu Site Knowledge

Xiaohongshu / 小红书 / XHS is a Chinese lifestyle social platform at
https://www.xiaohongshu.com (default landing page:
https://www.xiaohongshu.com/explore). Posts are called notes (笔记) and are
usually image carousels or short videos with title, body text, hashtags,
engagement counts, comments, and author/profile context.

## Browser Lock

The browser is locked to Xiaohongshu; every task is an XHS task. Do not
navigate to other websites. Drive the site through the XHS tools below
instead of direct `/explore/<note_id>` navigation, which often triggers
blocked, blank, app-only, or QR-code flows. Reply in the same language as the
user's task and ground your answer in tool output.

## Tools

`search_notes`, `extract_search_cards`, `list_search_tabs`,
`click_search_tab`, `reset_search_filters`, `apply_search_filters`,
`open_note`, `close_note`, `read_note`, `extract_note`, `extract_comments`, `scroll_in_note`,
`collect_carousel_images`, `extract_profile`, `author_scan`, `topic_scan`,
`page_state`.

Prefer `topic_scan` for any "research a topic" task — it bundles search,
sample, and read in one call. `topic_scan` is equivalent to calling
`search_notes` and then `read_note` on each result one by one, but it is
faster and uses far fewer tokens — so default to `topic_scan` and only fall
back to `search_notes` + per-note `read_note` when you have a clear reason to
read specific notes. For one-off
lookups: `page_state` → `search_notes` → `read_note` (or `open_note` +
`extract_note` + `extract_comments`). Close any open note modal before
searching again.

## Anti-Bot Rules

- Prefer `search_notes` from the homepage or any non-XHS tab; it navigates
  to XHS and submits the search like a user.
- Do not navigate directly to `/explore/<note_id>` unless no card-click path is
  available. Open notes from search/profile cards with `read_note`.
- Close note modals with `close_note`, Escape, or the close button. Do not
  reload the page just to close a note.
- If a page shows QR/app-only prompts, captcha, security verification, 404/blank
  direct-detail routes, or "page unavailable" copy, stop retrying that URL and
  return to search/profile card clicks.
- Add screenshots when visual state or extraction confidence matters.

## Page States

- `homepage`: left navigation and top search input. Use `search_notes`.
- `search_results`: query input, tabs `全部` / `图文` / `视频` / `用户`, waterfall
  note cards with cover, title, author, likes, and type.
- `note_detail`: modal or full detail. Left side is carousel/video, right side
  is author, title/body, hashtags, comments, and engagement bar.
- `profile_page`: author avatar/name/XHS ID/bio/IP location/stats and note-card
  grid. Use `author_scan` (opens the profile by id) when you have the author id,
  or `extract_profile` when already on the page; scroll loads more cards. The
  grid renders many cards at once (often dozens), so a small `num_notes` returns
  the first render without scrolling. Profile note cards open the same note
  detail (modal/full) as search cards, so `read_note`/`open_note` work on them.

## Entity Fields

Note fields: `note_id`, `url`, `type`, `title`, `author`, `author_id`,
`author_url`, `content`, `hashtags`, `date`, `location`, `ip_location`,
`likes`, `favorites`, `comments_count`, `shares`, `image_count`, `images`,
`video`, `top_comments`.

Comment fields: `username`, `text`, `likes`, `like_count`, `time`,
`is_author_reply`, `is_pinned`, `reply_count`, `sub_comments`.

Author fields: `display_name`, `xhs_id`, `profile_url`, `bio`, `ip_location`,
`followers`, `following`, `likes_and_collections` (each `*_value` is the parsed
integer, e.g. `5.6万` → `56000`), `note_count`, `note_cards`.

Image fields: `url`, `index`, `is_cover`, optional `ocr_text`,
`vision_description`, `local_path`.

Video fields: `url`, `resolved_url`, `poster_url`, optional `transcript`,
`transcript_summary`, `frame_paths`, `frame_descriptions`, `visual_summary`.

## Workflows

- Topic research: call `topic_scan(query=..., num_notes=N)`. It
  searches, optionally switches tab, optionally applies search-result filters,
  then reads notes top-to-bottom in feed order — opening each (which pages the
  next cards in as it scrolls), reading its body + top comments, writing
  artifacts, closing note modals, and marking already analyzed posts. Default
  `num_notes` is 10.
- Quick breadth scan: use `search_notes` to return search-result cards (accepts
  optional `filters`). By default returns only the first page (~19 cards, no
  scrolling); pass `num_notes` to auto-scroll the feed and collect that many
  cards (titles/likes/covers only — no bodies opened, so it stays fast). This is
  the right tool to batch-harvest card metadata for filtering/analysis without
  reading note bodies. Or use `extract_search_cards` to inspect already-loaded
  cards without opening notes.
- Manual note read: use `read_note(index=N)` or `read_note(note_id=...)`.
  Pass `include_media=true` only when images, OCR, video, or visual evidence
  materially matters. Use `download_media=true` when the user explicitly wants
  local image/video files rather than OCR/vision enrichment.
- Creator analysis: call `author_scan(author_id=..., num_notes=N)`. It opens
  the author's profile, reads the header (bio, xhs id, IP location,
  follower/following/liked-&-collected counts), and collects their note summary
  cards in page order (scrolling for `num_notes`; omit for the first render).
  Pass `read_notes=true` to also open each note and read its body + top
  comments — that makes it the profile-scoped analog of `topic_scan` (latency
  scales with the card count, so keep `num_notes` modest when combining with
  details). Use `extract_profile` instead only when already on the profile page.
  Keep creator inventory/style analysis separate from keyword/topic sampling
  unless the user explicitly asks for both.
- Comment sentiment: a note read already returns top comments; call
  `scroll_in_note` then `extract_comments` when more visible comments are needed.
- Media-heavy tasks: enable `include_media` sparingly. OCR, vision,
  transcription, and frame extraction depend on optional local/cloud
  capabilities and can be slow. `download_media=true` only downloads files into
  the run dir and avoids those expensive enrichment steps.

## Evidence Rules

- Preserve real XHS post links from cards when available, including
  `xsec_token` query parameters. Fall back to bare `/explore/<note_id>` only
  when no real tokenized URL exists.
- Cards carry `already_analyzed` / `history_include_media` flags when a prior
  run already read them. `read_note` and `topic_scan` short-circuit notes
  already covered at the same media setting — the returned payload has
  `skipped: true` plus the prior `history` entry. To deepen prior analysis, set
  `include_media: true`. Media downloads are not cache-skipped because fresh
  local files are expected in the current run dir.
- Keep DOM text, comment evidence, image OCR/vision, and video transcript/frame
  evidence labeled separately in final answers.
- If a read returns a stale-note warning or note-id mismatch, close the current
  modal and reopen the intended card before trusting the result.
- Post-level engagement extraction must exclude comment-area DOM; otherwise
  comment likes or comment UI text can be misread as the note's own engagement.
- For reports, include screenshots and artifact paths only when they support the
  conclusion or make verification easier.

## Chinese UI Hints

- Search tabs: `全部`, `图文`, `视频`, `用户`.
- No-result copy often contains: `没有找到相关内容`, `换个词试试`, `暂无相关内容`.
- Engagement labels: `赞`, `收藏`, `评论`, `分享`.
- Count suffixes: `万` and `w` mean 10000; `k` means 1000.
