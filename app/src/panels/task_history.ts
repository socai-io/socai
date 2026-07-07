//! Sidebar (task history list) + the selected-task detail pane.
//!
//! The detail body uses the "split" layout: timeline ‖ final answer, each
//! filling the pane. When only one panel has content — a running task with no
//! answer yet, or a failed task that produced only an error — it falls back to a
//! single stacked panel. (Narrow windows fold split into a stack via CSS.)
//!
//! The answer lives in one place: once the answer panel has a run's final
//! text, the timeline drops the assistant row that streamed that same text
//! and ends with a compact reference card pointing at the panel instead
//! (FinalAnswerRef in the SocaiV2 handoff).
//!
//! Below the body, a full-width filmstrip shows the notes the agent saw.

import type { AgentTaskEventPayload } from "../main";
import { esc } from "../lib/html";
import { renderNoteAnswer, renderTimelineEmbed, setNoteRegistry } from "./notes";
import { formatSourceCount, formatTaskCount, formatTaskTimestamp, formatTokenUsage, taskStatusLabel, t } from "../lib/i18n";
import type { AgentTaskView } from "./tasks";

export interface SidebarProps {
  tasks: AgentTaskView[];
  selectedTaskId: string | null;
  /** True while the compose view is showing — no row should read as selected. */
  composing: boolean;
}

export interface ReplyComposerProps {
  draft: string;
  submitting: boolean;
  error: string;
  connected: boolean;
}

export function renderSidebar(props: SidebarProps): string {
  return `
    <aside class="sidebar" aria-label="${esc(t("task.historyAria"))}">
      <div class="sidebar-head">
        <button id="sidebar-new" type="button" class="sidebar-new">
          <span class="sidebar-new-glyph" aria-hidden="true">+</span>${esc(t("task.new"))}
        </button>
      </div>
      <div class="sidebar-list-head">
        <p class="t-eyebrow result-label">${esc(t("task.history"))}</p>
        <span class="t-small subtle">${esc(formatTaskCount(props.tasks.length))}</span>
      </div>
      <div class="sidebar-list">
        ${renderTaskRows(props)}
      </div>
    </aside>
  `;
}

// A history row is a non-interactive container holding two sibling controls:
// a real <button> covering the glyph+title+meta (click/Enter/Space opens the
// task) and, for finished tasks, a quiet × button that surfaces on hover/focus
// (running/queued must be cancelled first — cancel lives in the detail head).
// Two siblings, never nested: an interactive control inside a role="button"
// is invalid ARIA and would fold the ×'s label into the row's name. Every
// delete affordance routes through the universal centered confirm dialog —
// nothing is destroyed until confirmed.
function renderTaskRows(props: SidebarProps): string {
  if (props.tasks.length === 0) {
    return `<p class="t-small placeholder task-list-empty">${esc(t("task.noTasks"))}</p>`;
  }
  return [...props.tasks]
    .sort((a, b) => b.created_at - a.created_at)
    .map((task) => {
      const active = !props.composing && task.task_id === props.selectedTaskId ? "task-row-active" : "";
      const running = task.status === "running" || task.status === "queued";
      return `
        <div class="task-row ${active}">
          <button type="button" class="task-row-open" data-task-id="${esc(task.task_id)}">
            <span class="task-row-glyph task-row-glyph-${esc(task.status)}" aria-hidden="true">${taskStatusGlyph(task.status)}</span>
            <span class="task-row-main">
              <span class="task-row-title">${esc(task.task)}</span>
              <span class="task-row-meta">${esc(taskStatusLabel(task.status))} · ${esc(formatTaskTimestamp(task.created_at))}</span>
            </span>
          </button>
          ${running ? "" : `
          <button
            type="button"
            class="task-row-delete"
            data-delete-task="${esc(task.task_id)}"
            aria-label="${esc(t("task.deleteAria"))} · ${esc(task.task)}"
            title="${esc(t("task.deleteAria"))}"
          >×</button>`}
        </div>
      `;
    })
    .join("");
}

export function renderTaskDetail(task: AgentTaskView | undefined, replyProps: ReplyComposerProps): string {
  if (!task) return renderEmptyDetail();

  // Point the note UI at this task's archive; the timeline embeds and answer
  // citations below resolve refs against it, and so does the viewer on click.
  setNoteRegistry(task.notes, task.run_dir);

  const running = task.status === "running" || task.status === "queued";
  const hasTimeline = task.events.length > 0 || running;
  const hasResult = !!task.final_text || !!task.error;
  const bothPanels = hasTimeline && hasResult;

  // A running/queued task can't take a reply yet — it already owns the one
  // agent slot (MAX_CONCURRENT_AGENT_TASKS). Once it lands, the composer lets
  // the thread continue instead of starting a fresh, context-less task. It
  // sits under the timeline (the conversation itself), not under the answer.
  const reply = running ? "" : renderReplyComposer(task, replyProps);

  let body: string;
  if (bothPanels) {
    body = `
      <div class="detail-split">
        <div class="detail-col">${renderTimelinePanel(task)}${reply}</div>
        <div class="detail-col">${renderResultPanel(task)}</div>
      </div>
    `;
  } else {
    body = `
      <div class="detail-body detail-body--stacked">
        ${hasTimeline ? renderTimelinePanel(task) : ""}
        ${reply}
        ${hasResult ? renderResultPanel(task) : ""}
        ${!hasTimeline && !hasResult ? `<p class="t-small placeholder">${esc(t("task.noTimeline"))}</p>` : ""}
      </div>
    `;
  }

  return `${renderDetailHead(task, running)}${body}`;
}

function renderReplyComposer(task: AgentTaskView, props: ReplyComposerProps): string {
  const runDisabled = props.submitting || !props.draft.trim() || !props.connected;
  // Sending a follow-up needs the browser, same as starting a task — but the
  // detail view has no compose-style connect overlay, so without this hint a
  // dropped connection just reads as a mysteriously gray send button.
  const connectHint = props.connected ? "" : `
      <p class="t-small subtle task-reply-hint">
        ${esc(t("task.replyConnectHint"))}
        <button id="reply-chrome-connect" type="button" class="btn-ghost btn-compact">${esc(t("chrome.connectCta"))}</button>
      </p>`;
  return `
    <form id="task-reply-form" class="task-reply-form" data-reply-task="${esc(task.task_id)}">
      <div class="task-reply-row">
        <textarea
          id="task-reply-input"
          class="task-reply-input"
          rows="1"
          placeholder="${esc(t("task.replyPlaceholder"))}"
          ${props.submitting ? "disabled" : ""}
        >${esc(props.draft)}</textarea>
        <button id="task-reply-submit" type="submit" class="btn-primary btn-compact task-reply-send" ${runDisabled ? "disabled" : ""}>
          ${props.submitting ? esc(t("task.replySending")) : esc(t("task.replySend"))}
        </button>
      </div>
      ${connectHint}
      ${props.error ? `<p class="t-small result-error task-reply-error">${esc(props.error)}</p>` : ""}
    </form>
  `;
}

// The detail head's meta line. Exported so the live poll in tasks.ts can
// refresh it in place while a run is active (tokens/steps stream into
// run.json mid-run; duration ticks) without a full re-render.
export function renderTaskMetaItems(task: AgentTaskView): string {
  const running = task.status === "running" || task.status === "queued";
  const time = formatTaskTimestamp(task.started_at ?? task.created_at);
  const duration = formatDuration(task);
  const tokens = task.input_tokens !== null && task.output_tokens !== null
    ? formatTokenUsage(task.input_tokens, task.output_tokens)
    : "";
  const dotClass = running ? "badge-dot-ink badge-dot-pulse" : "badge-dot-hollow";
  return [
    `<span class="task-meta-item task-meta-status"><i class="badge-dot ${dotClass}" aria-hidden="true"></i>${esc(taskStatusLabel(task.status))}</span>`,
    time ? `<span class="task-meta-item">${esc(time)}</span>` : "",
    duration ? `<span class="task-meta-item">${esc(duration)}</span>` : "",
    task.model ? `<span class="task-meta-item t-mono">${esc(task.model)}</span>` : "",
    tokens ? `<span class="task-meta-item">${esc(tokens)}</span>` : "",
  ].join("");
}

function renderDetailHead(task: AgentTaskView, running: boolean): string {
  const items = renderTaskMetaItems(task);
  return `
    <div class="task-detail-head">
      <div class="task-detail-headinfo">
        <h2 class="t-h3 task-detail-title">${esc(task.task)}</h2>
        <div class="task-detail-meta">${items}</div>
      </div>
      <div class="task-detail-actions">
        ${running
          ? `<button type="button" class="btn-ghost btn-compact" data-cancel-task="${esc(task.task_id)}">${esc(t("task.cancel"))}</button>`
          : `<button type="button" class="btn-ghost btn-compact" data-delete-task="${esc(task.task_id)}">${esc(t("task.delete"))}</button>`}
      </div>
    </div>
  `;
}

function renderTimelinePanel(task: AgentTaskView): string {
  const hasEvents = task.events.length > 0;
  const duplicateIndex = finalAnswerEventIndex(task);
  const rows = hasEvents
    ? renderRunGroups(task, duplicateIndex)
    : `<p class="t-small placeholder" data-events-placeholder>${esc(t("task.waitingForEvents"))}</p>`;
  const answerRef = task.final_text ? renderFinalAnswerRef(task) : "";
  return `
    <div class="result-block detail-panel">
      <p class="t-eyebrow result-label detail-panel-label">${esc(t("task.timeline"))}</p>
      <div class="event-stream" data-agent-events="${esc(task.task_id)}">${rows}${answerRef}</div>
    </div>
  `;
}

// A task's conversation can span several runs (replies continue it, each a
// fresh agent run — see socai-core's Conversation). A live run's stream opens
// with "queued"; a replayed run opens directly with "started" — so a new
// group begins on either boundary (but a "started" that follows its own run's
// "queued" stays in that group). Each reply then reads as its own "you asked
// / agent did" block instead of one undifferentiated stream.
function renderRunGroups(task: AgentTaskView, duplicateIndex: number): string {
  const groups: AgentTaskEventPayload[][] = [];
  task.events.forEach((ev, index) => {
    if (index === duplicateIndex) return;
    if (startsNewRunGroup(ev, groups[groups.length - 1])) {
      groups.push([]);
    }
    groups[groups.length - 1].push(ev);
  });
  return groups
    .map((events, index) => renderRunGroup(events, index === groups.length - 1))
    .join("");
}

function startsNewRunGroup(
  ev: AgentTaskEventPayload,
  currentGroup: AgentTaskEventPayload[] | undefined,
): boolean {
  if (!currentGroup) return true;
  if (ev.kind === "queued") return true;
  return ev.kind === "started" && currentGroup.some((e) => e.kind === "started");
}

// The "you" message opening a run group. Shared with the live event appender
// in tasks.ts so a streamed follow-up renders the same block a full render
// rebuilds from the started event.
export function renderRunMessage(userText: string): string {
  return `<div class="run-message">
       <span class="run-message__label">${esc(t("task.you"))}</span>
       <p class="run-message__text">${esc(userText)}</p>
     </div>`;
}

// A run group: the user's message (from the run's started event), the event
// rows, and — for completed earlier runs — that run's answer rendered rich
// (markdown + note citations) instead of an escaped assistant row. The last
// group's answer lives in the answer panel; its duplicate assistant event was
// already dropped upstream and the stream ends with the reference card.
function renderRunGroup(events: AgentTaskEventPayload[], isLast: boolean): string {
  const startedIndex = events.findIndex((ev) => ev.kind === "started");
  const userText = startedIndex >= 0 ? events[startedIndex].task ?? "" : "";
  const message = userText ? renderRunMessage(userText) : "";
  const body = events.filter((_, index) => index !== startedIndex);

  let answer = "";
  if (!isLast && body.some((ev) => ev.kind === "done")) {
    let answerIndex = -1;
    for (let index = body.length - 1; index >= 0; index -= 1) {
      if (body[index].kind === "assistant") {
        answerIndex = index;
        break;
      }
    }
    if (answerIndex >= 0) {
      let text = body[answerIndex].text;
      if (text.endsWith(EVENT_TRUNCATION_SUFFIX)) {
        text = text.slice(0, -EVENT_TRUNCATION_SUFFIX.length);
      }
      answer = `<div class="run-answer result-md note-answer">${renderNoteAnswer(text)}</div>`;
      body.splice(answerIndex, 1);
    }
  }
  return `<div class="run-group">${message}${body.map(renderAgentEvent).join("")}${answer}</div>`;
}

// The shell caps event text at 8k chars and marks the cut with this suffix.
const EVENT_TRUNCATION_SUFFIX = "\n... [truncated]";

// The timeline's copy of the final answer: the last assistant event, but only
// when the answer panel is showing the same text. The panel hydrates from
// report.md — the loop's final text plus an optional artifacts appendix — and
// the event may be truncated, so "same" means the event text prefixes the
// panel text, not equality. A failed run's panel holds an error string that
// matches no assistant text, so commentary before a failure stays readable.
// While a task runs, final_text is unset and the answer streams here in full.
function finalAnswerEventIndex(task: AgentTaskView): number {
  const finalText = task.final_text?.trim();
  if (!finalText) return -1;
  for (let index = task.events.length - 1; index >= 0; index -= 1) {
    const ev = task.events[index];
    if (ev.kind !== "assistant") continue;
    let text = ev.text;
    if (text.endsWith(EVENT_TRUNCATION_SUFFIX)) {
      text = text.slice(0, -EVENT_TRUNCATION_SUFFIX.length);
    }
    text = text.trim();
    return text && finalText.startsWith(text) ? index : -1;
  }
  return -1;
}

// The timeline's terminal element (FinalAnswerRef in the SocaiV2 handoff):
// stands in for the answer the stream no longer repeats — names it, gives a
// light signal (source count), and points at the answer panel. The handoff
// picks hint + arrow from a layout prop; the shipped app folds split→stacked
// in CSS, so both variants render and the fold's breakpoint picks one.
// Clicking rewinds the answer's scroll and flashes the panel; bound in
// tasks.ts via [data-answer-ref].
function renderFinalAnswerRef(task: AgentTaskView): string {
  const sources = task.notes?.length ?? 0;
  const count = sources > 0
    ? `<span class="final-answer-ref__count">${esc(formatSourceCount(sources))}</span>`
    : "";
  return `
    <button type="button" class="final-answer-ref" data-answer-ref aria-label="${esc(t("task.jumpToAnswerAria"))}">
      <span class="final-answer-ref__glyph" aria-hidden="true">✓</span>
      <span class="final-answer-ref__body">
        <span class="final-answer-ref__label">${esc(t("task.finalAnswer"))}</span>
        <span class="final-answer-ref__hint">
          ${count}
          <span class="final-answer-ref__cta final-answer-ref__cta--split">${esc(t("task.answerInPanel"))}</span>
          <span class="final-answer-ref__cta final-answer-ref__cta--stacked">${esc(t("task.answerBelow"))}</span>
        </span>
      </span>
      <span class="final-answer-ref__arrow final-answer-ref__arrow--split" aria-hidden="true">→</span>
      <span class="final-answer-ref__arrow final-answer-ref__arrow--stacked" aria-hidden="true">↓</span>
    </button>
  `;
}

function renderResultPanel(task: AgentTaskView): string {
  if (task.final_text) {
    return `
      <div class="agent-outcome detail-panel">
        <p class="t-eyebrow result-label detail-panel-label">${esc(t("task.finalAnswer"))}</p>
        <div class="result-pre result-md note-answer">${renderNoteAnswer(task.final_text)}</div>
      </div>
    `;
  }
  return `
    <div class="agent-outcome detail-panel">
      <p class="t-eyebrow result-label detail-panel-label">${esc(t("task.errorLabel"))}</p>
      <pre class="result-pre result-error">${esc(task.error ?? "")}</pre>
    </div>
  `;
}

// Universal delete confirmation — centered alertdialog on a dimmed scrim.
// Esc, scrim-click, or keep dismisses; delete commits. Warns that the task
// and ALL its artifacts are removed permanently. Bound in tasks.ts.
export function renderConfirmDeleteDialog(task: AgentTaskView): string {
  return `
    <div class="modal-scrim" data-delete-dismiss>
      <div class="confirm-dialog" role="alertdialog" aria-modal="true" aria-label="${esc(t("task.deleteAria"))}">
        <p class="confirm-dialog-title">${esc(t("task.deleteQuestion"))}</p>
        <p class="t-small confirm-dialog-task">“${esc(task.task)}”</p>
        <p class="t-small subtle">${esc(t("task.deleteWarn"))}</p>
        <div class="confirm-dialog-actions">
          <button id="confirm-delete-keep" type="button" class="btn-ghost btn-compact">${esc(t("task.deleteKeep"))}</button>
          <button id="confirm-delete-commit" type="button" class="btn-primary btn-compact">${esc(t("task.delete"))}</button>
        </div>
      </div>
    </div>
  `;
}

function renderEmptyDetail(): string {
  return `
    <div class="task-empty-detail">
      <p class="t-eyebrow result-label">${esc(t("task.selected"))}</p>
      <p class="t-small placeholder">${esc(t("task.emptyDetail"))}</p>
    </div>
  `;
}

export function renderAgentEvent(ev: AgentTaskEventPayload): string {
  const glyph = eventGlyph(ev.kind);
  const row = `<div class="event event-${ev.kind}"><span class="event-glyph">${glyph}</span><span class="event-text">${esc(ev.text)}</span></div>`;
  // Embed the notes this step surfaced as rich cards beneath the row.
  if (ev.kind === "tool_result") {
    const embed = renderTimelineEmbed(noteRefsFromEvent(ev), "rich");
    if (embed) return `${row}${embed}`;
  }
  return row;
}

// Note refs a tool_result surfaced: the design's `{type:"note", data:{ref}}`
// entities, plus (for the current bulk `search`/`author_scan` tools) the note
// ids nested in the xhs_search / card-grid / note entities. Exported for the
// live strip in tasks.ts, which shows only notes no result row has claimed yet.
export function noteRefsFromEvent(ev: AgentTaskEventPayload): string[] {
  const refs: string[] = [];
  const push = (v: unknown): void => {
    if (typeof v === "string" && v && !refs.includes(v)) refs.push(v);
  };
  const asArray = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
  for (const entity of ev.entities ?? []) {
    const data = (entity?.data ?? {}) as Record<string, unknown>;
    if (entity?.type === "note") {
      push(typeof data.ref === "string" ? data.ref : (data as { note_id?: unknown }).note_id);
      continue;
    }
    push(data.note_id);
    for (const n of asArray(data.notes)) {
      const obj = n as { entity?: { note_id?: unknown }; note_id?: unknown };
      push(obj?.entity?.note_id ?? obj?.note_id);
    }
    for (const c of asArray(data.cards)) push((c as { note_id?: unknown })?.note_id);
    for (const c of asArray(data.note_cards)) push((c as { note_id?: unknown })?.note_id);
  }
  return refs;
}

// Elapsed run time: started→finished, or started→now while still running.
// A terminal task without a finished_at has no meaningful end, so we show no
// duration rather than a figure that keeps ticking up against the wall clock.
function formatDuration(task: AgentTaskView): string | null {
  if (!task.started_at) return null;
  const running = task.status === "running" || task.status === "queued";
  const end = task.finished_at ?? (running ? Date.now() : null);
  if (end === null) return null;
  const seconds = Math.max(0, Math.round((end - task.started_at) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

function eventGlyph(kind: AgentTaskEventPayload["kind"]): string {
  switch (kind) {
    case "queued": return "○";
    case "running": return "●";
    case "started": return "▸";
    case "tab": return "□";
    case "step": return "──";
    case "assistant": return " ";
    case "reasoning": return "·";
    case "tool_call": return "→";
    case "tool_result": return "←";
    case "tool_error": return "✗";
    case "api_error": return "✗";
    case "done": return "✓";
    case "completed": return "✓";
    case "failed": return "✗";
    case "cancelled": return "−";
    case "interrupted": return "!";
  }
}

function taskStatusGlyph(status: AgentTaskView["status"]): string {
  switch (status) {
    case "queued": return "○";
    case "running": return "●";
    case "completed": return "✓";
    case "failed": return "×";
    case "cancelled": return "−";
    case "interrupted": return "!";
  }
}
