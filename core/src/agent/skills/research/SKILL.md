---
name: research
description: Explore social-platform topics and discover people, projects and adjacent leads; retain a broad map, selective depth and durable follow-ups.
---

# Research

Use for sourcing, people/project discovery and open-ended landscape research.
Simple facts, supplied-post summaries and single-tool requests usually do not
need this skill. User instructions determine scope.

The runtime supplies short research operating principles on every request,
including after compaction and during final/local delivery. This document is the
strategy/tool reference; reload only when its details help, not every round.

## Plan from the user's goal

Initialize research_update with objective, a few practical criteria and normally
focus=explore. Use focus=verify only for explicit fact-checking/due diligence.
Infer a useful plan from sparse requests and state assumptions briefly. Do not
invent geography, check size, fundraising stage or a hard time cutoff.

For investor sourcing, seek people/teams worth meeting: what they build, for whom,
progress, early feedback and a post/profile entry point. Explore complementary
angles such as technical builders, products/real use, hiring and collaboration.
Recent first-person demos, small-account updates, requests for testers/cofounders
and substantive comments can reveal people beyond mainstream coverage. Follow
useful authors into their history and collaborators. Unknown identity, funding,
location or traction is not an admission gate. Low popularity does not prove a
team is unfunded or unknown to investors. Famous companies are useful benchmarks;
if they dominate, try practitioner vocabulary and more concrete workflows.

Loading this skill raises the ordinary default step ceiling from 60 to 120;
explicit non-default budgets prevail. This is room to explore, not a quota.
Reserve time for delivery and keep a short exploration notebook of productive
paths, new branches and the next worthwhile actions. Park identity questions
that add little; revisit only with a new clue, not merely because a lead is high
priority. An unknown fact is not automatically unfinished discovery.

## Tools and source coverage

- Search with one or two core concepts, an intact name or a short question.
  Expand through adjacent concepts for breadth and observed entities/details for
  depth. Synonym swaps returning the same posts do not add coverage. Avoid
  stacking all criteria into one query or spending most searches on definitions.
- Preview to select unfamiliar useful posts, then read details/comments. Roundups
  and image posts can contain many names. Use read_saved_notes(note_ids=[...])
  and text pagination for saved body/OCR/comments that were truncated. For URLs
  only, section="links" returns a compact ID/title/URL index; avoid shell-parsing
  JSON or rereading all text. Omit note_ids to list saved sources.
- Use explore(branches=[{question: "..."}, ...]) for autonomous investigation of
  concepts, people, profiles or posts. Up to three independent branches use
  separate tabs. Supply the goal, known context, observed IDs and tried queries;
  workers do not inherit your whole conversation. Default 15 detail posts per
  branch is adjustable with max_posts. One branch can deepen a single candidate;
  direct author_scan/get_notes also work. Prefer useful fresh profile posts over
  reopening known material. Avoid overlapping assignments and merge secondary
  findings as well as headline names before choosing the next directions.
- Alternate breadth with selective author/history depth. Keep discovery on the
  requested social platform. External pages/papers/code are optional shortcuts
  when linked, easy and useful, not required verification for every candidate.
  Do not guess many domains or repeatedly repair optional external sources.
- Preserve useful self-reports, tentative connections and secondhand/preview
  leads with brief labels. A curator is distinct from a company they mention;
  several posts about one company are not several deals. Exclude irrelevance
  and duplicates, not useful unnamed builders. Do not invent visits or facts.

## Search filters on XHS

When default results repeat or favor the same popular accounts, try a purposeful
filter pass. If results are sparse, simplify the query or relax filters first;
a narrower time window reduces the eligible set rather than creating matches.

- filters={"sort":"最新"}: recent posts and potentially less-established authors.
- filters={"publish_time":"半年内"} or {"publish_time":"一周内"}: a relevant window.
- filters={"sort":"最多评论"}: discussions; 最多收藏/最多点赞: reference material.
  Engagement sorting is not itself a long-tail filter.
- filters={"search_scope":"未看过"}: account-level unseen posts, not run deduplication.

These are alternatives, not a checklist to exhaust. Change one dimension when
practical, honor explicit user constraints and use actual schema options; there
is no arbitrary date-range or low-like-count filter. Compare IDs, authors and
useful leads, not result count alone. Older posts can expose useful backgrounds.
Return to broader discovery when a restricted pass adds little.

## Durable record and long-tail delivery

research_update upserts complete entries by stable ID; omitted leads remain.
Send only changed leads. Keep exploration a short current plan, not a cumulative
report. Kinds are person/project/topic/query; statuses are pending/investigating/
promising/excluded/deferred. Promising means worth following up, not verified,
investable or fundraising. Store a source URL, useful findings, why a branch was
parked, and next actions/questions where useful. Do not replicate every fact in
rationale, findings, questions and the notebook.

The runtime checkpoint retains the complete compact identity/status index and
priority previews across compaction. Full findings remain in run artifacts.
research_read(ids=[...]) recovers complete details without browsing; omit ids to
page with offset/limit and follow next_offset. Read full_record_artifact for
larger batches. Task records are saved observations, not certification. Follow-up
runs should read relevant prior artifacts; never save task data as self-healing
learnings.

Before delivery reconcile BOTH the ledger and saved branch summaries: useful
secondary names may never have entered the ledger. Include a compact broader
list for small accounts, unnamed teams, prototypes, tester/cofounder requests and
preview-only discoveries. One line with name/description, relevance and a source
can suffice. Do not require another profile visit or identity check for inclusion.
Keep a focused shortlist without letting it erase the broader map. When space
is tight, shorten descriptions or use the full report, rather than dropping leads.

App answers can cite archived posts as [title](note:NOTE_ID). Preview-only sources
and standalone Markdown reports need original https URLs, preserving locator
parameters. Every candidate, including unnamed/pending leads, needs a clickable
discovery entry beside it; a distant source list or contact detail is insufficient.
Use saved notes/ledger to recover missing URLs rather than inventing them.

## Completion review

Before drafting long deliverables, submit research_update(review=...) with actual
coverage, remaining gaps and reason; read its feedback in a separate round.
The runtime may ask whether recent source additions imply worthwhile new paths.
Do useful follow-up before report generation. If additions repeat known substance
or no valuable path remains, explain briefly in exploration/review and proceed;
do not search simply to appease a counter. In verify focus, important open factual
questions must be addressed or explicitly deferred/partial.

Satisfied/saturated requires useful coverage, not resolved identities. Do not
claim saturation with valuable untried branches or merely because names are
unknown. A primary-platform access blocker is blocked/partial, not saturation.
Use self-healing for worthwhile recoverable dependencies; changing keywords does
not repair access. Optional-source failure need not stop other useful discovery.

Once coverage is ready, write and publish the requested report/process files.
Keep one comprehensive roster, expand priority cases, and summarize directions
and gaps. Avoid repeating the roster across the shortlist, a separate source
list and the final chat. The chat may highlight findings and link the full report
unless the user requests the full result inline. Process notes cover actual
paths, decisions and measured timing, not a second candidate report. Use runtime
counts rather than estimates. Routine searches, ledger saves and publishing need
a brief decision, not a new analysis of the whole landscape.

Local read/write/publish tools preserve the coverage review. New acquisition or
ledger changes invalidate it. Local shell reads and report assembly also preserve
it; if shell actually collects external material, update the coverage review to
include those findings. Prefer dedicated file tools for delivery. Do not rewrite
full reports to request another coverage review. If material new findings require
a revision, use a new filename and identify the latest published version; existing
attachments are not overwritten. In local-delivery/summary mode, acquisition and
completion bookkeeping are waived: finish from saved material. Do not contact
people without authorization.
