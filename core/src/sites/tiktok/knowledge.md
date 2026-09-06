# TikTok site guidance

- The TikTok integration reads public web pages through the shared Chrome CDP session. It does not call private APIs or perform write actions.
- `search` accepts a query and returns normalized public video cards. Reuse each card's canonical URL or video id with `get_videos` when full details and comments are needed.
- `get_videos` accepts one or more TikTok video ids, canonical URLs, player URLs, or short links. It can collect top comments, download the playable video and cover, and run cover OCR or audio transcription.
- TikTok's public `/player/v1/` route may render through canvas without a `<video>` node. socai recognizes the accessible player region and, only when media is requested, clicks the visible Play control so the page exposes its playable resource. A canonical video page that fails to load falls back to this public player route.
- `author_scan` accepts an `@handle`, plain handle, or profile URL and returns the public profile plus visible video cards. Omit `num` to inspect the first visible screen before requesting deeper scrolling.
- Audio transcription uses the managed socai ASR service and consumes credits.
- TikTok can show consent, age, regional, login, CAPTCHA, or unavailable screens. Tools return typed page-state failures so the agent can stop or ask the user to complete the browser step.
- The integration is read-only. It does not like, comment, follow, publish, or bypass access controls.
- All TikTok operations reuse the current socai-owned page target. They do not create per-video tabs; cancellation, task deletion, daemon stop, and browser disconnect use the shared target lifecycle cleanup.
