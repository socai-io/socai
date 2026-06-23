import type { ModelInfo, Status, ShellState } from "../main";
import { esc } from "../lib/html";
import {
  formatRunningTaskCount,
  formatTaskTimestamp,
  taskStatusLabel,
  t,
} from "../lib/i18n";
import type { AgentTaskView } from "./tasks";

export interface NewTaskPageProps {
  shell: ShellState;
  draft: string;
  submittingTask: boolean;
  submitError: string;
  tasks: AgentTaskView[];
  selectedModel: ModelInfo | undefined;
}

export function renderNewTaskPage(props: NewTaskPageProps): string {
  const connected = props.shell.status.state === "connected";
  const modelReady = !!props.selectedModel && props.selectedModel.has_key;
  const running = props.submittingTask;
  const runDisabled = running || !props.draft.trim() || !connected || !modelReady;
  const gated = !connected;

  return `
    <div class="new-task-page">
      <div class="new-task-compose">
        <div class="new-task-copy">
          <h2 class="t-h2">${esc(t("task.hero"))}</h2>
          <p class="t-small subtle">${esc(t("task.lede"))}</p>
        </div>
        <div class="compose-form-stack ${gated ? "is-masked" : ""}">
          <div class="compose-form-inner" aria-hidden="${gated ? "true" : "false"}">
            ${renderTaskForm(props, running, runDisabled)}
            ${renderInlineGuard(props.selectedModel)}
            ${props.submitError ? `<pre class="result-pre result-error">${esc(props.submitError)}</pre>` : ""}
          </div>
          ${!connected ? renderConnectOverlay(props.shell.status) : ""}
        </div>
      </div>
      ${renderRunningChip(props.tasks)}
      ${renderTaskGlance(props.tasks)}
    </div>
  `;
}

function renderTaskForm(
  props: NewTaskPageProps,
  running: boolean,
  runDisabled: boolean,
): string {
  return `
    <form id="task-form" class="task-form task-form-centered">
      <textarea
        id="task-input"
        class="task-input"
        rows="5"
        placeholder="${esc(t("task.agentPlaceholder"))}"
        ${running ? "disabled" : ""}
      >${esc(props.draft)}</textarea>

      <div class="task-controls">
        ${renderAgentSummary(props.selectedModel)}
        <button id="task-submit" type="submit" class="btn-primary" ${runDisabled ? "disabled" : ""}>
          ${running ? esc(t("task.starting")) : esc(t("task.new"))}
        </button>
      </div>
    </form>
  `;
}

function renderInlineGuard(selected: ModelInfo | undefined): string {
  if (!selected) return `<p class="t-small subtle">${esc(t("task.loadingModels"))}</p>`;
  if (!selected.has_key) return `<p class="t-small subtle">${esc(t("task.addKeyHint"))}</p>`;
  return "";
}

function renderAgentSummary(selected: ModelInfo | undefined): string {
  const modelId = selected?.model_id || selected?.default_model || "";
  const provider = selected?.provider_display_name || selected?.provider || "";
  const summary = selected
    ? `${esc(t("agent.label"))} · ${esc(provider)} · <span class="t-mono">${esc(modelId)}</span>`
    : `${esc(t("agent.label"))} · ${esc(t("agent.loading"))}`;
  return `<p class="t-small subtle task-context">${summary}</p>`;
}

function renderConnectOverlay(status: Status): string {
  const connecting = status.state === "connecting";
  const label = connecting
    ? `${t("chrome.label")} · ${t("chrome.connecting")} · ${(status as Extract<Status, { state: "connecting" }>).attempt}/3`
    : `${t("chrome.label")} · ${t("chrome.disconnected")}`;
  const heading = connecting ? t("chrome.lookingForChrome") : t("chrome.connectToStart");
  const cta = connecting ? t("chrome.connectingCta") : t("chrome.connectCta");
  const dotClass = connecting ? "badge-dot-ink badge-dot-pulse" : "badge-dot-hollow";
  return `
    <div class="connect-overlay" role="dialog" aria-label="${esc(t("chrome.requiredAria"))}">
      <span class="connect-overlay-pill">
        <i class="badge-dot ${dotClass}" aria-hidden="true"></i>${esc(label)}
      </span>
      <h3 class="connect-overlay-head">${esc(heading)}</h3>
      <button
        id="overlay-chrome-connect"
        type="button"
        class="btn-primary connect-overlay-cta"
        ${connecting ? "disabled" : ""}
      >${esc(cta)}</button>
      <a
        id="overlay-remote-debugging-help"
        class="connect-overlay-link t-small"
        href="https://socai.io/connect"
        target="_blank"
        rel="noopener noreferrer"
      >${esc(t("chrome.remoteDebuggingHelp"))}</a>
    </div>
  `;
}

function renderRunningChip(tasks: AgentTaskView[]): string {
  const running = [...tasks]
    .filter((t) => t.status === "running" || t.status === "queued")
    .sort((a, b) => b.created_at - a.created_at);
  if (running.length === 0) return "";
  const first = running[0];
  const isOne = running.length === 1;
  const count = formatRunningTaskCount(running.length);
  const taskLabel = isOne
    ? `<span class="running-chip-dot" aria-hidden="true">·</span><span class="running-chip-task">${esc(first.task)}</span>`
    : "";
  return `
    <button type="button" class="running-chip" data-task-id="${esc(first.task_id)}">
      <i class="badge-dot badge-dot-ink badge-dot-pulse" aria-hidden="true"></i>
      <span class="running-chip-count">${count}</span>
      ${taskLabel}
      <span class="running-chip-arrow" aria-hidden="true">→</span>
    </button>
  `;
}

function renderTaskGlance(tasks: AgentTaskView[]): string {
  const recent = [...tasks]
    .filter((task) => task.status !== "running" && task.status !== "queued")
    .sort((a, b) => b.created_at - a.created_at)
    .slice(0, 5);

  return `
    <div class="task-glance">
      <section class="task-glance-card">
        <div class="task-glance-head">
          <p class="t-eyebrow result-label">${esc(t("task.recent"))}</p>
          <button id="recent-history-link" type="button" class="btn-ghost btn-compact">${esc(t("task.viewHistory"))}</button>
        </div>
        ${renderTaskSummaryRows(recent, t("task.noRecent"))}
      </section>
    </div>
  `;
}

function renderTaskSummaryRows(items: AgentTaskView[], emptyText: string): string {
  if (items.length === 0) {
    return `<p class="t-small placeholder task-summary-empty">${esc(emptyText)}</p>`;
  }
  return `
    <div class="task-summary-list">
      ${items.map((task) => `
        <button type="button" class="task-summary-row" data-task-id="${esc(task.task_id)}">
          <span class="task-row-glyph task-row-glyph-${esc(task.status)}" aria-hidden="true">${taskStatusGlyph(task.status)}</span>
          <span class="task-row-main">
            <span class="task-row-title">${esc(task.task)}</span>
            <span class="task-row-meta">${esc(taskStatusLabel(task.status))} · ${esc(formatTaskTimestamp(task.created_at))}</span>
          </span>
        </button>
      `).join("")}
    </div>
  `;
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
