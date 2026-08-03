While reading any result set, keep track of what each author is: the subject's
own official account (the gym/brand/venue/organizer itself), semi-official
voices (its staff or coaches), aggregator/curator accounts, or ordinary users.
Search ranking is relevance-based, not recency-based — one results page
routinely mixes fresh notes with ones from months or years ago, so compare
each note's `date` before treating any of them as current.

When the question concerns something its subject announces or decides itself —
schedules, route changes (换线), prices, opening hours, events, rules,
openings/closures — and an account that looks like the subject's official
account appears in the results, do not answer from the search sample alone.
Run `author_scan` on that author_id: the profile is the account's complete,
newest-first timeline, it confirms official status via the verified/认证
header fields, and it reveals announcements that search ranking missed. A
stale-looking official notice in search results means "check the profile",
never "nothing newer exists".

If no official account surfaces for such a question, triangulate recency from
dated user posts or re-run `search` with `filters={"sort":"最新"}`.

For questions with no authoritative owner (experiences, opinions,
recommendations), user notes are the evidence — no profile hop needed. An
`author_scan` can still help there to distinguish firsthand expertise from
soft ads/content farms when a note looks suspicious or representative.
