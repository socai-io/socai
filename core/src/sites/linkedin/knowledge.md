# LinkedIn research workflow

Use LinkedIn for read-only people, company, relationship, and content research. Do not send invitations, messages, reactions, replies, or comments.

## Search routes

- People: `https://www.linkedin.com/search/results/people/?keywords=<encoded query>`
- Content: `https://www.linkedin.com/search/results/content/?keywords=<encoded query>`
- Companies: `https://www.linkedin.com/search/results/companies/?keywords=<encoded query>`
- All: `https://www.linkedin.com/search/results/all/?keywords=<encoded query>`

Navigate to a route with `navigate_site`, then call `searchState` before trusting `searchResults`. A redirect to `/authwall`, `/uas/login`, `/login`, or a challenge page is not an empty search result. Report the gate and ask the user to finish login or verification in the browser.

## Reading and identity

- Call `pageState` after each navigation.
- Use `profileDetail` only on `/in/<profile-id>` pages.
- For complete work or education history, navigate to `/in/<profile-id>/details/experience/` or `/in/<profile-id>/details/education/`, then call `profileHistory`. Do not assume the profile landing page has hydrated off-screen history.
- Treat `current_role` as authoritative only when the page explicitly marks an experience as current. When it is `null`, `latest_role` is historical/unknown-currentness context and must not be described as the person's current job.
- Keep `experience`, `education`, `connection_degree`, followers/connections, and avatar only when returned by the active page. A search headline is a discovery clue, not verified employment history.
- Use `companyDetail` only on `/company/<company-id>/` or `/showcase/<company-id>/`. Search results can contain similarly named or affiliated pages; select by canonical company URL, not result position.
- `companyPeople.people` reads the explicitly labelled “People you may know” module on `/company/<company-id>/people/`. It is a recommendation list and is not an employee roster. To identify company employees, search people by company name, open candidates, then use `profileHistory` and require an explicitly current experience entry whose organization matches the company.
- Use `relatedPeople.people` only as a graph-expansion clue. Preserve its `source_section`; never describe a recommendation as a follower, connection, colleague, or employee unless an explicit page field proves that relationship.
- Use `postDetail` and `comments` on `/posts/...` or `/feed/update/urn:li:...` pages.
- Keep the returned canonical URL and stable profile/activity id with every note or citation. Never identify a result only by its visible position.
- Preserve returned `image_url`, `avatar_url`, `logo_url`, and `media` HTTPS URLs in research output so the desktop can display linked previews or include them in Markdown/artifacts. Do not invent or substitute missing media.
- Visible guest post pages can contain useful post text and comments even when a sign-in overlay is present. Trust `postDetail.ok`; use `pageState.login_gate_present` to disclose that additional content may be hidden.

## Common research tasks

- Find people: search `people`, retain headline/location/relationship labels, then open selected profiles for `profileDetail`.
- Find people at a company: search `companies`, select the canonical company, search `people` with company plus role/location keywords, then verify employment from a current entry returned by `profileHistory`.
- Build a relationship map: start from one verified profile, call `relatedPeople`, open relevant profiles, and record only explicit connection degree, mutual-connection text, current experience, and source-section edges.
- Understand a person: combine profile details with their selected posts and visible comments, keeping each statement tied to its profile or activity URL.

## Greeting drafts

Build a short draft only from facts returned by `profileDetail`, `postDetail`, or the selected search result. Mention one concrete shared topic, avoid invented familiarity, and label the output as a draft. The user must review and send it themselves because this skill exposes no connect or message action.

## Pagination and stopping

Use `scrollResults` only when the current search is valid and more evidence is needed. Re-run `searchResults`, deduplicate by `id` or canonical `url`, and stop once the requested coverage is met. Do not scroll indefinitely or treat rate limiting, login gates, or incomplete hydration as zero results.
