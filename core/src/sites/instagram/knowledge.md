# Instagram research workflow

Use Instagram for read-only profile, post, reel, and comment research. Do not follow accounts, react, like, reply, comment, or send direct messages.

## Search and gates

- Logged-in keyword route: `https://www.instagram.com/explore/search/keyword/?q=<encoded query>`.
- Instagram can also expose search as a panel with a visible Search/搜索 input. If the keyword route is unavailable, open Instagram, use `setSearchQuery`, wait for results to hydrate, and then call `searchState`.
- Always call `searchState` before trusting `searchResults`. A redirect to `/accounts/login`, `/challenge`, or a rate-limit page is not an empty result. Report the gate and ask the user to finish login or verification in the browser.
- Treat zero candidates as a genuine empty result only when `searchState.ok` and `searchState.empty` are both true. Do not convert an unhydrated surface, query mismatch, or gate into zero results.

## Candidate selection

- Keep each candidate's `kind`, stable `id`, canonical `url`, and `position`; never identify a result only by its screen position.
- Use `profileDetail` and `profilePosts` on a selected `/<username>/` profile.
- Use `postDetail` and `comments` on a selected `/p/<shortcode>/`, `/reel/<shortcode>/`, or `/<username>/(p|reel)/<shortcode>/` page.
- Prefer candidates that match the user's topic in the returned title/subtitle or media description. Open a candidate before making claims from it.

## Public content and login overlays

Public profiles and posts can remain readable while Instagram shows a sign-up or login overlay. Trust `profileDetail.ok` or `postDetail.ok` when content is present, while disclosing `pageState.login_gate_present` because additional posts or comments may be hidden. `login_gate_present` alone does not mean the visible public content failed.

## Pagination and stopping

Use `scrollResults` only after `searchState` confirms a valid search surface and more evidence is needed. Re-run `searchResults`, deduplicate by `kind + id` or canonical URL, and stop when the requested coverage is met. Never scroll indefinitely or attempt to bypass a login, challenge, or rate limit.
