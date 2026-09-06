# Douyin site notes

- Site id: `dy`; home URL: `https://www.douyin.com/`.
- Douyin web may throttle by keeping the page visually blank for 4-5 minutes.
  Commands therefore use long waits by default and report
  `blank_or_throttled` instead of treating a blank page as an immediate hard
  failure.
- Use `page_state` when a search, video, or author call reports an unexpected
  page. It exposes login, blank-page, and route state without copying page
  content into an error message.
- The desktop starts from a blank conversation tab and opens Douyin only after
  the user request selects a Douyin tool. When a tool reports
  `login_required`, tell the user to sign in in that tab and call
  `wait_for_login` once. It polls the same tab for up to ten minutes. After
  `logged_in:true`, retry the original tool and continue. After
  `timed_out:true`, fail the task instead of starting another wait.
- `search` starts from the homepage/top search box, enters the keyword,
  submits with Enter, then extracts cards from the search-result waterfall.
  Use `--num` to scroll for more cards; default is 10.
- Observed stable-ish selectors on 2026-06-11:
  `data-e2e="searchbar-input"`, `data-e2e="searchbar-button"`,
  `.search-result-card`, parent ids like `waterfall_item_<video_id>`,
  `.videoImage`, `.RBpYLmIg` for title text, `.lGzJpEad` for author, and
  `.GiEcbsyC span` for visible like/play count text.
- Search-result "综合" includes non-video modules such as live rooms and topic
  cards. The extractor filters obvious live cards and keeps cards with video
  signals; fields absent from the search card, such as comments/shares, are
  returned as empty strings.
- `get_videos` accepts video ids or canonical URLs returned by `search` or
  `author_scan`, then verifies the requested video page before extracting the
  caption, comments, media, and creator fields.
- `author_scan` accepts an author id or profile URL and returns the profile plus
  video cards. Call `get_videos` for cards that need captions, comments, media,
  or audio transcription.
- `download_media` stores the video and cover under the current run directory.
  `transcribe_audio` also downloads the video and sends audio through the paid
  socai ASR service using the current task id for credits accounting.
- The integration is read-only. It does not like, comment, follow, publish, or
  call private Douyin APIs.
