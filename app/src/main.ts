//! Tauri desktop entry — header status/configuration plus tools / agent panels.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

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
          ${updateStatusBar()}
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
  bindUpdateStatusBar();
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

// ---------------------------------------------------------------------------
// In-app updater (macOS). Mirrors the connection-status badge + popover above.
// The updater plugin is configured in tauri.conf.json; check() is gated off in
// dev because the plugin only truly downloads/installs in bundled builds.
// ---------------------------------------------------------------------------

type UpdatePhase = "idle" | "available" | "downloading" | "ready" | "error";

interface UpdateState {
  phase: UpdatePhase;
  version?: string;
  notes?: string;
  downloaded?: number;
  total?: number;
  error?: string;
}

const RELEASES_URL = "https://github.com/socai-io/socai/releases/latest";

let updateState: UpdateState = { phase: "idle" };
let updateHandle: Update | null = null;
let updatePopoverOpen = false;

function updateStatusBar(): string {
  if (updateState.phase === "idle") return "";
  return `
    <div class="update-status" aria-live="polite">
      ${updateBadge()}
      ${updatePopoverOpen ? renderUpdateDialog() : ""}
    </div>
  `;
}

function updateBadge(): string {
  const expanded = updatePopoverOpen ? "true" : "false";
  const aria = htmlEsc(t("update.toggleAria"));
  const open = `<button id="update-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}" aria-label="${aria}">`;
  switch (updateState.phase) {
    case "available":
      return `${open}<i class="badge-dot badge-dot-hollow" aria-hidden="true"></i>${htmlEsc(t("update.available"))}</button>`;
    case "downloading":
      return `${open}<i class="badge-dot badge-dot-ink badge-dot-pulse" aria-hidden="true"></i>${htmlEsc(t("update.downloading"))} <span data-update-progress>${htmlEsc(formatUpdateProgress())}</span></button>`;
    case "ready":
      return `${open}<i class="badge-dot badge-dot-ink" aria-hidden="true"></i>${htmlEsc(t("update.restartToFinish"))}</button>`;
    case "error":
      return `${open}<i class="badge-dot badge-dot-muted" aria-hidden="true"></i>${htmlEsc(t("update.failed"))}</button>`;
    default:
      return "";
  }
}

function formatUpdateProgress(): string {
  if (updateState.total && updateState.total > 0 && updateState.downloaded != null) {
    const pct = Math.min(100, Math.round((updateState.downloaded / updateState.total) * 100));
    return `${pct}%`;
  }
  return "…";
}

function renderUpdateDialog(): string {
  const version = updateState.version ?? "";
  const head = `
    <div class="connection-dialog-head">
      <p class="t-eyebrow connection-dialog-title">${htmlEsc(t("update.title"))}</p>
      ${version ? `<span class="badge"><i class="badge-dot badge-dot-hollow" aria-hidden="true"></i>${htmlEsc(version)}</span>` : ""}
    </div>`;

  let body = "";
  let actions = "";

  switch (updateState.phase) {
    case "available":
      body = `
        <div>
          <p class="t-eyebrow">${htmlEsc(t("update.newVersion"))}</p>
          <p class="t-mono">${htmlEsc(version)}</p>
        </div>
        ${updateState.notes ? `<p class="t-small subtle update-notes">${htmlEsc(updateState.notes)}</p>` : ""}`;
      actions = `
        <button id="update-start" type="button" class="btn-primary btn-compact">${htmlEsc(t("update.upgradeAndRestart"))}</button>
        <button id="update-github" type="button" class="btn-ghost btn-compact">${htmlEsc(t("update.viewOnGithub"))}</button>`;
      break;
    case "downloading":
      body = `
        <div>
          <p class="t-eyebrow">${htmlEsc(t("update.downloading"))}</p>
          <p class="t-mono" data-update-progress>${htmlEsc(formatUpdateProgress())}</p>
        </div>`;
      break;
    case "ready":
      body = `<p class="t-small subtle">${htmlEsc(t("update.readyHint"))}</p>`;
      actions = `<button id="update-restart" type="button" class="btn-primary btn-compact">${htmlEsc(t("update.restartNow"))}</button>`;
      break;
    case "error":
      body = `<p class="t-small subtle update-notes">${htmlEsc(updateState.error ?? t("update.failed"))}</p>`;
      actions = `
        <button id="update-start" type="button" class="btn-primary btn-compact">${htmlEsc(t("update.retry"))}</button>
        <button id="update-github" type="button" class="btn-ghost btn-compact">${htmlEsc(t("update.viewOnGithub"))}</button>`;
      break;
  }

  return `
    <div class="topbar-popover update-dialog" role="dialog" aria-label="${htmlEsc(t("update.dialogAria"))}">
      ${head}
      ${body}
      ${actions ? `<div class="update-actions">${actions}</div>` : ""}
    </div>
  `;
}

function bindUpdateStatusBar(): void {
  document.getElementById("update-toggle")?.addEventListener("click", () => {
    updatePopoverOpen = !updatePopoverOpen;
    render();
  });
  document.getElementById("update-github")?.addEventListener("click", () => {
    invoke("open_external", { url: RELEASES_URL }).catch((e) => console.error("open_external failed:", e));
  });
  document.getElementById("update-start")?.addEventListener("click", () => {
    void startUpgrade();
  });
  document.getElementById("update-restart")?.addEventListener("click", () => {
    relaunch().catch((e) => console.error("relaunch failed:", e));
  });
}

async function startUpgrade(): Promise<void> {
  if (!updateHandle) return;
  updateState = { ...updateState, phase: "downloading", downloaded: 0, total: 0, error: undefined };
  render();
  try {
    await updateHandle.downloadAndInstall((event) => {
      switch (event.event) {
        case "Started":
          updateState.total = event.data.contentLength ?? 0;
          updateState.downloaded = 0;
          break;
        case "Progress":
          updateState.downloaded = (updateState.downloaded ?? 0) + event.data.chunkLength;
          // Patch the readout in place so a chunky download doesn't trigger a
          // full re-render per chunk (mirrors the appendTaskEvent precedent).
          document.querySelectorAll("[data-update-progress]").forEach((el) => {
            el.textContent = formatUpdateProgress();
          });
          break;
        case "Finished":
          break;
      }
    });
    updateState = { ...updateState, phase: "ready" };
    render();
  } catch (e) {
    console.error("update install failed:", e);
    updateState = { ...updateState, phase: "error", error: `${e}` };
    render();
  }
}

async function scheduleUpdateCheck(): Promise<void> {
  // The updater only truly installs in a bundled build; skip the network check
  // in `tauri dev` so it never nags or errors during local development.
  if (import.meta.env.DEV) return;
  try {
    const update = await check();
    if (!update) return;
    updateHandle = update;
    updateState = {
      phase: "available",
      version: update.version,
      notes: update.body || undefined,
    };
    render();
  } catch (e) {
    // Update checks are best-effort; never block the app on a failed check.
    console.error("update check failed:", e);
  }
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
    if (updatePopoverOpen && !eventPathHasClass(event, "update-status")) {
      updatePopoverOpen = false;
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
  void scheduleUpdateCheck();

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
