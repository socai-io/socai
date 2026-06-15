//! Tauri desktop entry — header status/configuration plus tools / agent panels.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { applyLanguageToDocument, formatTabs, getLanguage, isSupportedLanguage, setLanguage, t } from "./lib/i18n";
import { agentPanel } from "./panels/tasks";

export type Status =
  | { state: "disconnected"; reason: string }
  | { state: "connecting"; attempt: number }
  | {
      state: "connected";
      endpoint: string;
      browser_version: string;
      page_count: number;
      source?: string;
      managed?: boolean;
      user_data_dir?: string | null;
    };

export interface ModelInfo {
  provider: string;
  provider_display_name?: string;
  display_name: string;
  /** Concrete executable model id. Kept alongside default_model while older backend rows migrate. */
  model_id?: string;
  /** Back-compat: now also carries the concrete executable model id for each row. */
  default_model: string;
  selected_model?: string;
  has_key: boolean;
  credential_kind?: "api_key" | "codex_oauth" | null;
  is_default?: boolean;
  recommended?: boolean;
  source?: string | null;
}

export type AgentTaskStatus = "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted";

export interface AgentTaskSnapshot {
  task_id: string;
  task: string;
  model: string | null;
  status: AgentTaskStatus;
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
  run_id: string | null;
  run_dir: string | null;
  target_id: string | null;
  final_text: string | null;
  error: string | null;
  turns: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
}

export interface TimelineEntity {
  type: string;
  data: unknown;
}

export interface AgentTaskEventPayload {
  task_id: string;
  kind:
    | "queued"
    | "running"
    | "started"
    | "tab"
    | "turn"
    | "assistant"
    | "reasoning"
    | "tool_call"
    | "tool_result"
    | "tool_error"
    | "api_error"
    | "done"
    | "completed"
    | "failed"
    | "cancelled"
    | "interrupted";
  text: string;
  snapshot?: AgentTaskSnapshot | null;
  sequence: number;
  created_at: number;
  turn?: number;
  id?: string;
  sequence_in_turn?: number;
  name?: string;
  label?: string;
  args?: unknown;
  repeat_count?: number;
  ok?: boolean;
  summary?: string;
  duration_ms?: number;
  entities?: TimelineEntity[];
  error?: string | null;
  result_file?: string | null;
  run_id?: string | null;
  model?: string;
  task?: string;
  target_id?: string | null;
}

export interface ShellState {
  status: Status;
  rerender: () => void;
}

const MARK_SVG = `
  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="24" height="24" fill="none" role="img" aria-label="socai">
    <rect x="2.5" y="2.5" width="27" height="27" rx="3" stroke="currentColor" stroke-width="1.6"></rect>
    <rect x="16" y="16" width="10" height="10" rx="1.2" fill="currentColor"></rect>
  </svg>
`;

interface PanelModule {
  label: string;
  render: (shell: ShellState) => string;
  bind: (shell: ShellState) => void;
}

const PANELS: PanelModule[] = [
  { label: "tasks", render: agentPanel.render, bind: agentPanel.bind },
];

let status: Status = { state: "disconnected", reason: "starting" };
let connectionDetailsOpen = false;

function shell(): ShellState {
  return { status, rerender: render };
}

function render(): void {
  const root = document.getElementById("app");
  if (!root) return;
  const state = shell();
  const sections = PANELS
    .map(
      (p) => `
      <section class="section">
        ${p.render(state)}
      </section>`,
    )
    .join("");
  root.innerHTML = `
    <div class="shell">
      <header class="topbar">
        <div class="brand">${MARK_SVG}<span class="brand-name">socai</span></div>
        <div class="topbar-controls">
          ${renderLanguageSwitch()}
          ${connectionStatusBar()}
          ${agentPanel.renderHeader()}
        </div>
      </header>
      <main class="stack">${sections}</main>
    </div>
  `;
  bindLanguageSwitch();
  bindConnectionStatusBar();
  agentPanel.bindHeader(state);
  for (const p of PANELS) p.bind(state);
}

function renderLanguageSwitch(): string {
  const language = getLanguage();
  return `
    <div class="language-toggle" role="group" aria-label="${htmlEsc(t("language.switcherAria"))}">
      <button class="language-toggle__button" type="button" data-lang-option="zh" aria-pressed="${language === "zh" ? "true" : "false"}">中文</button>
      <button class="language-toggle__button" type="button" data-lang-option="en" aria-pressed="${language === "en" ? "true" : "false"}">en</button>
    </div>
  `;
}

function bindLanguageSwitch(): void {
  document.querySelectorAll<HTMLButtonElement>("[data-lang-option]").forEach((option) => {
    option.addEventListener("click", () => {
      const nextLanguage = option.dataset.langOption;
      if (!isSupportedLanguage(nextLanguage) || getLanguage() === nextLanguage) return;
      setLanguage(nextLanguage);
      render();
    });
  });
}

function connectionStatusBar(): string {
  return `
    <div class="connection-status" aria-live="polite">
      ${connectionBadge()}
      ${status.state === "connected" && connectionDetailsOpen ? renderConnectionDialog(status) : ""}
    </div>
  `;
}

function connectionBadge(): string {
  switch (status.state) {
    case "disconnected":
      return `<button id="chrome-connect" type="button" class="badge badge-button" aria-label="${htmlEsc(t("chrome.connectAria"))}"><i class="badge-dot badge-dot-muted" aria-hidden="true"></i>${htmlEsc(t("chrome.label"))} · ${htmlEsc(t("chrome.disconnected"))}</button>`;
    case "connecting":
      return `<button type="button" class="badge badge-button" disabled><i class="badge-dot badge-dot-ink badge-dot-pulse" aria-hidden="true"></i>${htmlEsc(t("chrome.label"))} · ${htmlEsc(t("chrome.connecting"))} · ${status.attempt}/3</button>`;
    case "connected":
      return `<button id="chrome-status-toggle" type="button" class="badge badge-button" aria-expanded="${connectionDetailsOpen ? "true" : "false"}" aria-label="${htmlEsc(t("chrome.statusToggleAria"))}"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i>${htmlEsc(t("chrome.label"))} · ${htmlEsc(t("chrome.connected"))}</button>`;
  }
}

function renderConnectionDialog(connected: Extract<Status, { state: "connected" }>): string {
  const tabs = formatTabs(connected.page_count);
  return `
    <div class="topbar-popover connection-dialog" role="dialog" aria-label="${htmlEsc(t("chrome.dialogAria"))}">
      <div class="connection-dialog-head">
        <p class="t-eyebrow connection-dialog-title">${htmlEsc(t("chrome.label"))}</p>
        <span class="badge"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i>${htmlEsc(t("chrome.connected"))}</span>
      </div>
      <div class="connection-meta">
        <div>
          <p class="t-eyebrow">${htmlEsc(t("chrome.tabs"))}</p>
          <p class="t-mono">${htmlEsc(tabs)}</p>
        </div>
        <div>
          <p class="t-eyebrow">${htmlEsc(t("chrome.browser"))}</p>
          <p class="t-mono">${htmlEsc(connected.browser_version)}</p>
        </div>
        <div>
          <p class="t-eyebrow">${htmlEsc(t("chrome.source"))}</p>
          <p class="t-mono">${htmlEsc(connected.managed ? t("chrome.sourceManaged") : t("chrome.sourceExisting"))}</p>
        </div>
        ${connected.user_data_dir ? `
        <div class="connection-meta-wide">
          <p class="t-eyebrow">${htmlEsc(t("chrome.profile"))}</p>
          <p class="t-mono connection-endpoint">${htmlEsc(connected.user_data_dir)}</p>
        </div>` : ""}
        <div class="connection-meta-wide">
          <p class="t-eyebrow">${htmlEsc(t("chrome.endpoint"))}</p>
          <p class="t-mono connection-endpoint">${htmlEsc(connected.endpoint)}</p>
        </div>
      </div>
      <button id="chrome-disconnect" type="button" class="btn-ghost">${htmlEsc(t("chrome.disconnect"))}</button>
    </div>
  `;
}

function bindConnectionStatusBar(): void {
  document.getElementById("chrome-connect")?.addEventListener("click", () => {
    connectionDetailsOpen = false;
    invoke("cdp_connect").catch((e) => console.error("cdp_connect failed:", e));
  });
  document.getElementById("chrome-status-toggle")?.addEventListener("click", async () => {
    const opening = !connectionDetailsOpen;
    if (opening) {
      try {
        status = await invoke<Status>("cdp_status");
      } catch (e) {
        console.error("cdp_status failed:", e);
      }
    }
    connectionDetailsOpen = opening;
    render();
  });
  document.getElementById("chrome-disconnect")?.addEventListener("click", () => {
    connectionDetailsOpen = false;
    invoke("cdp_disconnect").catch((e) => console.error("cdp_disconnect failed:", e));
  });
}

function htmlEsc(s: string): string {
  return s.replace(/[<>&"']/g, (c) => {
    return (
      { "<": "&lt;", ">": "&gt;", "&": "&amp;", '"': "&quot;", "'": "&#39;" } as Record<string, string>
    )[c];
  });
}

function bindGlobalDismiss(): void {
  document.addEventListener("click", (event) => {
    let changed = false;

    if (connectionDetailsOpen && !eventPathHasClass(event, "connection-status")) {
      connectionDetailsOpen = false;
      changed = true;
    }
    if (!eventPathHasClass(event, "agent-status") && agentPanel.closeHeaderConfig()) {
      changed = true;
    }

    if (changed) render();
  });
}

function eventPathHasClass(event: Event, className: string): boolean {
  return event.composedPath().some((item) => item instanceof Element && item.classList.contains(className));
}

async function main(): Promise<void> {
  applyLanguageToDocument();

  await listen<Status>("cdp:status_changed", (event) => {
    status = event.payload;
    if (status.state !== "connected") connectionDetailsOpen = false;
    render();
  });

  // Stream task-scoped agent events incrementally. Snapshot/status events ask
  // for a full render so the task list and final answer update; normal stream
  // rows append in place to preserve scroll.
  await listen<AgentTaskEventPayload>("agent_task:event", (event) => {
    if (agentPanel.appendTaskEvent(event.payload)) render();
  });

  let initialTasks: AgentTaskSnapshot[] = [];
  try {
    status = await invoke<Status>("cdp_status");
  } catch (e) {
    console.error("initial cdp_status failed:", e);
  }
  try {
    const models = await invoke<ModelInfo[]>("agent_list_models");
    agentPanel.setModels(models);
  } catch (e) {
    console.error("agent_list_models failed:", e);
  }
  try {
    initialTasks = await invoke<AgentTaskSnapshot[]>("agent_task_list");
    agentPanel.setTasks(initialTasks);
  } catch (e) {
    console.error("agent_task_list failed:", e);
  }
  render();
  bindGlobalDismiss();
  void hydrateTaskEvents(initialTasks);

  const refresh = (): void => {
    invoke("cdp_refresh").catch((e) => console.error("cdp_refresh failed:", e));
    agentPanel.refreshModels()
      .then(() => render())
      .catch((e) => console.error("agent_list_models refresh failed:", e));
  };
  window.addEventListener("focus", refresh);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") refresh();
  });
}

async function hydrateTaskEvents(tasks: AgentTaskSnapshot[]): Promise<void> {
  let changed = false;
  await Promise.all(
    tasks.map(async (task) => {
      try {
        const events = await invoke<AgentTaskEventPayload[]>("agent_task_events", { taskId: task.task_id });
        if (agentPanel.setTaskEvents(task.task_id, events)) changed = true;
      } catch (e) {
        console.error("agent_task_events failed:", e);
      }
    }),
  );
  if (changed) render();
}

main();
