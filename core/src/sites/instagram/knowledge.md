# Instagram research workflow

Use Instagram for read-only profile, post, reel, and comment research by default. It may comment only when the user explicitly requests the exact target post and comment content. Do not follow accounts, react, like, reply to comments, or send direct messages.

## Search and gates

- Logged-in keyword route: `https://www.instagram.com/explore/search/keyword/?q=<encoded query>`.
- Instagram can also expose search as a panel with a visible Search/搜索 input. If the keyword route is unavailable, open Instagram, use `setSearchQuery`, wait for results to hydrate, and then call `searchState`.
- Always call `searchState` before trusting `searchResults`. A redirect to `/accounts/login`, `/challenge`, or a rate-limit page is not an empty result. Report the gate and ask the user to finish login or verification in the browser.
- Treat zero candidates as a genuine empty result only when `searchState.ok` and `searchState.empty` are both true. Do not convert an unhydrated surface, query mismatch, or gate into zero results.

## Candidate selection

- Keep each candidate's `kind`, stable `id`, canonical `url`, and `position`; never identify a result only by its screen position.
- Default `search` opens each post or Reel and returns caption, author, engagement, and comments together. `preview=true` returns grid cards only. Keyword-grid cells often have no caption; that text appears only after the post is opened.
- Cite the short post `url` (`https://www.instagram.com/p/<shortcode>/`). `video_url` is the playable file and stays on the post because a later download needs it.
- Use `profileDetail` and `profilePosts` on a selected `/<username>/` profile.
- Use `postDetail` and `comments` on a selected `/p/<shortcode>/`, `/reel/<shortcode>/`, or `/<username>/(p|reel)/<shortcode>/` page.
- For a requested comment budget, call `postDetail`, then `comments`. The host automatically alternates extraction with `scrollComments`, expands collapsed replies, deduplicates, and returns the accumulated set up to `limit` (100 by default). The expansion action is read-only; never click Like, Reply, Follow, or Submit controls.
- Prefer candidates that match the user's topic in the returned title/subtitle or media description. Open a candidate before making claims from it.

## Public content and login overlays

Public profiles and posts can remain readable while Instagram shows a sign-up or login overlay. Trust `profileDetail.ok` or `postDetail.ok` when content is present, while disclosing `pageState.login_gate_present` because additional posts or comments may be hidden. `login_gate_present` alone does not mean the visible public content failed.

## Pagination and stopping

Use `scrollResults` only after `searchState` confirms a valid search surface and more evidence is needed. Re-run `searchResults`, deduplicate by `kind + id` or canonical URL, and stop when the requested coverage is met. Never scroll indefinitely or attempt to bypass a login, challenge, or rate limit.

## Post assets

Post/reel rows from `searchResults` and `profilePosts` are automatically collected across lazy-scroll pages up to `limit`. Those rows, full `postDetail` data, and the final accumulated comments are archived automatically as desktop cards and JSON artifacts. Preserve returned media URLs. Cite a saved post with its exact archive id (`instagram:<shortcode>`) when the host requests `note:` citations.

## Explicit comments

- Use `comment` only for an explicit user-authorized write. Preserve the requested text and target; do not invent additional comments.
- The command uses visible CDP pointer and keyboard events. It never calls a platform write API, never replaces a non-empty draft, and dispatches the Post click at most once.
- If the exact text already exists, the command fails closed instead of creating a duplicate or claiming ownership. Treat `commit_unknown` as unknown and never retry automatically.
