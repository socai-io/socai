//! Inline conversation view for a task (SocaiV2 "Socai Today" handoff).
//!
//! Replaces the old timeline ‖ final-answer split (and the separate compose
//! pane): a task reads as one continuous thread —
//!   · the user's prompt as a right-aligned bubble (one turn per run)
//!   · the agent's work as a quiet, collapsible activity log that auto-expands
//!     while the run streams and folds once the answer lands
//!   · the notes each search surfaced as always-visible card groups (they are
//!     the conversation's artifacts, never tucked inside the fold)
//!   · the answer inline as rich markdown with note citations — no separate
//!     panel, no "jump to answer" bridge
//! The same chat composer serves both faces: pinned under the thread in
//! "reply" mode (disabled while that conversation runs), and centered with the
//! hero in the new-task compose pane
//! (renderComposePane), which the connect overlay masks until chrome is up.
//!
//! Rendering only; state and bindings live in tasks.ts.

import type { AgentArtifact, AgentTaskEventPayload, AgentTaskSnapshot, Status } from "../main";
import { esc } from "../lib/html";
import {
  formatStepCount,
  formatTaskApiError,
  formatTaskCommandErrorPresentation,
  formatTaskInterruptionMessage,
  formatTaskTimestamp,
  formatTokenUsage,
  isTaskApiError,
  t,
  taskStatusLabel,
} from "../lib/i18n";
import { sendShortcutLabel } from "../lib/shortcuts";
import type { ComposerVoiceState } from "../lib/voice-input";
import feishuLogo from "../assets/connectors/feishu.png";
import chromeRemoteDebuggingImage from "../assets/chrome-remote-debugging.png";
import chromeAllowDialogImage from "../assets/chrome-allow-dialog.png";
import { renderNoteAnswer, renderNoteCards } from "./notes";
import { artifactFileIcon, downloadIcon, eyeIcon, formatArtifactSize } from "./artifact_preview";
import type { AgentTaskView } from "./tasks";

export interface ComposerProps {
  mode: "new" | "reply";
  value: string;
  submitting: boolean;
  error: string;
  status: Status;
  /** New-task gate: the selected model has a key. Replies always pass. */
  modelReady: boolean;
  /** True while the shown task runs — the composer waits for the slot. */
  running: boolean;
  /**
   * Configured browser source is the remote hosted browser. Disconnected is
   * routine there (hosted sessions expire between runs) and submitting a run
   * reconnects on demand, so the connect overlay and send gating don't apply.
   */
  remoteProfile: boolean;
  /** Existing Chrome has exposed its CDP endpoint after the user opted in. */
  remoteDebuggingReady: boolean;
  /** Cloud microphone availability and the current recording phase. */
  voice: ComposerVoiceState;
}

export interface ConversationProps {
  task: AgentTaskView;
  running: boolean;
  /** Resolves a turn's activity fold state (tasks.ts owns the toggles). */
  isActivityOpen: (turnIndex: number, defaultOpen: boolean) => boolean;
  /** Download/open progress survives the full-shell rerenders owned by tasks.ts. */
  artifactDownloadState: (path: string) => ArtifactDownloadState | undefined;
  artifactPreviewPath: string | null;
  composer: ComposerProps;
}

export interface ArtifactDownloadState {
  status: "downloading" | "downloaded" | "opening" | "download_failed" | "open_failed";
  destination?: string;
  identity?: string;
}

interface TurnMetrics {
  steps: number | null;
  durationMs: number | null;
  inputTokens: number | null;
  outputTokens: number | null;
  cachedInputTokens: number | null;
  cacheCreationInputTokens: number | null;
  estimatedCost: number | null;
  costCurrency: string | null;
  pointsUsed: number | null;
}

export function renderConversation(props: ConversationProps): string {
  const { task, running } = props;
  return `
    <section class="conversation" aria-label="${esc(t("task.viewAria"))}">
      ${renderHead(task, running)}
      <div class="thread" data-agent-events="${esc(task.task_id)}">
        <div class="thread-inner">
          ${renderThread(
            task,
            running,
            props.isActivityOpen,
            props.artifactDownloadState,
            props.artifactPreviewPath,
          )}
        </div>
      </div>
      <div class="composer-dock">
        ${renderComposer(props.composer)}
      </div>
    </section>
  `;
}

// ── new-task compose (centered) ──────────────────────────────────────
// A centered hero + the same chat composer. When chrome isn't connected the
// form is masked behind the connect overlay.
export function renderComposePane(composer: ComposerProps): string {
  const gated = !composer.remoteProfile && composer.status.state !== "connected";
  return `
    <div class="compose-pane">
      <div class="new-task-compose">
        <div class="new-task-copy">
          <h2 class="t-h2">${esc(t("task.hero"))}</h2>
          <p class="t-small subtle">${esc(t("task.lede"))}</p>
        </div>
        <div class="compose-form-stack ${gated ? "is-masked" : ""}">
          <div class="compose-form-inner" aria-hidden="${gated ? "true" : "false"}">
            ${renderComposer(composer)}
          </div>
          ${gated ? renderConnectOverlay(composer.status, composer.remoteDebuggingReady) : ""}
        </div>
      </div>
    </div>
  `;
}

function renderConnectOverlay(status: Status, remoteDebuggingReady: boolean): string {
  const connecting = status.state === "connecting";
  const ready = remoteDebuggingReady;
  return `
    <div class="connect-overlay" role="dialog" aria-label="${esc(t("chrome.requiredAria"))}">
      <h3 class="connect-overlay-head">${esc(t("chrome.setupTitle"))}</h3>
      <div class="connect-overlay-steps">
        <section class="connect-setup-step ${ready ? "is-complete" : "is-active"}">
          <div class="connect-setup-step-head">
            <span class="connect-setup-index">${ready ? "✓" : "1"}</span>
            <h4 class="connect-setup-title">${esc(t("chrome.setupEnableTitle"))}</h4>
            <span class="connect-setup-state">${esc(t(ready ? "chrome.setupDone" : "chrome.setupWaiting"))}</span>
          </div>
          <img
            class="connect-setup-image"
            src="${chromeRemoteDebuggingImage}"
            alt="${esc(t("chrome.setupEnableImageAlt"))}"
          />
          <button
            id="overlay-remote-debugging-help"
            type="button"
            class="btn-primary connect-setup-action"
            ${ready ? "disabled" : ""}
          >${esc(t("chrome.setupOpenSettings"))}</button>
        </section>
        <section class="connect-setup-step ${ready ? "is-active" : ""}">
          <div class="connect-setup-step-head">
            <span class="connect-setup-index">2</span>
            <h4 class="connect-setup-title">${esc(t("chrome.setupAllowTitle"))}</h4>
            <span class="connect-setup-state">${esc(t(connecting ? "chrome.connecting" : "chrome.setupWaiting"))}</span>
          </div>
          <img
            class="connect-setup-image"
            src="${chromeAllowDialogImage}"
            alt="${esc(t("chrome.setupAllowImageAlt"))}"
          />
          <button
            id="overlay-chrome-connect"
            type="button"
            class="btn-primary connect-setup-action"
            ${!ready || connecting ? "disabled" : ""}
          >${esc(t(connecting ? "chrome.connectingCta" : "chrome.setupConnect"))}</button>
        </section>
      </div>
    </div>
  `;
}

// ── slim head: persistent subject + status + row-level actions ──────
function renderHead(task: AgentTaskView, running: boolean): string {
  const dotClass = running ? "badge-dot-ink badge-dot-pulse" : "badge-dot-hollow";
  const actions = running
    ? `<button type="button" class="btn-ghost btn-compact" data-cancel-task="${esc(task.task_id)}">${esc(t("task.cancel"))}</button>`
    : `${task.status === "interrupted" || task.status === "cancelled"
      ? `<button type="button" class="btn-ghost btn-compact" data-resume-task="${esc(task.task_id)}">${esc(t("task.resume"))}</button>`
      : ""}<button type="button" class="btn-ghost btn-compact" data-delete-task="${esc(task.task_id)}">${esc(t("task.delete"))}</button>`;
  return `
    <div class="conversation-head">
      <span class="conversation-head__title" title="${esc(task.task)}">${esc(task.task)}</span>
      <div class="conversation-head__actions">
        <span class="conv-status">
          <i class="badge-dot ${dotClass}" aria-hidden="true"></i>
          ${esc(taskStatusLabel(task.status))}
        </span>
        ${actions}
      </div>
    </div>
  `;
}

// ── run groups → turns ───────────────────────────────────────────────
// A task's conversation can span several runs (replies continue it, each a
// fresh agent run). A live run's stream opens with "queued"; a replayed run
// opens directly with "started" — so a new turn begins on either boundary
// (but a "started" that follows its own run's "queued" stays in that turn).
function renderThread(
  task: AgentTaskView,
  running: boolean,
  isActivityOpen: ConversationProps["isActivityOpen"],
  artifactDownloadState: ConversationProps["artifactDownloadState"],
  artifactPreviewPath: ConversationProps["artifactPreviewPath"],
): string {
  const duplicateIndex = finalAnswerEventIndex(task);
  const groups = groupRunEvents(task, duplicateIndex);
  if (groups.length === 0) groups.push([]); // no events (e.g. an early failure) → still show prompt + error
  return groups
    .map((events, index) => renderTurn(
      task,
      events,
      index,
      index === groups.length - 1,
      running,
      isActivityOpen,
      artifactDownloadState,
      artifactPreviewPath,
    ))
    .join("");
}

// One turn is one run's events. Turn 0 has no "started" event (the task's
// original prompt lives on task.task); replies carry their prompt on "started".
// An earlier (superseded) run keeps its answer inline: the trailing assistant
// event is lifted out of the body and rendered as that turn's prose answer.
// The latest run's answer comes from task.final_text instead. Message
// timestamps ride along: the prompt's from task creation / the started event,
// the answer's from the lifted assistant event (the latest run's answer uses
// task.finished_at instead).
function buildTurn(
  events: AgentTaskEventPayload[],
  isFirst: boolean,
  isLast: boolean,
  task: AgentTaskView,
): { userText: string; userAt: number | null; body: AgentTaskEventPayload[]; answerText: string | null; answerAt: number | null } {
  const startedIndex = events.findIndex((ev) => ev.kind === "started");
  const started = startedIndex >= 0 ? events[startedIndex] : null;
  const userText = started ? started.task ?? "" : isFirst ? task.task : "";
  const userAt = started ? started.created_at : isFirst ? task.created_at : null;
  let body = events.filter((_, index) => index !== startedIndex);

  let answerText: string | null = null;
  let answerAt: number | null = null;
  if (!isLast && body.some((ev) => ev.kind === "done")) {
    for (let index = body.length - 1; index >= 0; index -= 1) {
      if (body[index].kind === "assistant") {
        answerText = stripTruncation(body[index].text);
        answerAt = body[index].created_at;
        body = body.filter((_, j) => j !== index);
        break;
      }
    }
  }
  const apiErrors = new Set(
    body.filter((event) => event.kind === "api_error").map((event) => event.text.trim()),
  );
  body = body.filter((event) => event.kind !== "failed" || !apiErrors.has(event.text.trim()));
  return { userText, userAt, body, answerText, answerAt };
}

function renderTurn(
  task: AgentTaskView,
  events: AgentTaskEventPayload[],
  index: number,
  isLast: boolean,
  running: boolean,
  isActivityOpen: ConversationProps["isActivityOpen"],
  artifactDownloadState: ConversationProps["artifactDownloadState"],
  artifactPreviewPath: ConversationProps["artifactPreviewPath"],
): string {
  const { userText, userAt, body, answerText, answerAt } = buildTurn(events, index === 0, isLast, task);
  const showWorking = isLast && running;
  const hosted = task.provider === "socai";
  const metrics = turnMetrics(task, events, isLast);

  // Every completed turn owns its own model, duration, usage and cost. Never
  // read an earlier answer's figures from the task's latest-run snapshot.
  let answer = "";
  let exportText: string | null = null;
  const metaBits: string[] = [];
  if (isLast) {
    const finishedAt = task.finished_at ? formatTaskTimestamp(task.finished_at) : null;
    const apiError = taskApiError(task);
    const commandError = task.error ? formatTaskCommandErrorPresentation(task.error) : null;
    if (commandError) {
      answer = renderTaskCommandErrorCard(commandError);
      if (finishedAt) metaBits.push(finishedAt);
    } else if (apiError) {
      answer = renderTaskApiErrorCard(apiError);
      if (finishedAt) metaBits.push(finishedAt);
    } else if (task.final_text) {
      exportText = task.final_text;
      answer = `<div class="conv-answer result-md note-answer">${renderNoteAnswer(task.final_text)}</div>`;
      if (finishedAt) metaBits.push(finishedAt);
      if (!hosted && task.model) metaBits.push(task.model);
      if (metrics.durationMs !== null) metaBits.push(formatDurationMs(metrics.durationMs));
      if (metrics.inputTokens !== null && metrics.outputTokens !== null) {
        metaBits.push(formatTokenUsage(
          metrics.inputTokens,
          metrics.outputTokens,
          metrics.cachedInputTokens,
          metrics.cacheCreationInputTokens ?? 0,
          hosted ? null : metrics.estimatedCost,
          hosted ? null : metrics.costCurrency,
        ));
      }
      if (metrics.pointsUsed !== null && (hosted || metrics.pointsUsed > 0)) {
        metaBits.push(t("billing.pointsUsed", { points: metrics.pointsUsed }));
      }
    } else if (task.error) {
      const error = task.status === "interrupted"
        ? formatTaskInterruptionMessage(task.error)
        : task.error;
      answer = `<pre class="conv-error">${esc(error)}</pre>`;
      if (finishedAt) metaBits.push(finishedAt);
    }
  } else if (answerText != null) {
    exportText = answerText;
    answer = `<div class="conv-answer result-md note-answer">${renderNoteAnswer(answerText)}</div>`;
    if (answerAt) metaBits.push(formatTaskTimestamp(answerAt));
    const started = events.find((ev) => ev.kind === "started");
    if (!hosted && started?.model) metaBits.push(started.model);
    if (metrics.durationMs !== null) metaBits.push(formatDurationMs(metrics.durationMs));
    if (metrics.inputTokens !== null && metrics.outputTokens !== null) {
      metaBits.push(formatTokenUsage(
        metrics.inputTokens,
        metrics.outputTokens,
        metrics.cachedInputTokens,
        metrics.cacheCreationInputTokens ?? 0,
        hosted ? null : metrics.estimatedCost,
        hosted ? null : metrics.costCurrency,
      ));
    }
    if (metrics.pointsUsed !== null && (hosted || metrics.pointsUsed > 0)) {
      metaBits.push(t("billing.pointsUsed", { points: metrics.pointsUsed }));
    }
  }
  const meta = renderConvMeta(metaBits);
  const artifactCards = renderArtifactCards(
    task.task_id,
    task.artifacts ?? [],
    index,
    artifactDownloadState,
    artifactPreviewPath,
  );
  const exportAction = exportText
    ? `<div class="conv-answer-actions">
        <button type="button" class="btn-ghost btn-compact feishu-export-action" data-feishu-export="${esc(task.task_id)}" data-feishu-turn="${index}">
          ${feishuIcon()}<span>${esc(t("feishu.export"))}</span>
        </button>
      </div>`
    : "";

  const userLabel = userAt ? `${t("task.you")} · ${formatTaskTimestamp(userAt)}` : t("task.you");
  const user = userText
    ? `<div class="conv-user">
        <span class="conv-user__label">${esc(userLabel)}</span>
        <p class="conv-user__bubble">${esc(userText)}</p>
      </div>`
    : "";

  return `
    <div class="turn">
      ${user}
      ${renderActivity(task, body, index, showWorking, metrics, isActivityOpen)}
      ${answer}
      ${artifactCards}
      ${exportAction}
      ${meta}
    </div>
  `;
}

function renderArtifactCards(
  taskId: string,
  artifacts: AgentArtifact[],
  turnIndex: number,
  downloadState: ConversationProps["artifactDownloadState"],
  previewPath: string | null,
): string {
  const cards = artifacts
    .filter((artifact) => artifact.turn_index === turnIndex)
    .map((artifact) => {
      const state = downloadState(artifact.path);
      const statusKey = state?.status === "downloading"
        ? "artifact.downloading"
        : state?.status === "downloaded"
          ? "artifact.open"
          : state?.status === "opening"
            ? "artifact.opening"
            : state?.status === "open_failed"
              ? "artifact.openFailed"
              : state?.status === "download_failed"
                ? "artifact.downloadFailed"
                : "artifact.download";
      const openingState = state?.status === "downloaded"
        || state?.status === "opening"
        || state?.status === "open_failed";
      const stateClass = openingState
        ? " is-downloaded"
        : state?.status === "download_failed"
          ? " is-error"
          : "";
      const ariaLabel = state?.status === "downloading"
        ? t("artifact.downloadingAria", { name: artifact.name })
        : state?.status === "download_failed"
          ? t("artifact.downloadFailedAria", { name: artifact.name })
          : state?.status === "opening"
            ? t("artifact.openingAria", { name: artifact.name })
            : state?.status === "open_failed"
              ? t("artifact.openFailedAria", { name: artifact.name })
              : openingState
                ? t("artifact.openAria", { name: artifact.name })
                : t("artifact.downloadAria", { name: artifact.name });
      const previewable = !!artifact.preview_kind;
      const main = previewable
        ? `<button
            type="button"
            class="artifact-card__main"
            data-artifact-preview="${esc(taskId)}"
            data-artifact-path="${esc(artifact.path)}"
            aria-label="${esc(t("artifact.previewAria", { name: artifact.name }))}"
            aria-pressed="${previewPath === artifact.path ? "true" : "false"}"
          >
            <span class="artifact-card__icon" aria-hidden="true">${artifactFileIcon(artifact.name)}</span>
            <span class="artifact-card__copy">
              <span class="artifact-card__name">${esc(artifact.name)}</span>
              <span class="artifact-card__meta">${esc(artifact.kind)} · ${esc(formatArtifactSize(artifact.size_bytes))}</span>
            </span>
            <span class="artifact-card__eye" aria-hidden="true">${eyeIcon()}</span>
          </button>`
        : `<div class="artifact-card__main artifact-card__main--static">
            <span class="artifact-card__icon" aria-hidden="true">${artifactFileIcon(artifact.name)}</span>
            <span class="artifact-card__copy">
              <span class="artifact-card__name">${esc(artifact.name)}</span>
              <span class="artifact-card__meta">${esc(artifact.kind)} · ${esc(formatArtifactSize(artifact.size_bytes))}</span>
            </span>
          </div>`;
      return `
        <div class="artifact-card${stateClass}${state?.status === "open_failed" ? " is-error" : ""}${previewPath === artifact.path ? " is-previewing" : ""}" title="${esc(state?.destination ?? artifact.relative_path)}">
          ${main}
          <button
            type="button"
            class="artifact-card__action"
            data-artifact-action="${esc(taskId)}"
            data-artifact-path="${esc(artifact.path)}"
            title="${esc(t(statusKey))}"
            aria-label="${esc(ariaLabel)}"
            ${state?.status === "downloading" || state?.status === "opening" ? 'aria-disabled="true" aria-busy="true"' : ""}
          >${downloadIcon()}</button>
          <span class="sr-only" role="status" aria-live="polite">${esc(t(statusKey))}</span>
        </div>
      `;
    })
    .join("");
  if (!cards) return "";
  return `<div class="artifact-cards" aria-label="${esc(t("artifact.listAria"))}">${cards}</div>`;
}

/** Exact answer represented by an answer-level export button. */
export function answerTextForTurn(task: AgentTaskView, turnIndex: number): string | null {
  const groups = groupRunEvents(task, finalAnswerEventIndex(task));
  const events = groups[turnIndex];
  if (!events) return null;
  const isLast = turnIndex === groups.length - 1;
  if (isLast) return task.final_text ?? null;
  return buildTurn(events, turnIndex === 0, false, task).answerText;
}

function feishuIcon(): string {
  return `<img class="feishu-export-action__icon" src="${esc(feishuLogo)}" alt="">`;
}

// ── agent activity — auto-compacted by default ───────────────────────
// The low-level run detail (tool calls/steps/reasoning) folds into one quiet
// summary line; click to expand. A live run auto-expands so you watch it work,
// then auto-folds when the answer lands (tasks.ts clears the toggle state on
// the terminal transition). The notes it surfaces are NOT hidden here — they
// render in the always-visible per-search card groups below the fold.
function renderActivity(
  task: AgentTaskView,
  body: AgentTaskEventPayload[],
  turnIndex: number,
  showWorking: boolean,
  metrics: TurnMetrics,
  isActivityOpen: ConversationProps["isActivityOpen"],
): string {
  if (body.length === 0 && !showWorking) return "";
  const notesHtml = body.map(renderSearchGroupForEvent).join("");
  const open = isActivityOpen(turnIndex, showWorking);

  // compact summary signal: steps · duration (sources live in the notes strip)
  const stepCount =
    metrics.steps
      ? metrics.steps
      : body.filter((ev) => ev.kind === "step").length || body.filter((ev) => ev.kind === "tool_call").length;
  // While a run is live this is the only place its accumulating figures can
  // appear. Once the answer lands, the answer meta owns the final figures so
  // the same duration/usage/points are not repeated twice in one turn.
  const bits = showWorking ? activityMetricBits(task, metrics, stepCount) : [];
  const meta =
    bits.length > 0
      ? `<span class="activity-toggle__meta" data-turn-metrics>· ${esc(bits.join(" · "))}</span>`
      : `<span class="activity-toggle__meta" data-turn-metrics></span>`;
  const dot = showWorking
    ? `<span class="activity-toggle__dot"><i class="badge-dot badge-dot-ink badge-dot-pulse" aria-hidden="true"></i></span>`
    : "";
  const workingRow = showWorking
    ? `<div class="act-row act-row--working"><span class="act-row__glyph"><i class="badge-dot badge-dot-ink badge-dot-pulse" aria-hidden="true"></i></span><span class="act-row__text">${esc(t("task.working"))}</span></div>`
    : "";

  return `
    <div class="activity-wrap">
      <button
        type="button"
        class="activity-toggle ${open ? "is-open" : ""}"
        data-activity-turn="${turnIndex}"
        aria-expanded="${open ? "true" : "false"}"
      >
        <span class="activity-toggle__chev" aria-hidden="true">${open ? "▾" : "▸"}</span>
        ${dot}
        <span class="activity-toggle__label">${esc(showWorking ? t("task.working") : t("task.activityLabel"))}</span>
        ${meta}
      </button>
      ${open ? `<div class="activity activity--transcript">${body.map(renderEventRow).join("")}${workingRow}</div>` : ""}
      ${notesHtml ? `<div class="activity-notes">${notesHtml}</div>` : ""}
    </div>
  `;
}

/** One activity row. Shared with the live event appender in tasks.ts so a
 *  streamed row lands as the same markup a full render rebuilds. */
export function renderEventRow(ev: AgentTaskEventPayload): string {
  const progressKey = ev.kind === "tool_progress" ? `${ev.id ?? ""}:${ev.phase ?? ""}` : "";
  const progressAttr = progressKey ? ` data-tool-progress="${esc(progressKey)}"` : "";
  if (ev.kind === "api_error") return renderTaskApiErrorEvent(ev);
  const text = ev.kind === "tool_progress"
    ? toolProgressText(ev)
    : formatTaskInterruptionMessage(ev.text);
  return `<div class="act-row act-row--${esc(ev.kind)}"${progressAttr}><span class="act-row__glyph" aria-hidden="true">${eventGlyph(ev.kind)}</span><span class="act-row__text">${esc(text)}</span></div>`;
}

function renderTaskApiErrorEvent(ev: AgentTaskEventPayload): string {
  const presentation = formatTaskApiError(ev.text);
  return `<div class="act-row act-row--api_error">
    <span class="act-row__glyph" aria-hidden="true">${eventGlyph(ev.kind)}</span>
    <span class="act-row__text task-api-error-copy">
      <span class="task-api-error-copy__title">${esc(presentation.title)}</span>
      <span class="task-api-error-copy__message">${esc(presentation.message)}</span>
      <span class="task-api-error-copy__meta">${esc(presentation.meta)}</span>
    </span>
  </div>`;
}

function renderTaskApiErrorCard(error: string): string {
  const presentation = formatTaskApiError(error);
  return `<div class="task-api-error-card" role="alert">
    <div class="task-api-error-card__heading">
      <i class="runtime-error-notice__dot" aria-hidden="true"></i>
      <p class="task-api-error-card__title">${esc(presentation.title)}</p>
    </div>
    <p class="task-api-error-card__message">${esc(presentation.message)}</p>
    <p class="task-api-error-card__meta">${esc(presentation.meta)}</p>
  </div>`;
}

function renderTaskCommandErrorCard(
  presentation: NonNullable<ReturnType<typeof formatTaskCommandErrorPresentation>>,
): string {
  return `<div class="task-api-error-card" role="alert">
    <div class="task-api-error-card__heading">
      <i class="runtime-error-notice__dot" aria-hidden="true"></i>
      <p class="task-api-error-card__title">${esc(presentation.title)}</p>
    </div>
    <p class="task-api-error-card__message">${esc(presentation.message)}</p>
    <p class="task-api-error-card__meta">${esc(presentation.meta)}</p>
  </div>`;
}

function taskApiError(task: AgentTaskView): string | null {
  const error = task.error?.trim();
  if (error && isTaskApiError(error)) return error;
  const finalText = task.final_text?.trim();
  if (!finalText || !/^API error:\s*/i.test(finalText)) return null;
  return finalText.replace(/^API error:\s*/i, "");
}

function toolProgressText(ev: AgentTaskEventPayload): string {
  const phase = ev.phase === "ocr" ? t("task.progressOcr") : t("task.progressReading");
  const current = ev.item_index ?? ev.current ?? 0;
  const total = ev.total ?? 0;
  const count = total > 0 ? ` ${current}/${total}` : "";
  const title = ev.title?.trim() ? ` · ${ev.title.trim()}` : "";
  return `${phase}${count}${title}`;
}

// ── per-search note previews (always-visible card groups) ────────────
/** The notes an event surfaced, as a labeled card group ("search · <query>").
 *  Empty when the event carries no resolvable notes. */
export function renderSearchGroupForEvent(ev: AgentTaskEventPayload): string {
  const refs = noteRefsFromEvent(ev);
  if (refs.length === 0) return "";
  const cards = renderNoteCards(refs, "rich");
  if (!cards) return "";
  const isSearch = ev.name === "search";
  const args = ev.args as { query?: unknown } | null | undefined;
  const query = isSearch && args && typeof args.query === "string" ? args.query : "";
  return renderSearchGroup(t(isSearch ? "task.searchLabel" : "task.notesLabel"), query, cards, false);
}

/** The live strip: notes already on disk that no result row has claimed yet
 *  (tasks.ts polls the archive mid-run and pins this to the last turn). While
 *  a search is still in flight, its query labels the strip — the same header
 *  the finished result's group will carry. */
export function renderLiveNotesGroup(refs: string[], searchQuery: string | null): string {
  const cards = renderNoteCards(refs, "rich");
  if (!cards) return "";
  if (searchQuery !== null) return renderSearchGroup(t("task.searchLabel"), searchQuery, cards, true);
  return renderSearchGroup(t("task.notesLabel"), "", cards, true);
}

/** The query of the search still in flight: the newest search tool_call with
 *  no tool_result/tool_error answering its id. Null when no search is pending
 *  ("" when one is pending but its args carry no readable query). */
export function pendingSearchQuery(task: AgentTaskView): string | null {
  const answered = new Set<string>();
  for (const ev of task.events) {
    if ((ev.kind === "tool_result" || ev.kind === "tool_error") && ev.id) answered.add(ev.id);
  }
  for (let index = task.events.length - 1; index >= 0; index -= 1) {
    const ev = task.events[index];
    if (ev.kind !== "tool_call" || ev.name !== "search") continue;
    if (ev.id && answered.has(ev.id)) return null; // newest search already landed
    const args = ev.args as { query?: unknown } | null | undefined;
    return args && typeof args.query === "string" ? args.query : "";
  }
  return null;
}

function renderSearchGroup(label: string, query: string, cardsHtml: string, live: boolean): string {
  return `
    <div class="search-group"${live ? " data-live-strip" : ""}>
      <div class="search-group__label">
        <span class="search-group__tool">${esc(label)}</span>
        ${query ? `<span class="search-group__q">${esc(query)}</span>` : ""}
      </div>
      <div class="search-group__row">${cardsHtml}</div>
    </div>
  `;
}

// Note refs an event surfaced: the design's `{type:"note", data:{ref}}`
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

// ── quiet meta line under an agent message ───────────────────────────
function renderConvMeta(bits: string[]): string {
  if (bits.length === 0) return "";
  return `<div class="conv-meta">${bits
    .map((bit) => `<span>${esc(bit)}</span>`)
    .join(`<span class="conv-meta__sep" aria-hidden="true">·</span>`)}</div>`;
}

// ── composer (pinned) — starts a new task OR continues the thread ────
function renderComposer(c: ComposerProps): string {
  const connected = c.status.state === "connected";
  const connecting = c.status.state === "connecting";
  const disabled = c.submitting || c.running;
  // A remote profile submits while disconnected: the run reconnects (minting
  // a fresh hosted session) on demand.
  const needsConnection = !connected && !c.remoteProfile;
  const sendDisabled = disabled
    || !c.value.trim()
    || needsConnection
    || !c.modelReady
    || c.voice.phase !== "idle";
  const voiceDisabled = c.voice.phase === "recording"
    ? false
    : disabled
      || c.voice.phase === "requesting"
      || c.voice.phase === "transcribing"
      || !c.voice.available;
  const voiceTitle = disabled && c.voice.phase === "idle"
    ? t("voice.unavailable.taskBusy")
    : c.voice.title;
  const placeholder = c.running
    ? t("task.working")
    : c.mode === "new"
      ? t("task.agentPlaceholder")
      : t("task.replyPlaceholder");
  const glyph = c.submitting
    ? `<span class="composer__send-dot" aria-hidden="true"></span>`
    : `<svg class="composer__send-glyph" viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="16 7 16 13 8 13"></polyline><polyline points="11 10 8 13 11 16"></polyline></svg>`;
  const voiceGlyph = c.voice.phase === "recording"
    ? `<span class="composer__voice-stop" aria-hidden="true"></span>`
    : c.voice.phase === "requesting" || c.voice.phase === "transcribing"
      ? `<span class="composer__send-dot" aria-hidden="true"></span>`
      : `<svg class="composer__voice-glyph" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="3" width="6" height="11" rx="3"></rect><path d="M5.5 10.5a6.5 6.5 0 0 0 13 0"></path><path d="M12 17v4"></path><path d="M9 21h6"></path></svg>`;
  const connectHint = connected
    ? ""
    : c.remoteProfile
      ? `<p class="composer__hint">${esc(t("chrome.remoteAutoReconnect"))}</p>`
      : `
      <p class="composer__hint">
        ${esc(t(c.mode === "new" ? "chrome.connectToStart" : "task.replyConnectHint"))}
        <button id="composer-connect" type="button" class="btn-ghost btn-compact" ${connecting ? "disabled" : ""}>
          ${esc(connecting ? t("chrome.connectingCta") : t("chrome.connectCta"))}
        </button>
      </p>`;
  const keyHint =
    connected && !c.modelReady ? `<p class="composer__hint">${esc(t("task.addKeyHint"))}</p>` : "";
  return `
    <div class="composer">
      <form id="composer-form" class="composer__form">
        <div class="composer__row ${disabled ? "is-disabled" : ""}">
          <textarea
            id="composer-input"
            class="composer__input"
            rows="1"
            placeholder="${esc(placeholder)}"
            ${disabled ? "disabled" : ""}
          >${esc(c.value)}</textarea>
          <span class="composer__voice-wrap" title="${esc(voiceTitle)}">
            <button
              id="composer-voice"
              type="button"
              class="composer__voice ${c.voice.phase === "recording" ? "is-recording" : ""}"
              aria-label="${esc(voiceTitle)}"
              aria-pressed="${c.voice.phase === "recording" ? "true" : "false"}"
              ${voiceDisabled ? "disabled" : ""}
            >${voiceGlyph}</button>
          </span>
          <button
            id="composer-send"
            type="submit"
            class="composer__send"
            title="${esc(sendShortcutLabel)}"
            aria-label="${esc(t(c.mode === "new" ? "task.new" : "task.replySend"))}"
            ${sendDisabled ? "disabled" : ""}
          >${glyph}</button>
        </div>
      </form>
      ${connectHint}
      ${keyHint}
      ${c.voice.error ? `<pre class="composer__error">${esc(c.voice.error)}</pre>` : ""}
      ${c.error ? `<pre class="composer__error">${esc(c.error)}</pre>` : ""}
    </div>
  `;
}

// ── run grouping (shared event plumbing) ─────────────────────────────

// The shell caps event text at 8k chars and marks the cut with this suffix.
const EVENT_TRUNCATION_SUFFIX = "\n... [truncated]";

function stripTruncation(text: string): string {
  return text.endsWith(EVENT_TRUNCATION_SUFFIX) ? text.slice(0, -EVENT_TRUNCATION_SUFFIX.length) : text;
}

function startsNewRunGroup(
  ev: AgentTaskEventPayload,
  currentGroup: AgentTaskEventPayload[] | undefined,
): boolean {
  if (!currentGroup) return true;
  if (ev.kind === "queued") return true;
  return ev.kind === "started" && currentGroup.some((e) => e.kind === "started");
}

// The thread's copy of the final answer: the last assistant event, but only
// when task.final_text is showing the same text. final_text hydrates from
// report.md — the loop's final text plus an optional artifacts appendix — and
// the event may be truncated, so "same" means the event text prefixes the
// final text, not equality. A failed run's error matches no assistant text,
// so commentary before a failure stays readable. While a task runs,
// final_text is unset and the answer streams into the activity in full.
function finalAnswerEventIndex(task: AgentTaskView): number {
  const finalText = task.final_text?.trim();
  if (!finalText) return -1;
  for (let index = task.events.length - 1; index >= 0; index -= 1) {
    const ev = task.events[index];
    if (ev.kind !== "assistant") continue;
    const text = stripTruncation(ev.text).trim();
    return text && finalText.startsWith(text) ? index : -1;
  }
  return -1;
}

function groupRunEvents(task: AgentTaskView, duplicateIndex: number): AgentTaskEventPayload[][] {
  const groups: AgentTaskEventPayload[][] = [];
  task.events.forEach((ev, index) => {
    if (index === duplicateIndex) return;
    if (startsNewRunGroup(ev, groups[groups.length - 1])) {
      groups.push([]);
    }
    groups[groups.length - 1].push(ev);
  });
  return groups;
}

// Elapsed run time: started→finished, or started→now while still running.
// A terminal task without a finished_at has no meaningful end, so we show no
// duration rather than a figure that keeps ticking up against the wall clock.
function elapsedDurationMs(task: Pick<AgentTaskSnapshot, "started_at" | "finished_at" | "status">): number | null {
  if (!task.started_at) return null;
  const running = task.status === "running" || task.status === "queued";
  const end = task.finished_at ?? (running ? Date.now() : null);
  if (end === null) return null;
  return Math.max(0, end - task.started_at);
}

function formatDurationMs(durationMs: number): string {
  const seconds = Math.max(0, Math.round(durationMs / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

function turnMetrics(task: AgentTaskView, events: AgentTaskEventPayload[], isLast: boolean): TurnMetrics {
  if (isLast && (task.status === "queued" || task.status === "running")) {
    return metricsFromSnapshot(task);
  }
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const snapshot = events[index].snapshot;
    if (snapshot) return metricsFromSnapshot(snapshot);
  }
  const done = [...events].reverse().find((event) => event.kind === "done");
  if (done) {
    return {
      steps: done.steps ?? null,
      durationMs: done.duration_ms ?? null,
      inputTokens: done.input_tokens ?? null,
      outputTokens: done.output_tokens ?? null,
      cachedInputTokens: done.cached_input_tokens ?? null,
      cacheCreationInputTokens: done.cache_creation_input_tokens ?? null,
      estimatedCost: done.estimated_cost ?? null,
      costCurrency: done.cost_currency ?? null,
      pointsUsed: done.points_used ?? null,
    };
  }
  return isLast ? metricsFromSnapshot(task) : emptyMetrics();
}

function metricsFromSnapshot(task: AgentTaskSnapshot | AgentTaskView): TurnMetrics {
  return {
    steps: task.steps,
    durationMs: elapsedDurationMs(task),
    inputTokens: task.input_tokens,
    outputTokens: task.output_tokens,
    cachedInputTokens: task.cached_input_tokens,
    cacheCreationInputTokens: task.cache_creation_input_tokens,
    estimatedCost: task.estimated_cost,
    costCurrency: task.cost_currency,
    pointsUsed: task.points_used,
  };
}

function emptyMetrics(): TurnMetrics {
  return {
    steps: null,
    durationMs: null,
    inputTokens: null,
    outputTokens: null,
    cachedInputTokens: null,
    cacheCreationInputTokens: null,
    estimatedCost: null,
    costCurrency: null,
    pointsUsed: null,
  };
}

function activityMetricBits(task: AgentTaskView, metrics: TurnMetrics, stepCount: number): string[] {
  const hosted = task.provider === "socai";
  const bits: string[] = [];
  if (stepCount) bits.push(formatStepCount(stepCount));
  if (metrics.durationMs !== null) bits.push(formatDurationMs(metrics.durationMs));
  if (metrics.inputTokens !== null && metrics.outputTokens !== null) {
    bits.push(formatTokenUsage(
      metrics.inputTokens,
      metrics.outputTokens,
      metrics.cachedInputTokens,
      metrics.cacheCreationInputTokens ?? 0,
      hosted ? null : metrics.estimatedCost,
      hosted ? null : metrics.costCurrency,
    ));
  }
  if (metrics.pointsUsed !== null && (hosted || metrics.pointsUsed > 0)) {
    bits.push(t("billing.pointsUsed", { points: metrics.pointsUsed }));
  }
  return bits;
}

export function liveActivityMetricsText(task: AgentTaskView): string {
  const groups = groupRunEvents(task, finalAnswerEventIndex(task));
  const events = groups.at(-1) ?? [];
  const metrics = turnMetrics(task, events, true);
  const stepCount = metrics.steps
    ?? (events.filter((event) => event.kind === "step").length
      || events.filter((event) => event.kind === "tool_call").length);
  const bits = activityMetricBits(task, metrics, stepCount);
  return bits.length > 0 ? `· ${bits.join(" · ")}` : "";
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
    case "tool_progress": return "·";
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
