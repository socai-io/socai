//! Tauri desktop entry — header status/configuration plus tools / agent panels.

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check, type Update } from "@tauri-apps/plugin-updater";

import { applyLanguageToDocument, t } from "./lib/i18n";
import { authMenu } from "./panels/auth";
import { agentPanel } from "./panels/tasks";
import { settingsMenu } from "./panels/settings";
import { subscriptionMenu } from "./panels/subscription";

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
      remote?: boolean;
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
  credential_preview?: string | null;
  is_default?: boolean;
  recommended?: boolean;
  source?: string | null;
}

export type AgentTaskStatus = "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted";

export interface AgentTaskSnapshot {
  task_id: string;
  task: string;
  provider: string | null;
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
  steps: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  cached_input_tokens: number | null;
  cache_creation_input_tokens: number | null;
  estimated_cost: number | null;
  cost_currency: string | null;
  points_used: number | null;
}

export interface TimelineEntity {
  type: string;
  data: unknown;
}

/** One media item of a note (SocaiV2 design contract). media[0] === cover. */
export interface NoteMedia {
  kind: "image" | "video";
  ratio?: string; // "3:4" | "1:1" | "9:16" | "16:9"
  src?: string; // downloaded file — absolute path, or media_dir-relative
  poster?: string; // video only — first-frame still (path)
  dur?: string; // video only — "0:48"
  status?: "loading" | "failed"; // background video-file download state
  error?: string; // local diagnostic; intentionally not rendered
  w?: number;
  h?: number;
}

/** One archived top comment on a note (replies flatten one level). */
export interface NoteComment {
  text: string;
  author?: string;
  likes?: number;
  time?: string;
  is_author?: boolean; // the note author replying in their own comments
  replies?: NoteComment[];
}

/** A note the agent saw/cited — one canonical object per note (the registry unit). */
export interface NoteData {
  note_id: string;
  url?: string;
  title?: string;
  content?: string; // full note body (excerpt is its first ~90 chars)
  excerpt?: string;
  author?: { name?: string; handle?: string; avatar?: string; url?: string };
  posted_at?: number; // epoch ms
  ip_location?: string; // author IP territory shown on the note ("广东")
  stats?: { likes?: number; collects?: number; comments?: number; shares?: number };
  comments?: NoteComment[]; // top comments captured with the read
  media?: NoteMedia[]; // media[0] === cover
  media_dir?: string; // run-relative folder, when src paths are relative
  transcript?: string; // video audio transcript (cloud ASR)
  saved?: boolean;
  // Tolerate extra fields the archive may carry.
  [key: string]: unknown;
}

export interface AgentTaskEventPayload {
  task_id: string;
  kind:
    | "queued"
    | "running"
    | "started"
    | "tab"
    | "step"
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
  step?: number;
  steps?: number;
  id?: string;
  sequence_in_step?: number;
  name?: string;
  label?: string;
  args?: unknown;
  repeat_count?: number;
  ok?: boolean;
  summary?: string;
  duration_ms?: number;
  input_tokens?: number | null;
  output_tokens?: number | null;
  cached_input_tokens?: number | null;
  cache_creation_input_tokens?: number | null;
  estimated_cost?: number | null;
  cost_currency?: string | null;
  points_used?: number | null;
  entities?: TimelineEntity[];
  error?: string | null;
  result_file?: string | null;
  run_id?: string | null;
  model?: string;
  task?: string;
  target_id?: string | null;
}

interface BackgroundMediaEvent {
  run_dir: string;
  note_id: string;
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
        <div class="topbar-left ${sidebarOpen ? "topbar-left--bordered" : ""}" data-tauri-drag-region>
          <button
            id="sidebar-toggle"
            type="button"
            class="icon-button"
            aria-label="${htmlEsc(t(sidebarOpen ? "sidebar.collapseAria" : "sidebar.expandAria"))}"
            aria-expanded="${sidebarOpen ? "true" : "false"}"
          >${PANEL_ICON_SVG}</button>
          ${renderUpdateChip()}
        </div>
        <div class="topbar-controls" data-tauri-drag-region>
          <div class="status-capsule" role="group" aria-label="${htmlEsc(t("status.capsuleAria"))}">
            ${connectionStatusBar()}
            <span class="status-capsule__divider" aria-hidden="true"></span>
            ${authMenu.render(
              agentPanel.currentModelLabel(),
              agentPanel.renderAccountConfig(),
              subscriptionMenu.render(),
            )}
          </div>
          ${settingsMenu.render(state)}
        </div>
      </header>
      <div class="body">
        ${sidebarOpen ? agentPanel.renderSidebar() : ""}
        <div class="workspace-stack">
          <main class="workspace-main">${agentPanel.renderWorkspace(state)}</main>
          ${agentPanel.renderWorkspaceOverlays()}
        </div>
      </div>
    </div>
  `;
  bindConnectionStatusBar();
  bindUpdateChip();
  bindSidebarToggle();
  agentPanel.bindHeader(state);
  authMenu.bind(
    state,
    () => {
      settingsMenu.closePopover();
    },
    async () => {
      await Promise.all([
        settingsMenu.loadConfig(),
        agentPanel.refreshModels(),
        subscriptionMenu.refresh(authMenu.isLoggedIn()),
      ]);
      if (authMenu.isLoggedIn()) await agentPanel.selectSocaiAgent();
    },
  );
  subscriptionMenu.bind(state, async () => {
    const alreadyHadPro = authMenu.hasProAccess();
    await authMenu.refreshWallet();
    if (!alreadyHadPro && authMenu.hasProAccess()) {
      await settingsMenu.selectRemoteForNewPro(state);
    }
  });
  settingsMenu.bind(
    state,
    () => {
      authMenu.closePopover();
    },
    async () => {
      await Promise.all([
        authMenu.refreshWallet(),
        subscriptionMenu.refresh(authMenu.isLoggedIn()),
      ]);
    },
  );
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
      ${connectionDetailsOpen ? renderConnectionDialog() : ""}
    </div>
  `;
}

function connectionBadge(): string {
  const expanded = connectionDetailsOpen ? "true" : "false";
  switch (status.state) {
    case "disconnected":
      return `<button id="chrome-status-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}" aria-label="${htmlEsc(t("chrome.statusToggleAria"))}"><i class="badge-dot badge-dot-muted" aria-hidden="true"></i>${htmlEsc(t("chrome.label"))} · ${htmlEsc(t("chrome.disconnected"))}</button>`;
    case "connecting":
      return `<button id="chrome-status-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}" aria-label="${htmlEsc(t("chrome.statusToggleAria"))}"><i class="badge-dot badge-dot-ink badge-dot-pulse" aria-hidden="true"></i>${htmlEsc(t("chrome.label"))} · ${htmlEsc(t("chrome.connecting"))}</button>`;
    case "connected":
      return `<button id="chrome-status-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}" aria-label="${htmlEsc(t("chrome.statusToggleAria"))}"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i>${htmlEsc(t("chrome.label"))} · ${htmlEsc(t("chrome.connected"))}</button>`;
  }
}

function renderConnectionDialog(): string {
  const remoteBlocked = settingsMenu.isRemoteSelected() && !authMenu.hasProAccess();
  const action = status.state === "connected"
    ? `<button id="chrome-disconnect" type="button" class="btn-ghost chrome-manager-action">${htmlEsc(t("chrome.disconnect"))}</button>`
    : status.state === "connecting"
      ? `<button type="button" class="btn-primary chrome-manager-action" disabled>${htmlEsc(t("chrome.connectingCta"))}</button>`
      : `<button id="chrome-connect-action" type="button" class="btn-primary chrome-manager-action" ${remoteBlocked || settingsMenu.isSaving() ? "disabled" : ""}>${htmlEsc(t("chrome.connectCta"))}</button>`;
  return `
    <div class="topbar-popover connection-dialog" role="dialog" aria-label="${htmlEsc(t("chrome.dialogAria"))}">
      ${settingsMenu.renderChromeManager()}
      ${action}
    </div>
  `;
}

function bindConnectionStatusBar(): void {
  document.getElementById("chrome-connect-action")?.addEventListener("click", () => {
    connectionDetailsOpen = false;
    invoke("cdp_connect").catch((e) => console.error("cdp_connect failed:", e));
  });
  document.getElementById("chrome-status-toggle")?.addEventListener("click", async (event) => {
    event.stopPropagation();
    const opening = !connectionDetailsOpen;
    if (opening) {
      settingsMenu.closePopover();
      authMenu.closePopover();
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
// In-app updater (macOS). Updates download + install in the background; while
// the download runs the chip shows a passive `downloading update…` state (so
// the user knows a new version exists). Once staged, the app relaunches
// automatically when no task is queued or running and socai is not focused;
// while focused, the clickable `restart to finish` chip remains available.
// Checks fire on launch, when the window returns to the foreground, and when a
// task finishes — throttled, and skipped while an update is already downloading
// or staged. The updater plugin is configured in tauri.conf.json; check() is
// gated off in dev because it only installs in bundled builds.
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
// Guard manual and automatic paths from racing to relaunch more than once.
let updateRelaunchStarted = false;
// Auto-relaunch only while another app has focus so returning to socai never
// looks like an unexpected crash. Manual restart remains available while focused.
let appWindowFocused = document.hasFocus();

// `downloading` renders a passive announcement, `ready` the restart action;
// errors reset to idle and retry quietly on the next check.
function renderUpdateChip(): string {
  if (updateState.phase === "downloading") {
    return `
    <div class="update-chip-wrap">
      <button type="button" class="update-chip" disabled role="status" aria-live="polite">
        <span class="update-chip-glyph" aria-hidden="true">↓</span>
        <span class="update-chip-label">${htmlEsc(t("update.downloading"))}</span>
      </button>
      ${updateTooltip()}
    </div>
  `;
  }
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

// Custom command, not the process plugin's relaunch(): that one respawns
// during event-loop teardown and on macOS the process dies before the new
// instance finishes spawning — the app quits and never reopens
// (tauri-apps/tauri#11392).
function doRelaunch(): void {
  if (updateRelaunchStarted) return;
  updateRelaunchStarted = true;
  invoke("app_relaunch").catch((e) => {
    updateRelaunchStarted = false;
    console.error("relaunch failed:", e);
  });
}

// Once the update is staged, take the same path as clicking the update chip when
// no task is queued or running and the user is in another app. Task completion
// and window blur both call this again when their respective gate changes.
function maybeRelaunchReadyUpdate(): void {
  if (updateState.phase !== "ready" || agentPanel.hasActiveTask() || appWindowFocused) return;
  doRelaunch();
}

// Download + install the update in the background. The chip announces the
// download as soon as it starts; once staged, relaunch while idle and unfocused,
// or wait for the active task to finish / user to switch away. A failure resets
// to idle so the next trigger retries.
async function startBackgroundUpgrade(): Promise<void> {
  if (!updateHandle) return;
  updateState = { ...updateState, phase: "downloading" };
  render();
  try {
    await updateHandle.downloadAndInstall();
    updateState = { ...updateState, phase: "ready" };
  } catch (e) {
    console.error("update download/install failed:", e);
    updateState = { phase: "idle" };
  }
  render();
  maybeRelaunchReadyUpdate();
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
    if (settingsMenu.isOpen() && !eventPathHasClass(event, "settings-menu") && settingsMenu.closePopover()) {
      changed = true;
    }
    if (authMenu.isOpen() && !eventPathHasClass(event, "auth-menu") && authMenu.closePopover()) {
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

// Tauri's webview neither follows target="_blank" nor hands external links to
// the OS browser, so clicking a web link (e.g. a note URL in a final answer)
// does nothing. Delegate every http(s) anchor click to the backend
// `open_external` command, which opens the system default browser. The raw
// attribute is checked (not `anchor.href`) so app-internal links — `note:`
// citations, `#` anchors — which the DOM would resolve against the app origin
// are left alone.
function bindExternalLinks(): void {
  document.addEventListener("click", (event) => {
    if (event.defaultPrevented) return;
    const anchor = event
      .composedPath()
      .find((item): item is HTMLAnchorElement => item instanceof HTMLAnchorElement);
    if (!anchor) return;
    const href = anchor.getAttribute("href") ?? "";
    if (!/^https?:\/\//i.test(href)) return;
    event.preventDefault();
    invoke("open_external", { url: href }).catch((e) => console.error("open_external failed:", e));
  });
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

  // Stream task-scoped agent events incrementally. Snapshot/status events and
  // run boundaries ask for a full render so the task list and conversation
  // turns update; normal stream rows append in place to preserve scroll.
  await listen<AgentTaskEventPayload>("agent_task:event", (event) => {
    if (agentPanel.appendTaskEvent(event.payload)) render();
    if (event.payload.kind === "tool_result") {
      // Each finished step's notes are on disk — refresh so the result row's
      // embed renders them now, not when the whole task ends.
      void agentPanel.loadTaskNotes(event.payload.task_id, shell());
    }
    if (isTaskFinishedEvent(event.payload)) {
      // A finished task is a good, quiet moment to check for an update, and the
      // point at which its full note archive (notes.json) is on disk to render.
      void maybeCheckForUpdate();
      void agentPanel.loadTaskNotes(event.payload.task_id, shell());
      maybeRelaunchReadyUpdate();
      void authMenu.refreshWallet().then(render);
    }
  });
  await listen<BackgroundMediaEvent>("agent_task:notes_updated", (event) => {
    agentPanel.handleBackgroundMediaUpdate(event.payload.run_dir, shell());
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
  await Promise.all([settingsMenu.loadConfig(), authMenu.loadSession()]);
  await subscriptionMenu.refresh(authMenu.isLoggedIn());
  render();
  bindGlobalDismiss();
  bindExternalLinks();
  installUpdatePreviewHook();
  void hydrateTaskEvents(initialTasks);
  void agentPanel.loadSelectedTaskNotes(shell());
  void maybeCheckForUpdate();

  const refresh = (): void => {
    invoke("cdp_refresh").catch((e) => console.error("cdp_refresh failed:", e));
    agentPanel.refreshModels()
      .then(() => render())
      .catch((e) => console.error("agent_list_models refresh failed:", e));
    // Foreground is the primary update-check trigger; the check throttles itself.
    void maybeCheckForUpdate();
  };
  window.addEventListener("focus", () => {
    appWindowFocused = true;
    refresh();
  });
  window.addEventListener("blur", () => {
    appWindowFocused = false;
    maybeRelaunchReadyUpdate();
  });
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

// Dev-only: simulate the update flow (downloading chip for 3s, then the
// restart chip) so both states and the running-task warning can be exercised
// without a bundled build + real feed. `__previewUpdate()` in the webview
// console starts it; reload to clear.
function installUpdatePreviewHook(): void {
  if (!import.meta.env.DEV) return;
  (window as Window & { __previewUpdate?: () => void }).__previewUpdate = () => {
    updateState = { phase: "downloading", version: "0.5.0" };
    render();
    window.setTimeout(() => {
      updateState = { phase: "ready", version: "0.5.0" };
      render();
    }, 3000);
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
