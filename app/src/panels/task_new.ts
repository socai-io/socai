import type { ModelInfo, Status, ShellState } from "../main";
import { esc } from "../lib/html";
import {
  formatRunningTaskCount,
  getLocale,
  taskStatusLabel,
  t,
} from "../lib/i18n";
import type { AgentTaskView, TaskMode, ToolCommand } from "./tasks";

export interface NewTaskPageProps {
  shell: ShellState;
  mode: TaskMode;
  toolCommand: ToolCommand;
  draft: string;
  submittingTask: boolean;
  toolInFlight: boolean;
  toolResult: unknown;
  toolError: string;
  submitError: string;
  tasks: AgentTaskView[];
  selectedModel: ModelInfo | undefined;
}

export function renderNewTaskPage(props: NewTaskPageProps): string {
  const connected = props.shell.status.state === "connected";
  const modelReady = !!props.selectedModel && props.selectedModel.has_key;
  const agentMode = props.mode === "agent";
  const running = agentMode ? props.submittingTask : props.toolInFlight;
  const runDisabled = running || !props.draft.trim() || !connected || (agentMode && !modelReady);
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
            ${renderTaskForm(props, agentMode, running, runDisabled)}
            ${renderInlineGuard(props.mode, props.toolCommand, props.selectedModel)}
            ${props.submitError ? `<pre class="result-pre result-error">${esc(props.submitError)}</pre>` : ""}
            ${agentMode ? "" : renderToolResult(props)}
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
  agentMode: boolean,
  running: boolean,
  runDisabled: boolean,
): string {
  return `
    <form id="task-form" class="task-form task-form-centered">
      <textarea
        id="task-input"
        class="task-input"
        rows="5"
        placeholder="${esc(taskPlaceholder(props.mode, props.toolCommand))}"
        ${running ? "disabled" : ""}
      >${esc(props.draft)}</textarea>

      <div class="task-controls">
        <div class="mode-switch" aria-label="${esc(t("task.modeAria"))}">
          <button id="mode-agent" type="button" class="mode-button ${agentMode ? "mode-button-active" : ""}">${esc(t("task.modeAgent"))}</button>
          <button id="mode-tools" type="button" class="mode-button ${!agentMode ? "mode-button-active" : ""}">${esc(t("task.modeTools"))}</button>
        </div>
        ${agentMode ? renderAgentSummary(props.selectedModel) : renderToolPicker(props.toolCommand)}
        <button id="task-submit" type="submit" class="btn-primary" ${runDisabled ? "disabled" : ""}>
          ${running ? esc(t("task.starting")) : agentMode ? esc(t("task.new")) : esc(t("task.runTest"))}
        </button>
      </div>
    </form>
  `;
}

function renderInlineGuard(mode: TaskMode, toolCommand: ToolCommand, selected: ModelInfo | undefined): string {
  if (mode !== "agent") return renderToolHint(toolCommand);
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

function renderToolPicker(toolCommand: ToolCommand): string {
  const tools: Array<[ToolCommand, string]> = [
    ["search_notes", t("tool.searchNotes")],
    ["topic_scan", t("tool.topicScan")],
    ["extract_note", t("tool.extractNote")],
  ];
  return `
    <div class="tool-picker" aria-label="${esc(t("tool.pickerAria"))}">
      ${tools.map(([cmd, label]) => `
        <button type="button" data-tool="${cmd}" class="tool-choice ${toolCommand === cmd ? "tool-choice-active" : ""}">
          ${esc(label)}
        </button>
      `).join("")}
    </div>
  `;
}

function renderToolHint(toolCommand: ToolCommand): string {
  const hint = {
    search_notes: t("tool.hintSearchNotes"),
    topic_scan: t("tool.hintTopicScan"),
    extract_note: t("tool.hintExtractNote"),
  }[toolCommand];
  return `<p class="t-small subtle">${esc(hint)}</p>`;
}

function taskPlaceholder(mode: TaskMode, toolCommand: ToolCommand): string {
  if (mode === "agent") return t("task.agentPlaceholder");
  switch (toolCommand) {
    case "search_notes": return t("tool.placeholderSearch");
    case "topic_scan": return t("tool.placeholderTopic");
    case "extract_note": return t("tool.placeholderNote");
  }
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
            <span class="task-row-meta">${esc(taskStatusLabel(task.status))} · ${esc(formatTime(task.created_at))}</span>
          </span>
        </button>
      `).join("")}
    </div>
  `;
}

function renderToolResult(props: NewTaskPageProps): string {
  if (props.toolInFlight) {
    return `<pre class="result-pre result-running">${esc(t("tool.running"))}: ${esc(props.toolCommand)}(${esc(JSON.stringify(props.draft.trim()))})</pre>`;
  }
  if (props.toolError) {
    return `<pre class="result-pre result-error">${esc(props.toolError)}</pre>`;
  }
  if (props.toolResult) {
    return `<pre class="result-pre">${esc(JSON.stringify(props.toolResult, null, 2))}</pre>`;
  }
  return `<p class="t-small placeholder">${esc(t("tool.noResult"))}</p>`;
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

function formatTime(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleTimeString(getLocale(), { hour: "2-digit", minute: "2-digit" });
}
