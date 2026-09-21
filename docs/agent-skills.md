# Agent skills and self-healing

socai exposes focused procedural knowledge to the TUI and desktop agents as
Agent Skills. Skills use progressive disclosure: the system prompt contains a
compact name and description, while the full instruction is returned by `read_skill` or preloaded when the user
selects a skill in the desktop composer.

The catalog contains two bundled skills:

- `self-healing`: diagnose a failed or incomplete action, make the smallest
  safe recovery, verify the original outcome, and retain a reusable learning
  only after recovery succeeds.
- `research`: explore social topics and discover people, projects and adjacent
  leads, maintaining a broad map, selective depth and durable follow-ups.

The bundled instructions live at
`core/src/agent/skills/{self-healing,research}/SKILL.md`. Their frontmatter follows the Agent
Skills `name` and `description` contract. The file is compiled into
`socai-core`, so retained local data cannot replace the canonical safety
instruction.

## Runtime contract

`local_agent_tools()` registers the skill tools for both interactive
entrypoints:

- `read_skill({ "name": "self-healing" })` returns the canonical instruction
  and the newest bounded local learnings. Calling it marks the skill as loaded
  for the current agent run.
- `record_skill_learning(...)` accepts a structured symptom, verified root
  cause, recovery, and validation. It is rejected unless `read_skill` loaded
  `self-healing` in the same run and the runtime independently observed the
  same non-skill tool fail and then succeed in a later step.
- `read_skill({ "name": "research" })` activates research mode for this run.
  It loads only the canonical instruction; there is no retained learning store.
- `research_update(...)` initializes the objective/criteria, upserts complete
  leads by stable ID, and records a completion review. Updates are saved as
  standard tool artifacts and the current record is kept in RunState. The
  request receives a compact checkpoint after transcript compaction; the full
  artifact remains readable. This is model-authored data, not verified truth.
- `research_read({"ids":["lead-id"]})` or `research_read({"offset":0,"limit":3})`
  retrieves full saved entries without browsing again (at most five per page).
- `research_update(exploration=...)` preserves a free-form discovery notebook.
  XHS additionally exposes `explore` for up to three autonomous concept/profile/post
  reading with a free-text answer, and `read_saved_notes` for full local source
  retrieval by note ID. Candidate notes are saved and indexed automatically.

Research policy lives in `agent/research.rs` as `ResearchExtension`, registered
through the generic `agent/extensions.rs` lifecycle interface. The shared loop
only dispatches request-context, before/after-tool, before-finish and execution-
limit hooks; it contains no research-specific conditions or prompts. RunState
holds the optional ledger, while the extension owns review/failure counters.

Research defaults to `focus: explore`: broad discovery with selective depth.
Unknown identity/funding and preview-only sources can remain useful leads;
`promising` means worth following up, not verified or investable. Explicit
fact-checking can use `focus: verify`. The desktop composer has a research toggle,
persisted with the task and inherited by replies. When unselected the model can
still activate the skill. Generic `initial_skills` loads selected instructions
before the first request without an additional model round.

The short `skills/research/CORE.md` is included in every active research request,
including summary/local delivery, so history compaction cannot remove core
operating instructions. `SKILL.md` supplies detailed strategies on demand. The
full ledger remains outside transcript compaction; each request includes all
lead IDs/titles/statuses, up to 12 detail previews and a full-artifact pointer.
`research_read` retrieves complete entries. Updates accept 20 changed leads per
call and 100 per run; omitted leads remain. New turns start a fresh ledger and
must explicitly read prior artifacts to reuse earlier findings.

Submit a coverage review before drafting long reports. Recent source additions
can trigger up to three reconsiderations; novelty is judged by the model, not a
source-count quota. The same acquisition progress is not challenged again merely
because a report was written. New collection/ledger changes invalidate review;
local reads, writes and publishing preserve it. Missing reviews receive at most
two reminders before partial delivery. Verify focus requires important open
questions to be addressed or explicitly deferred. Include a broad linked roster
as well as a shortlist, reconciling saved branch summaries for unrecorded leads.

The default action ceiling is 60; research raises that default to 120 through
the generic execution-ceiling hook. Explicit non-default budgets prevail. It is
room to explore, not a target; a step may call several tools. Tool failures prompt
recovery or parking an optional dependency. Primary-source access failures are
partial outcomes, not search saturation. Ordinary tasks keep their own completion
behavior.

XHS `explore` uses one to three separate tabs, defaults to 15 detailed posts per
branch (adjustable), and returns free-form findings with source IDs. Short-query
and purposeful-filter guidance is shared through `sites/xhs/knowledge.md`, search
schemas and CLI help. Full saved sources remain accessible by ID, and
`ocr_truncated` flags omitted OCR in lean results. Operating instructions live
in the skill rather than being duplicated here.

## Local retention

Verified learnings are stored separately from the bundled instruction:

```text
$SOCAI_HOME/skills/self-healing/learnings.json
```

When `SOCAI_HOME` is unset, the default is
`~/.socai/skills/self-healing/learnings.json`. The JSON store is versioned,
written through a temporary file, protected by process-local and cross-process
locks, deduplicated, and bounded to 24 entries and 64 KiB. Unix uses atomic
rename and Windows uses `MoveFileExW` with replace/write-through flags, without
unlinking the last good store first. `read_skill` shows at most the newest eight
entries to limit context growth.

The write tool has no path argument and only recognizes the bundled
`self-healing` name. It rejects oversized fields, unsupported records, common
credential/session markers, and common prompt-control text. Files edited
outside socai are validated again before their contents are shown to the
agent. Local learnings are explicitly presented as advisory data and must be
revalidated against the current environment.

## Verification

Run the focused contract tests with:

```bash
cargo test -p socai-core agent::skills
```

The tests cover bundled metadata validation, progressive disclosure, TUI and
desktop shared-tool registration, read-before-write and verified-outcome gates,
runtime failure/recovery evidence, round-trip persistence, deduplication,
bounded eviction, external-file tamper handling, and unsafe-content rejection.

### Local delivery after dependency loss

A failed dependency stops acquisition but allows eight bounded local steps.
Tools opt in through `available_in_local_delivery`: read_file, write_file,
read_saved_notes, research_read and desktop publish_artifact. Shell and browser
operations remain unavailable. write_file creates new UTF-8 files under the run
and does not overwrite existing files; revisions use new names. The outcome
remains partial even when attachments are successfully published.

App answers use archived note citations; standalone Markdown keeps original web
URLs. A useful unnamed/preview lead still needs a clickable source entry.
