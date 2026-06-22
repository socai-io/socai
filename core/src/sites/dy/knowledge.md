# Douyin site notes

- Site id: `dy`; home URL: `https://www.douyin.com/`.
- Douyin web may throttle by keeping the page visually blank for 4-5 minutes.
  Commands therefore use long waits by default and report
  `blank_or_throttled` instead of treating a blank page as an immediate hard
  failure.
- Current first-step tool: `page_state`. Use it with `--debug-snapshot` to
  verify whether the homepage/search UI is visible before implementing or
  relying on deeper workflows.
- `search_videos` starts from the homepage/top search box, enters the keyword,
  submits with Enter, then extracts cards from the search-result waterfall.
  Use `--num-videos` to scroll for more cards; default is 30.
- Observed stable-ish selectors on 2026-06-11:
  `data-e2e="searchbar-input"`, `data-e2e="searchbar-button"`,
  `.search-result-card`, parent ids like `waterfall_item_<video_id>`,
  `.videoImage`, `.RBpYLmIg` for title text, `.lGzJpEad` for author, and
  `.GiEcbsyC span` for visible like/play count text.
- Search-result "综合" includes non-video modules such as live rooms and topic
  cards. The extractor filters obvious live cards and keeps cards with video
  signals; fields absent from the search card, such as comments/shares, are
  returned as empty strings.
