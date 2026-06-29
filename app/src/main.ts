//! Tauri desktop entry — header status/configuration plus tools / agent panels.

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

import { applyLanguageToDocument, formatTabs, t } from "./lib/i18n";
import { agentPanel } from "./panels/tasks";
import { settingsMenu } from "./panels/settings";

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

let status: Status = { state: "disconnected", reason: "starting" };
let connectionDetailsOpen = false;
// The sidebar (task history rail) starts expanded; the topbar toggle collapses it.
let sidebarOpen = true;

// Sidebar-collapse toggle glyph (mirrors the prototype's PanelIcon). On macOS
// the native traffic lights sit to its left; the .topbar-left gutter clears them.
const PANEL_ICON_SVG = `
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <rect x="3" y="4.5" width="18" height="15" rx="2.5"></rect>
    <line x1="9.5" y1="4.5" x2="9.5" y2="19.5"></line>
  </svg>
`;

function shell(): ShellState {
  return { status, rerender: render };
}

function render(): void {
  const root = document.getElementById("app");
  if (!root) return;
  const state = shell();
  root.innerHTML = `
    <div class="shell">
      <header class="topbar" data-tauri-drag-region>
        <div class="topbar-left ${sidebarOpen ? "topbar-left--bordered" : ""}">
          <button
            id="sidebar-toggle"
            type="button"
            class="icon-button"
            aria-label="${htmlEsc(t(sidebarOpen ? "sidebar.collapseAria" : "sidebar.expandAria"))}"
            aria-expanded="${sidebarOpen ? "true" : "false"}"
          >${PANEL_ICON_SVG}</button>
          ${renderUpdateChip()}
        </div>
        <div class="topbar-controls">
          <div class="status-capsule" role="group" aria-label="${htmlEsc(t("status.capsuleAria"))}">
            ${connectionStatusBar()}
            <span class="status-capsule__divider" aria-hidden="true"></span>
            ${agentPanel.renderHeader()}
          </div>
          ${settingsMenu.render(state)}
        </div>
      </header>
      <div class="body">
        ${sidebarOpen ? agentPanel.renderSidebar() : ""}
        <main class="workspace-main">${agentPanel.renderWorkspace(state)}</main>
      </div>
    </div>
  `;
  bindConnectionStatusBar();
  bindUpdateChip();
  bindSidebarToggle();
  agentPanel.bindHeader(state);
  settingsMenu.bind(state);
  agentPanel.bind(state);
}

function bindSidebarToggle(): void {
  document.getElementById("sidebar-toggle")?.addEventListener("click", () => {
    sidebarOpen = !sidebarOpen;
    render();
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
  document.getElementById("chrome-status-toggle")?.addEventListener("click", async (event) => {
    event.stopPropagation();
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
// In-app updater (macOS). Updates download + install silently in the background;
// the only thing the user ever sees or clicks is a `restart to finish` chip once
// an update is staged. Checks fire on launch, when the window returns to the
// foreground, and when a task finishes — throttled, and skipped while an update
// is already downloading or staged. The updater plugin is configured in
// tauri.conf.json; check() is gated off in dev because it only installs in
// bundled builds.
// ---------------------------------------------------------------------------

type UpdatePhase = "idle" | "downloading" | "ready";

interface UpdateState {
  phase: UpdatePhase;
  /** Latest available version — shown in the restart chip's hover tooltip. */
  version?: string;
}

let updateState: UpdateState = { phase: "idle" };
let updateHandle: Update | null = null;
// Installed app version — fetched once at startup for the chip's hover tooltip.
let currentVersion = "";
// Throttle so "check whenever the app foregrounds" can't hammer the endpoint.
let lastUpdateCheck = 0;
const UPDATE_CHECK_INTERVAL_MS = 30 * 60 * 1000;
// Inline warning shown when restart is clicked while a task is still running.
let restartWarn = false;

// Only the `ready` state renders — downloading is silent, errors retry quietly.
function renderUpdateChip(): string {
  if (updateState.phase !== "ready") return "";
  return `
    <div class="update-chip-wrap">
      <button id="update-chip" type="button" class="update-chip" aria-label="${htmlEsc(t("update.restartToUpdate"))}">
        <span class="update-chip-glyph" aria-hidden="true">↑</span>
        <span class="update-chip-label">${htmlEsc(t("update.chip"))}</span>
      </button>
      ${restartWarn ? renderRestartWarn() : updateTooltip()}
    </div>
  `;
}

function updateTooltip(): string {
  if (!currentVersion || !updateState.version) return "";
  return `
    <span class="update-tooltip" role="tooltip">
      <span class="update-tooltip-old">${htmlEsc(currentVersion)}</span>
      <span class="update-tooltip-arrow" aria-hidden="true">→</span>
      <span class="update-tooltip-new">${htmlEsc(updateState.version)}</span>
    </span>
  `;
}

function renderRestartWarn(): string {
  return `
    <div class="topbar-popover update-warn" role="dialog" aria-label="${htmlEsc(t("update.restartToUpdate"))}">
      <p class="t-small">${htmlEsc(t("update.taskRunningWarn"))}</p>
      <div class="update-warn-actions">
        <button id="update-restart-anyway" type="button" class="btn-primary btn-compact">${htmlEsc(t("update.restartAnyway"))}</button>
        <button id="update-restart-cancel" type="button" class="btn-ghost btn-compact">${htmlEsc(t("update.later"))}</button>
      </div>
    </div>
  `;
}

function bindUpdateChip(): void {
  document.getElementById("update-chip")?.addEventListener("click", (event) => {
    event.stopPropagation();
    // Relaunching interrupts any running task, so guard it: warn first with an
    // explicit escape hatch.
    if (agentPanel.hasActiveTask()) {
      restartWarn = true;
      render();
      return;
    }
    doRelaunch();
  });
  document.getElementById("update-restart-anyway")?.addEventListener("click", () => {
    restartWarn = false;
    doRelaunch();
  });
  document.getElementById("update-restart-cancel")?.addEventListener("click", () => {
    restartWarn = false;
    render();
  });
}

function doRelaunch(): void {
  relaunch().catch((e) => console.error("relaunch failed:", e));
}

// Download + install the staged update silently. Surfaces the restart chip only
// once it's installed; a failure resets to idle so the next trigger retries.
async function startBackgroundUpgrade(): Promise<void> {
  if (!updateHandle) return;
  updateState = { ...updateState, phase: "downloading" };
  try {
    await updateHandle.downloadAndInstall();
    updateState = { ...updateState, phase: "ready" };
    render();
  } catch (e) {
    console.error("update download/install failed:", e);
    updateState = { phase: "idle" };
  }
}

// Check for an update and, if found, kick off the silent background download.
// Triggered on launch, foreground, and task-finish; throttled, and skipped while
// an update is already downloading or staged.
async function maybeCheckForUpdate(): Promise<void> {
  // The updater only truly installs in a bundled build; skip the network check
  // in `tauri dev` so it never nags or errors during local development.
  if (import.meta.env.DEV) return;
  if (updateState.phase !== "idle") return;
  const now = Date.now();
  if (now - lastUpdateCheck < UPDATE_CHECK_INTERVAL_MS) return;
  lastUpdateCheck = now;
  try {
    const update = await check();
    if (!update) return;
    updateHandle = update;
    updateState = { phase: "idle", version: update.version };
    void startBackgroundUpgrade();
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
    if (!eventPathHasClass(event, "agent-status") && agentPanel.closeHeaderConfig()) {
      changed = true;
    }
    if (settingsMenu.isOpen() && !eventPathHasClass(event, "settings-menu") && settingsMenu.closePopover()) {
      changed = true;
    }
    if (restartWarn && !eventPathHasClass(event, "update-chip-wrap")) {
      restartWarn = false;
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
  // Overlay titlebar (see tauri.conf.json) floats the native macOS traffic
  // lights over our header; reserve their inset only on macOS.
  if (navigator.userAgent.includes("Macintosh")) {
    document.body.classList.add("is-macos");
    // In fullscreen macOS hides the traffic lights, so collapse the left gutter.
    const win = getCurrentWindow();
    const syncFullscreen = async (): Promise<void> => {
      try {
        document.body.classList.toggle("is-fullscreen", await win.isFullscreen());
      } catch (e) {
        console.error("isFullscreen failed:", e);
      }
    };
    void syncFullscreen();
    void win.onResized(() => void syncFullscreen());
  }

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
    // A finished task is a good, quiet moment to check for an update.
    if (isTaskFinishedEvent(event.payload)) void maybeCheckForUpdate();
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
  try {
    currentVersion = await getVersion();
  } catch (e) {
    console.error("getVersion failed:", e);
  }
  await settingsMenu.loadConfig();
  render();
  bindGlobalDismiss();
  installUpdatePreviewHook();
  void hydrateTaskEvents(initialTasks);
  void maybeCheckForUpdate();

  const refresh = (): void => {
    invoke("cdp_refresh").catch((e) => console.error("cdp_refresh failed:", e));
    agentPanel.refreshModels()
      .then(() => render())
      .catch((e) => console.error("agent_list_models refresh failed:", e));
    // Foreground is the primary update-check trigger; the check throttles itself.
    void maybeCheckForUpdate();
  };
  window.addEventListener("focus", refresh);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") refresh();
  });
}

const TERMINAL_TASK_EVENT_KINDS = new Set<AgentTaskEventPayload["kind"]>([
  "done",
  "completed",
  "failed",
  "cancelled",
  "interrupted",
]);

function isTaskFinishedEvent(payload: AgentTaskEventPayload): boolean {
  if (TERMINAL_TASK_EVENT_KINDS.has(payload.kind)) return true;
  const status = payload.snapshot?.status;
  return status === "completed" || status === "failed" || status === "cancelled" || status === "interrupted";
}

// Dev-only: force the `restart to finish` chip so its appearance and the
// running-task warning can be exercised without a bundled build + real feed.
// `__previewUpdate()` in the webview console shows the chip; reload to clear.
function installUpdatePreviewHook(): void {
  if (!import.meta.env.DEV) return;
  (window as Window & { __previewUpdate?: () => void }).__previewUpdate = () => {
    updateState = { phase: "ready", version: "0.5.0" };
    render();
  };
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
