//! Compose view: the centered hero + task textarea shown in the workspace when
//! no task is selected (or after "new task"). When chrome is disconnected the
//! form is masked behind a connect overlay.

import type { ModelInfo, Status, ShellState } from "../main";
import { esc } from "../lib/html";
import { t } from "../lib/i18n";
import { sendShortcutLabel } from "../lib/shortcuts";

export interface ComposePaneProps {
  shell: ShellState;
  draft: string;
  submittingTask: boolean;
  submitError: string;
  selectedModel: ModelInfo | undefined;
}

export function renderComposePane(props: ComposePaneProps): string {
  const connected = props.shell.status.state === "connected";
  const modelReady = !!props.selectedModel && props.selectedModel.has_key;
  const running = props.submittingTask;
  const runDisabled = running || !props.draft.trim() || !connected || !modelReady;
  const gated = !connected;

  return `
    <div class="compose-pane">
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
    </div>
  `;
}

function renderTaskForm(
  props: ComposePaneProps,
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
        <button id="task-submit" type="submit" class="btn-primary" title="${esc(sendShortcutLabel)}" ${runDisabled ? "disabled" : ""}>
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
