//! Task workspace coordinator: shared state, agent configuration popover,
//! task event intake, and bindings. The sidebar (history list) + delete dialog
//! live in `task_history.ts`; the conversation view lives in `conversation.ts`.

import { invoke } from "@tauri-apps/api/core";
import type {
  AgentTaskEventPayload,
  AgentTaskSnapshot,
  AgentTaskStatus,
  ModelInfo,
  NoteData,
  ShellState,
} from "../main";
import { esc } from "../lib/html";
import { t } from "../lib/i18n";
import { isSendShortcut } from "../lib/shortcuts";
import { settingsMenu } from "./settings";
import { renderConfirmDeleteDialog, renderSidebar as renderSidebarMarkup } from "./task_history";
import {
  noteRefsFromEvent,
  pendingSearchQuery,
  answerTextForTurn,
  liveActivityMetricsText,
  renderComposePane,
  renderConversation,
  renderEventRow,
  renderLiveNotesGroup,
  renderSearchGroupForEvent,
} from "./conversation";
import type { ComposerProps } from "./conversation";
import { bindNoteInteractions, setNoteRegistry } from "./notes";
import {
  bindFeishuConnector,
  renderFeishuConnector,
} from "../connectors/feishu";

// The workspace is one conversation pane: the compose view (default / "new
// task") is the empty thread + composer; picking a task shows its thread with
// the same composer in reply mode. The sidebar history list is always present.
type WorkspaceView = "compose" | "detail";
export type AgentTaskView = AgentTaskSnapshot & {
  events: AgentTaskEventPayload[];
  notes?: NoteData[];
};
type CodexLoginStart = { message: string };

// ── Agent task workspace ──────────────────────────────────────────────────

export namespace agentPanel {
  let view: WorkspaceView = "compose";
  let draft = "";
  let model = "";
  let modelProvider = "";
  let modelByProvider = new Map<string, string>();
  let submittingTask = false;
  let submitError = "";
  let replyDraft = "";
  let submittingReply = false;
  let replyError = "";
  let tasks: AgentTaskView[] = [];
  let pendingEvents = new Map<string, AgentTaskEventPayload[]>();
  let selectedTaskId: string | null = null;
  // Full shell renders replace the sidebar DOM. Keep its viewport stable when
  // selecting a task (and across any other render) instead of jumping to top.
  let sidebarScrollTop = 0;
  // Confirm-first delete: every affordance (row ×, conversation-head button)
  // opens the centered dialog by setting this; the delete only runs on confirm.
  let deleteRequestTaskId: string | null = null;
  let modelsCache: ModelInfo[] = [];
  let remoteDebuggingReady = false;
  let chromeSetupPollTimer: number | null = null;
  let chromeSetupPollInFlight = false;
  let chromeSetupStatus: ShellState["status"] = { state: "disconnected", reason: "starting" };

  // Explicit activity-fold choices, keyed `${taskId}#${turnIndex}`. Absent =
  // the default (open while that turn's run streams, folded otherwise). The
  // terminal transition clears a task's entries so the fold lands with the
  // answer even if the user toggled mid-run.
  const activityOpen = new Map<string, boolean>();

  function isActivityOpen(taskId: string, turnIndex: number, defaultOpen: boolean): boolean {
    return activityOpen.get(`${taskId}#${turnIndex}`) ?? defaultOpen;
  }

  // Key-entry sub-state — used by the header configuration popover.
  let pendingKey = "";
  let savingKey = false;
  let codexStarting = false;
  let keyMessage = "";
  let keyError = "";
  let configOpen = false;
  // Shows the key-entry form for a provider that already has a credential.
  let editingKey = false;

  export function setModels(models: ModelInfo[]): void {
    modelsCache = models;
    rememberConfiguredModels(models);
    const current = models.find((m) => sameModel(m, modelProvider, model));
    if (current) {
      modelProvider = current.provider;
      return;
    }

    // Restore the persisted choice (`is_default`); otherwise fall back to the
    // first provider that has a key.
    const persisted = models.find((m) => m.is_default);
    const withKey = models.find((m) => m.has_key);
    selectModelInfo(persisted ?? withKey ?? models[0]);
  }

  export async function refreshModels(): Promise<ModelInfo[]> {
    const models = await invoke<ModelInfo[]>("agent_list_models");
    setModels(models);
    return models;
  }

  export async function selectSocaiAgent(): Promise<void> {
    const picked = modelsCache.find((item) => item.provider === "socai" && item.recommended)
      ?? modelsCache.find((item) => item.provider === "socai");
    if (!picked) return;
    selectModelInfo(picked);
    await persistModelChoice(picked);
  }

  export function setTasks(snapshots: AgentTaskSnapshot[]): void {
    const existingById = new Map(tasks.map((task) => [task.task_id, task]));
    const snapshotIds = new Set(snapshots.map((snapshot) => snapshot.task_id));
    const hydrated = snapshots.map((snapshot) => {
      const existing = existingById.get(snapshot.task_id);
      const pending = pendingEvents.get(snapshot.task_id) ?? [];
      pendingEvents.delete(snapshot.task_id);
      const merged = existing ? mergeSnapshot(existing, snapshot) : snapshot;
      return { ...merged, events: mergeEvents(existing?.events ?? [], pending) };
    });
    const liveOnly = tasks.filter((task) => !snapshotIds.has(task.task_id));
    tasks = [...hydrated, ...liveOnly];
    if (!selectedTaskId && tasks.length > 0) {
      selectedTaskId = newestTask(tasks)?.task_id ?? null;
    }
  }

  export function setTaskEvents(taskId: string, events: AgentTaskEventPayload[]): boolean {
    if (events.length === 0) return false;
    const task = tasks.find((item) => item.task_id === taskId);
    if (!task) {
      pendingEvents.set(taskId, mergeEvents(pendingEvents.get(taskId) ?? [], events));
      return false;
    }
    const before = task.events.length;
    task.events = mergeEvents(task.events, events);
    return task.events.length !== before && taskId === selectedTaskId;
  }

  export function setTaskNotes(taskId: string, notes: NoteData[]): boolean {
    const task = tasks.find((item) => item.task_id === taskId);
    if (!task) return false;
    task.notes = notes;
    return taskId === selectedTaskId;
  }

  // Fetch a task's note archive (notes.json) and re-render when it changed the
  // currently-selected task. Best-effort: a run simply may have no notes.
  export async function loadTaskNotes(taskId: string, shell: ShellState): Promise<void> {
    try {
      const notes = await invoke<NoteData[]>("agent_task_notes", { taskId });
      if (setTaskNotes(taskId, notes)) shell.rerender();
    } catch (e) {
      console.error("agent_task_notes failed:", e);
    }
  }

  export function loadSelectedTaskNotes(shell: ShellState): void {
    const selected = selectedTask();
    if (selected) void loadTaskNotes(selected.task_id, shell);
  }

  const backgroundMediaRefreshTimers = new Map<string, number>();

  /** A background video finished after its tool (or whole agent run) returned.
   *  Coalesce concurrent completions, then reload the matching task so its
   *  poster-only card upgrades to a playable video without user action. */
  export function handleBackgroundMediaUpdate(runDir: string, shell: ShellState): void {
    const task = tasks.find((item) => item.run_dir === runDir);
    if (!task) return;
    const existing = backgroundMediaRefreshTimers.get(runDir);
    if (existing !== undefined) window.clearTimeout(existing);
    const timer = window.setTimeout(() => {
      backgroundMediaRefreshTimers.delete(runDir);
      void loadTaskNotes(task.task_id, shell);
    }, 350);
    backgroundMediaRefreshTimers.set(runDir, timer);
  }

  // ── live notes while a task runs ─────────────────────────────────────
  // notes.json is written incrementally (one rewrite per note read), but no
  // event fires mid-tool-call — a scan is silent for minutes. Poll the archive
  // while the selected task runs so each note's card appears right after its
  // media lands, in a "live strip" pinned to the bottom of the stream. Rows
  // for finished steps render their own embeds from tool_result entities, so
  // the strip only shows notes no result row has claimed yet.
  let notesPollTimer: number | null = null;

  function syncNotesPolling(shell: ShellState): void {
    const selected = selectedTask();
    const running = !!selected && (selected.status === "running" || selected.status === "queued");
    if (running && notesPollTimer === null) {
      notesPollTimer = window.setInterval(() => void pollLiveNotes(shell), 2000);
    } else if (!running && notesPollTimer !== null) {
      window.clearInterval(notesPollTimer);
      notesPollTimer = null;
    }
  }

  async function pollLiveNotes(shell: ShellState): Promise<void> {
    const task = selectedTask();
    if (!task || (task.status !== "running" && task.status !== "queued")) {
      syncNotesPolling(shell);
      return;
    }
    // Refresh the snapshot too: tokens/steps accumulate into run.json after
    // every LLM step, but no agent event carries them mid-run — this keeps the
    // state fresh so the running activity summary and eventual answer meta
    // show the current run's own figures without replacing earlier turns.
    try {
      upsertTask(await invoke<AgentTaskSnapshot>("agent_task_get", { taskId: task.task_id }));
      updateLiveTaskMetrics(task);
    } catch (e) {
      console.error("agent_task_get poll failed:", e);
    }
    try {
      const notes = await invoke<NoteData[]>("agent_task_notes", { taskId: task.task_id });
      // Within a run records only accumulate; no growth means nothing to do.
      if (notes.length <= (task.notes?.length ?? 0)) return;
      task.notes = notes;
      setNoteRegistry(notes, task.run_dir);
      updateLiveStrip(task);
    } catch (e) {
      console.error("agent_task_notes poll failed:", e);
    }
  }

  function updateLiveTaskMetrics(task: AgentTaskView): void {
    const stream = document.querySelector<HTMLDivElement>(`[data-agent-events="${task.task_id}"]`);
    const metrics = stream?.querySelector<HTMLElement>(".thread-inner > .turn:last-child [data-turn-metrics]");
    if (metrics) metrics.textContent = liveActivityMetricsText(task);
  }

  let liveStripKey = "";

  // Per-task scroll memory for the conversation thread. A full render rebuilds
  // the thread DOM (dropping its scroll offset), so a scroll listener records
  // the position here and the post-render bind restores it. No entry, or a
  // pinned entry, means auto-follow: keep the thread glued to its newest row.
  const streamScroll = new Map<string, { top: number; pinned: boolean }>();

  function isPinnedToBottom(stream: HTMLDivElement): boolean {
    return stream.scrollTop + stream.clientHeight >= stream.scrollHeight - 8;
  }

  // The last turn's activity-notes container — where streamed note groups and
  // the live strip land. Created on demand: a turn renders without one until
  // its first search result arrives.
  function lastTurnNotesContainer(stream: HTMLDivElement): HTMLDivElement | null {
    const turn = stream.querySelector<HTMLDivElement>(".thread-inner > .turn:last-child");
    const wrap = turn?.querySelector<HTMLDivElement>(".activity-wrap");
    if (!wrap) return null;
    let notes = wrap.querySelector<HTMLDivElement>(".activity-notes");
    if (!notes) {
      wrap.insertAdjacentHTML("beforeend", `<div class="activity-notes"></div>`);
      notes = wrap.querySelector<HTMLDivElement>(".activity-notes");
    }
    return notes;
  }

  function updateLiveStrip(task: AgentTaskView): void {
    const stream = document.querySelector<HTMLDivElement>(`[data-agent-events="${task.task_id}"]`);
    if (!stream) return;
    const claimed = new Set<string>();
    for (const ev of task.events) {
      if (ev.kind !== "tool_result") continue;
      for (const ref of noteRefsFromEvent(ev)) claimed.add(ref);
    }
    const refs = (task.notes ?? []).map((n) => n.note_id).filter((id) => !claimed.has(id));
    // The in-flight search's query heads the strip (the finished result's
    // group will carry the same header once it lands).
    const query = pendingSearchQuery(task);
    // Keyed per task so switching tasks can't suppress another task's strip.
    const key = `${task.task_id}|${query ?? "∅"}|${refs.join(",")}`;
    const existing = stream.querySelector("[data-live-strip]");
    if (key === liveStripKey && existing) return;
    liveStripKey = key;
    // Capture auto-follow BEFORE touching the DOM — removing the old strip or
    // creating the notes container already shifts scrollHeight, which would
    // read as "the user scrolled away" and strand the thread mid-scroll.
    const pinned = isPinnedToBottom(stream);
    existing?.remove();
    if (refs.length === 0) return;
    const html = renderLiveNotesGroup(refs, query);
    if (!html) return;
    const notes = lastTurnNotesContainer(stream);
    if (!notes) return;
    notes.insertAdjacentHTML("beforeend", html);
    if (pinned) stream.scrollTop = stream.scrollHeight;
  }

  // Whether any task is running or queued — used to guard the app restart.
  export function hasActiveTask(): boolean {
    return tasks.some((task) => task.status === "running" || task.status === "queued");
  }

  export function renderHeader(): string {
    const showConfig = configOpen || selectedNeedsKey();
    return `
      <div class="agent-status">
        ${renderAgentBadge()}
        ${showConfig ? renderConfigPopover() : ""}
      </div>
    `;
  }

  export function bindHeader(shell: ShellState): void {
    document.getElementById("agent-config-toggle")?.addEventListener("click", (event) => {
      event.stopPropagation();
      configOpen = selectedNeedsKey() ? true : !configOpen;
      if (!configOpen) editingKey = false;
      shell.rerender();
    });

    document.getElementById("agent-header-key-edit")?.addEventListener("click", () => {
      editingKey = true;
      pendingKey = "";
      keyMessage = "";
      keyError = "";
      shell.rerender();
    });

    document.getElementById("agent-header-key-cancel")?.addEventListener("click", () => {
      editingKey = false;
      pendingKey = "";
      keyMessage = "";
      keyError = "";
      shell.rerender();
    });

    document.querySelectorAll<HTMLButtonElement>(".agent-provider-option").forEach((opt) => {
      opt.addEventListener("click", () => {
        const nextProvider = opt.dataset.provider;
        if (!nextProvider || nextProvider === modelProvider) return;
        keyMessage = "";
        keyError = "";
        pendingKey = "";
        editingKey = false;
        const picked = preferredModelForProvider(nextProvider);
        selectModelInfo(picked);
        if (picked) persistModelChoice(picked);
        configOpen = true;
        shell.rerender();
      });
    });

    const modelSelect = document.getElementById("agent-model-select") as HTMLSelectElement | null;
    modelSelect?.addEventListener("change", () => {
      const next = modelSelect.value;
      const nextProvider = modelSelect.dataset.provider;
      if (!next || !nextProvider || (next === model && nextProvider === modelProvider)) return;
      keyMessage = "";
      keyError = "";
      pendingKey = "";
      editingKey = false;
      const picked = modelsCache.find((m) => sameModel(m, nextProvider, next));
      selectModelInfo(picked);
      if (picked) persistModelChoice(picked);
      configOpen = true;
      shell.rerender();
    });

    const keyInput = document.getElementById("agent-header-key-input") as HTMLInputElement | null;
    keyInput?.addEventListener("input", () => {
      pendingKey = keyInput.value;
      keyMessage = "";
      keyError = "";
    });

    const saveBtn = document.getElementById("agent-header-key-save") as HTMLButtonElement | null;
    saveBtn?.addEventListener("click", async () => {
      const provider = saveBtn.dataset.provider;
      const key = pendingKey.trim();
      if (!provider || !key || savingKey) return;
      savingKey = true;
      keyMessage = "";
      keyError = "";
      shell.rerender();
      try {
        await invoke("agent_save_api_key", { provider, apiKey: key });
        setModels(await invoke<ModelInfo[]>("agent_list_models"));
        pendingKey = "";
        editingKey = false;
      } catch (err) {
        keyError = `${err}`;
      } finally {
        savingKey = false;
        shell.rerender();
      }
    });

    document.getElementById("agent-header-codex-login")?.addEventListener("click", async () => {
      if (codexStarting) return;
      codexStarting = true;
      keyMessage = "";
      keyError = "";
      shell.rerender();
      try {
        const login = await invoke<CodexLoginStart>("agent_open_codex_login");
        keyMessage = login.message;
        codexStarting = false;
        shell.rerender();
        void pollCodexOAuth(shell);
      } catch (err) {
        keyError = `${err}`;
        codexStarting = false;
        shell.rerender();
      }
    });

  }

  export function closeHeaderConfig(): boolean {
    if (selectedNeedsKey()) return false;
    if (!configOpen) return false;
    configOpen = false;
    editingKey = false;
    return true;
  }

  export function currentModelLabel(): string {
    const selected = selectedModel();
    if (!selected) return t("agent.label");
    return selected.provider === "socai"
      ? providerDisplayLabel(selected)
      : modelDisplayLabel(selected);
  }

  export function renderAccountConfig(): string {
    return `<div class="agent-account-config">${renderConfigContent()}</div>`;
  }

  function renderAgentBadge(): string {
    const selected = selectedModel();
    const expanded = configOpen || selectedNeedsKey() ? "true" : "false";
    if (!selected) {
      return `<button id="agent-config-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}"><i class="badge-dot badge-dot-muted" aria-hidden="true"></i><span class="badge-text">${esc(t("agent.label"))} · ${esc(t("agent.loading"))}</span></button>`;
    }
    const label = selected.provider === "socai"
      ? providerDisplayLabel(selected)
      : modelDisplayLabel(selected);
    if (!selected.has_key) {
      return `<button id="agent-config-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}"><i class="badge-dot badge-dot-hollow" aria-hidden="true"></i><span class="badge-text">${esc(t("agent.label"))} · ${esc(label)} · ${esc(t("agent.keyNeeded"))}</span></button>`;
    }
    return `<button id="agent-config-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i><span class="badge-text">${esc(t("agent.label"))} · ${esc(label)}</span></button>`;
  }

  function renderConfigPopover(): string {
    return `
      <div class="topbar-popover agent-config-popover" role="dialog" aria-label="${esc(t("agent.configurationAria"))}">
        ${renderConfigContent()}
      </div>
    `;
  }

  function renderConfigContent(): string {
    const selected = selectedModel();
    const disabled = savingKey || submittingTask;
    const activeProvider = modelProvider || selected?.provider || providerSummaries()[0]?.provider || "";
    const activeModel = selected?.provider === activeProvider ? selected : preferredModelForProvider(activeProvider);
    const selectedModelId = modelId(activeModel);
    const providerOptions = providerSummaries()
      .map((provider) => {
        const active = provider.provider === activeProvider;
        const dotClass = active ? "badge-dot-ink" : "badge-dot-hollow";
        const flag = provider.hasKey ? "" : `<span class="t-small subtle">${esc(t("agent.keyNeeded"))}</span>`;
        const selectedForProvider = preferredModelForProvider(provider.provider);
        const hint = provider.provider === "socai"
          ? t("agent.managedModel")
          : selectedForProvider ? modelNameLabel(selectedForProvider) : t("common.loading");
        return `
          <button
            type="button"
            class="agent-provider-option${active ? " is-active" : ""}"
            data-provider="${esc(provider.provider)}"
            role="option"
            aria-selected="${active ? "true" : "false"}"
            ${disabled ? "disabled" : ""}
          >
            <i class="badge-dot ${dotClass}" aria-hidden="true"></i>
            <span class="agent-model-copy">
              <span class="agent-model-name">${esc(provider.displayName)}</span>
              <span class="agent-model-id">${esc(hint)}</span>
            </span>
            ${flag}
          </button>
        `;
      })
      .join("");

    const modelRows = modelsForProvider(activeProvider);
    const modelOptions = modelRows
      .map((m) => {
        const id = modelId(m);
        return `<option value="${esc(id)}" ${id === selectedModelId ? "selected" : ""}>${esc(modelOptionLabel(m))}</option>`;
      })
      .join("");

    return `
      <section class="agent-config-field">
        <div class="agent-model-list agent-provider-list" role="listbox" aria-label="${esc(t("agent.selectProviderAria"))}">
          ${providerOptions || `<p class="t-small subtle agent-picker-empty">${esc(t("common.loading"))}</p>`}
        </div>
      </section>
      ${renderCredentialSection(activeModel)}
      ${activeProvider === "socai" ? "" : `<section class="agent-config-field">
        <label class="t-eyebrow agent-config-title" for="agent-model-select">${esc(t("agent.modelVersion"))}</label>
        <select
          id="agent-model-select"
          class="input-field agent-model-select"
          data-provider="${esc(activeProvider)}"
          aria-label="${esc(t("agent.selectModelAria"))}"
          ${disabled || modelRows.length === 0 ? "disabled" : ""}
        >
          ${modelOptions}
        </select>
      </section>`}
    `;
  }

  function renderCredentialSection(selected: ModelInfo | undefined): string {
    if (!selected) return "";
    if (selected.provider === "socai") return "";
    if (selected.has_key && !editingKey) return renderCredentialConfigured(selected);
    return renderHeaderKeyEntry(selected);
  }

  function renderCredentialConfigured(selected: ModelInfo): string {
    return `
      <div class="agent-config-key agent-config-key-ready">
        <p class="t-eyebrow agent-config-title">${esc(t("agent.apiKey"))}</p>
        <p class="t-small subtle">${esc(
          selected.credential_kind === "codex_oauth"
            ? t("agent.chatgptConnected")
            : t("agent.credentialPreview", {
                preview: selected.credential_preview || t("agent.apiKey"),
              }),
        )}</p>
        <div class="agent-config-actions">
          <button id="agent-header-key-edit" type="button" class="btn-ghost btn-compact" ${savingKey || submittingTask ? "disabled" : ""}>
            ${esc(t("agent.updateCredential"))}
          </button>
        </div>
      </div>
    `;
  }

  function renderHeaderKeyEntry(selected: ModelInfo): string {
    const openai = selected.provider === "openai";
    return `
      <div class="agent-config-key">
        <p class="t-eyebrow agent-config-title">${esc(t("agent.apiKey"))}</p>
        <p class="t-small subtle">${esc(
          selected.has_key
            ? t("agent.replaceCredential", { provider: providerDisplayLabel(selected) })
            : t("agent.needsCredential", { model: providerDisplayLabel(selected) }),
        )}</p>
        ${openai ? `
          <div class="agent-config-actions">
            <button id="agent-header-codex-login" type="button" class="btn-primary btn-compact" ${codexStarting ? "disabled" : ""}>
              ${codexStarting ? esc(t("agent.opening")) : esc(t("agent.connectChatgpt"))}
            </button>
          </div>
        ` : ""}
        ${openai ? `<p class="t-small subtle">${esc(t("common.or"))}</p>` : ""}
        <div class="agent-config-key-row">
          <input
            id="agent-header-key-input"
            class="input-field"
            type="password"
            placeholder="${esc(t("agent.pasteApiKey"))}"
            value="${esc(pendingKey)}"
            autocomplete="off"
            ${savingKey ? "disabled" : ""}
          />
          <button id="agent-header-key-save" type="button" data-provider="${esc(selected.provider)}" class="btn-primary btn-compact" ${savingKey ? "disabled" : ""}>
            ${savingKey ? esc(t("common.saving")) : esc(t("common.save"))}
          </button>
          ${selected.has_key ? `
            <button id="agent-header-key-cancel" type="button" class="btn-ghost btn-compact" ${savingKey ? "disabled" : ""}>
              ${esc(t("common.cancel"))}
            </button>
          ` : ""}
        </div>
        ${keyMessage ? `<p class="t-small subtle">${esc(keyMessage)}</p>` : ""}
        ${keyError ? `<p class="t-small result-error">${esc(keyError)}</p>` : ""}
      </div>
    `;
  }

  function modelId(info: ModelInfo | undefined): string {
    return info?.model_id || info?.default_model || "";
  }

  function sameModel(info: ModelInfo, provider: string, id: string): boolean {
    return info.provider === provider && modelId(info) === id;
  }

  function rememberConfiguredModels(models: ModelInfo[]): void {
    for (const info of models) {
      if (!info.provider || modelByProvider.has(info.provider)) continue;
      const preferred = models.find((m) => m.provider === info.provider && modelId(m) === m.selected_model)
        ?? models.find((m) => m.provider === info.provider && m.recommended)
        ?? info;
      const id = modelId(preferred);
      if (id) modelByProvider.set(info.provider, id);
    }
  }

  function selectModelInfo(info: ModelInfo | undefined): void {
    model = modelId(info);
    modelProvider = info?.provider || "";
    if (info && model) modelByProvider.set(info.provider, model);
  }

  function persistModelChoice(info: ModelInfo): Promise<void> {
    const id = modelId(info);
    if (!id) return Promise.resolve();
    return invoke<void>("agent_set_default_model", { provider: info.provider, model: id }).catch(
      (err) => console.error("agent_set_default_model failed:", err),
    );
  }

  function providerSummaries(): Array<{ provider: string; displayName: string; hasKey: boolean }> {
    const seen = new Set<string>();
    const providers: Array<{ provider: string; displayName: string; hasKey: boolean }> = [];
    for (const info of modelsCache) {
      if (seen.has(info.provider)) continue;
      seen.add(info.provider);
      providers.push({
        provider: info.provider,
        displayName: providerDisplayLabel(info),
        hasKey: info.has_key,
      });
    }
    return providers;
  }

  function modelsForProvider(provider: string): ModelInfo[] {
    return modelsCache.filter((m) => m.provider === provider);
  }

  function preferredModelForProvider(provider: string): ModelInfo | undefined {
    const rows = modelsForProvider(provider);
    const remembered = modelByProvider.get(provider);
    return rows.find((m) => modelId(m) === remembered)
      ?? rows.find((m) => modelId(m) === m.selected_model)
      ?? rows.find((m) => m.recommended)
      ?? rows[0];
  }

  function providerDisplayLabel(info: ModelInfo): string {
    return info.provider_display_name || info.provider;
  }

  function modelNameLabel(info: ModelInfo): string {
    return info.display_name || modelId(info);
  }

  function modelOptionLabel(info: ModelInfo): string {
    const name = modelNameLabel(info);
    const recommended = info.recommended ? ` (${t("agent.defaultModel")})` : "";
    return `${name}${recommended}`;
  }

  function modelDisplayLabel(info: ModelInfo): string {
    const provider = providerDisplayLabel(info);
    const name = modelNameLabel(info);
    return name.startsWith(provider) ? name : `${provider} · ${name}`;
  }

  function selectedModel(): ModelInfo | undefined {
    return modelsCache.find((m) => sameModel(m, modelProvider, model));
  }

  function selectedNeedsKey(): boolean {
    const selected = selectedModel();
    return !!selected && !selected.has_key;
  }

  // Append a streamed event and update state. Returns true when the shell
  // should re-render: a snapshot/status changed, or the event opens a run
  // boundary ("queued"/"started" start a new conversation turn — rebuilt by a
  // full render rather than patched in place).
  export function appendTaskEvent(payload: AgentTaskEventPayload): boolean {
    if (payload.snapshot) upsertTask(payload.snapshot);
    const boundary = payload.kind === "queued" || payload.kind === "started";

    const task = tasks.find((item) => item.task_id === payload.task_id);
    if (!task) {
      stashPendingEvent(payload);
      return !!payload.snapshot;
    }

    if (payload.text.trim()) {
      const added = appendUniqueEvent(task, payload);
      if (added && !boundary) appendEventRowIfSelected(payload);
    }

    return !!payload.snapshot || boundary;
  }

  // The persistent left rail: "new task" + the history list. Always rendered
  // (when the sidebar is expanded); independent of which workspace view is up.
  export function renderSidebar(): string {
    return renderSidebarMarkup({
      tasks,
      selectedTaskId,
      composing: view === "compose",
    });
  }

  // The main pane: the centered new-task compose (hero + chat composer,
  // masked by the connect overlay while chrome is down), or the selected
  // task's conversation with the same composer in "reply" mode.
  export function renderWorkspace(shell: ShellState): string {
    const task = view === "compose" ? null : selectedTask() ?? null;
    if (!task) return renderComposePane(newComposer(shell));
    // Point the note UI at this task's archive; the thread's card groups and
    // answer citations resolve refs against it, and so does the viewer.
    setNoteRegistry(task.notes, task.run_dir);
    const running = task.status === "running" || task.status === "queued";
    return renderConversation({
      task,
      running,
      isActivityOpen: (turnIndex, defaultOpen) => isActivityOpen(task.task_id, turnIndex, defaultOpen),
      composer: replyComposer(shell, running),
    });
  }

  // Dialogs live in a sibling layer above the right-side task view, rather
  // than inside its clipped content tree. This keeps their centering relative
  // to the workspace (not the whole window/sidebar) without relying on WebKit
  // fixed-position behavior under an overflow:hidden ancestor.
  export function renderWorkspaceOverlays(): string {
    const deleteRequest = tasks.find((task) => task.task_id === deleteRequestTaskId);
    const dialog = deleteRequest ? renderConfirmDeleteDialog(deleteRequest) : "";
    return `<div class="workspace-overlay-root">${dialog}${renderFeishuConnector(tasks)}</div>`;
  }

  function newComposer(shell: ShellState): ComposerProps {
    const selected = selectedModel();
    return {
      mode: "new",
      value: draft,
      submitting: submittingTask,
      error: submitError,
      status: shell.status,
      modelReady: !!selected && selected.has_key,
      running: false,
      remoteProfile: settingsMenu.isRemoteProfile(),
      remoteDebuggingReady,
    };
  }

  function replyComposer(shell: ShellState, running: boolean): ComposerProps {
    return {
      mode: "reply",
      value: replyDraft,
      submitting: submittingReply,
      error: replyError,
      status: shell.status,
      modelReady: true,
      running,
      remoteProfile: settingsMenu.isRemoteProfile(),
      remoteDebuggingReady,
    };
  }

  export function bind(shell: ShellState): void {
    restoreSidebarScroll();
    bindFeishuConnector(shell, (taskId, turnIndex) => {
      const task = tasks.find((item) => item.task_id === taskId);
      return task ? answerTextForTurn(task, turnIndex) : null;
    });
    document.getElementById("sidebar-new")?.addEventListener("click", () => {
      view = "compose";
      shell.rerender();
    });

    bindComposer(shell);

    // Fold/unfold a turn's activity detail. The explicit choice is remembered
    // per task+turn until the run's terminal transition clears it.
    document.querySelectorAll<HTMLButtonElement>("[data-activity-turn]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const taskId = selectedTaskId;
        const turn = btn.dataset.activityTurn;
        if (!taskId || turn === undefined) return;
        activityOpen.set(`${taskId}#${turn}`, !btn.classList.contains("is-open"));
        shell.rerender();
      });
    });

    // The row's open control is a native <button>, so click/Enter/Space are
    // handled for free — no hand-bound keydown, no role/tabindex.
    document.querySelectorAll<HTMLButtonElement>("[data-task-id]").forEach((btn) => {
      btn.addEventListener("click", () => {
        selectedTaskId = btn.dataset.taskId ?? null;
        view = "detail";
        // A stale draft/error from whatever task was open before shouldn't
        // leak into this one's reply box.
        replyDraft = "";
        replyError = "";
        shell.rerender();
        if (selectedTaskId) void loadTaskNotes(selectedTaskId, shell);
      });
    });
    // Note interactions (viewer, card carousel, citation hover, external links)
    // are wired once via delegation on document.
    bindNoteInteractions();
    document.querySelectorAll<HTMLButtonElement>("[data-cancel-task]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const taskId = btn.dataset.cancelTask;
        if (!taskId) return;
        btn.disabled = true;
        try {
          const snapshot = await invoke<AgentTaskSnapshot>("agent_task_cancel", { taskId });
          upsertTask(snapshot);
        } catch (err) {
          submitError = `${err}`;
        } finally {
          shell.rerender();
        }
      });
    });
    document.querySelectorAll<HTMLButtonElement>("[data-resume-task]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const taskId = btn.dataset.resumeTask;
        if (!taskId) return;
        btn.disabled = true;
        await submitReplyValue(shell, taskId, t("task.resumePrompt"));
      });
    });

    // Every delete affordance (row ×, detail-head button) only requests —
    // the confirm dialog commits. The row × is a sibling of the open button,
    // so it can't trigger it; stopPropagation just keeps the click off the
    // document-level dismiss handlers.
    document.querySelectorAll<HTMLButtonElement>("[data-delete-task]").forEach((btn) => {
      btn.addEventListener("click", (event) => {
        event.stopPropagation();
        deleteRequestTaskId = btn.dataset.deleteTask ?? null;
        shell.rerender();
      });
    });
    bindDeleteDialog(shell);

    // Re-apply the selected task's thread scroll position after a render:
    // follow the latest row unless the user had scrolled up to read.
    restoreSelectedEventsScroll();
    // Poll the note archive while the selected task runs (bind runs after
    // every render, so this tracks selection and status changes).
    syncNotesPolling(shell);
    // A full render rebuilds the stream without the DOM-only live strip, and
    // the poll skips ticks where no new note was recorded — so restore the
    // strip here or it stays hidden until the next note lands.
    const selected = selectedTask();
    if (selected && (selected.status === "running" || selected.status === "queued")) {
      updateLiveStrip(selected);
    }
  }

  // A shell render rebuilds the left rail, so restore the task list's previous
  // viewport and keep recording it for the next render.
  function restoreSidebarScroll(): void {
    const list = document.querySelector<HTMLDivElement>(".sidebar-list");
    if (!list) return;
    list.scrollTop = sidebarScrollTop;
    list.addEventListener("scroll", () => {
      sidebarScrollTop = list.scrollTop;
    });
  }

  // Put the freshly rebuilt event stream back where the user left it: at the
  // saved offset when they had scrolled up, otherwise pinned to the newest row.
  // Also (re)attaches the scroll listener that feeds `streamScroll`.
  function restoreSelectedEventsScroll(): void {
    const taskId = selectedTaskId;
    if (!taskId) return;
    const stream = document.querySelector<HTMLDivElement>(`[data-agent-events="${taskId}"]`);
    if (!stream) return;
    const saved = streamScroll.get(taskId);
    stream.scrollTop = saved && !saved.pinned ? saved.top : stream.scrollHeight;
    stream.addEventListener("scroll", () => {
      streamScroll.set(taskId, { top: stream.scrollTop, pinned: isPinnedToBottom(stream) });
    });
  }

  async function pollCodexOAuth(shell: ShellState): Promise<void> {
    for (let attempt = 0; attempt < 120; attempt += 1) {
      await delay(1000);
      try {
        const models = await refreshModels();
        if (models.some((item) => item.provider === "openai" && item.has_key)) {
          keyMessage = "";
          keyError = "";
          savingKey = false;
          editingKey = false;
          shell.rerender();
          return;
        }
      } catch (err) {
        keyMessage = "";
        keyError = `${err}`;
        savingKey = false;
        shell.rerender();
        return;
      }
    }

    keyMessage = "";
    keyError = t("agent.codexLoginMissing");
    savingKey = false;
    shell.rerender();
  }

  function delay(ms: number): Promise<void> {
    return new Promise((resolve) => window.setTimeout(resolve, ms));
  }

  // The pinned composer: one input, two modes. Compose mode (no task shown)
  // starts a fresh task; a shown task takes a follow-up reply instead.
  function bindComposer(shell: ShellState): void {
    syncChromeSetupDetection(shell);
    const composerTask = view === "compose" ? undefined : selectedTask();
    const input = document.getElementById("composer-input") as HTMLTextAreaElement | null;
    if (input) autosizeComposerInput(input);
    updateComposerButton(shell);
    input?.addEventListener("input", () => {
      if (composerTask) replyDraft = input.value;
      else draft = input.value;
      autosizeComposerInput(input);
      updateComposerButton(shell);
    });
    // Enter sends; routed through the button so its disabled state (empty
    // draft, disconnected, no model key, task running) keeps gating submission.
    input?.addEventListener("keydown", (e) => {
      if (!isSendShortcut(e)) return;
      e.preventDefault();
      document.getElementById("composer-send")?.click();
    });
    document.getElementById("composer-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (composerTask) await submitReply(shell, composerTask.task_id);
      else await startAgentTask(shell);
    });
    document.getElementById("composer-connect")?.addEventListener("click", () => {
      invoke("cdp_connect").catch((e) => console.error("cdp_connect failed:", e));
    });
    document.getElementById("overlay-chrome-connect")?.addEventListener("click", () => {
      invoke("cdp_connect").catch((e) => console.error("cdp_connect failed:", e));
    });
    // chrome:// pages cannot be navigated to from ordinary web content. Route
    // the explicit user click through the native shell so Chrome opens its own
    // privileged remote-debugging page.
    document.getElementById("overlay-remote-debugging-help")?.addEventListener("click", (event) => {
      event.preventDefault();
      remoteDebuggingReady = false;
      invoke("open_chrome_remote_debugging")
        .then(() => pollChromeSetup(shell))
        .catch((e) => console.error("open_chrome_remote_debugging failed:", e));
    });
  }

  function syncChromeSetupDetection(shell: ShellState): void {
    chromeSetupStatus = shell.status;

    const overlayVisible = document.getElementById("overlay-remote-debugging-help") !== null;
    if (!overlayVisible || shell.status.state === "connected" || settingsMenu.isRemoteProfile()) {
      stopChromeSetupPolling();
      if (shell.status.state === "connected") {
        remoteDebuggingReady = true;
      }
      return;
    }

    if (chromeSetupPollTimer === null) {
      chromeSetupPollTimer = window.setInterval(() => void pollChromeSetup(shell), 1_000);
    }
    void pollChromeSetup(shell);
  }

  function stopChromeSetupPolling(): void {
    if (chromeSetupPollTimer !== null) window.clearInterval(chromeSetupPollTimer);
    chromeSetupPollTimer = null;
  }

  async function pollChromeSetup(shell: ShellState): Promise<void> {
    if (chromeSetupPollInFlight || chromeSetupStatus.state === "connected") return;
    chromeSetupPollInFlight = true;
    try {
      const ready = await invoke<boolean>("cdp_remote_debugging_ready");
      const changed = ready !== remoteDebuggingReady;
      remoteDebuggingReady = ready;
      if (changed) shell.rerender();
    } catch (e) {
      console.error("cdp_remote_debugging_ready failed:", e);
    } finally {
      chromeSetupPollInFlight = false;
    }
  }

  // Toggling `disabled` on every keystroke via a full shell.rerender() would
  // rebuild the whole pane (losing focus and cursor position mid-type), so
  // this pokes the send button directly instead.
  function updateComposerButton(shell: ShellState): void {
    const button = document.getElementById("composer-send") as HTMLButtonElement | null;
    if (!button) return;
    const task = view === "compose" ? undefined : selectedTask();
    const value = task ? replyDraft : draft;
    const submitting = task ? submittingReply : submittingTask;
    const running = !!task && (task.status === "running" || task.status === "queued");
    // Remote profiles submit while disconnected; the run reconnects on demand.
    const needsConnection =
      shell.status.state !== "connected" && !settingsMenu.isRemoteProfile();
    const selected = selectedModel();
    const modelReady = task ? true : !!selected && selected.has_key;
    button.disabled = submitting || running || !value.trim() || needsConnection || !modelReady;
  }

  // Grows the composer textarea with its content instead of leaving multi-line
  // input scrollable in a short box; capped by the CSS max-height (the browser
  // clips `height` to it, so no separate JS cap is needed).
  function autosizeComposerInput(el: HTMLTextAreaElement): void {
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }

  async function startAgentTask(shell: ShellState): Promise<void> {
    const value = draft.trim();
    if (!value || submittingTask) return;
    submittingTask = true;
    submitError = "";
    shell.rerender();
    try {
      const selected = selectedModel();
      const snapshot = await invoke<AgentTaskSnapshot>("agent_task_start", {
        task: value,
        provider: selected?.provider || modelProvider || null,
        model: selected ? modelId(selected) : model || null,
      });
      upsertTask(snapshot);
      selectedTaskId = snapshot.task_id;
      view = "detail";
      draft = "";
    } catch (err) {
      submitError = `${err}`;
    } finally {
      submittingTask = false;
      shell.rerender();
    }
  }

  // Continues an existing task's conversation: same task_id/session, a fresh
  // run dir for this turn. Mirrors startAgentTask but hits agent_task_reply.
  async function submitReply(shell: ShellState, taskId: string): Promise<void> {
    const value = replyDraft.trim();
    if (!value || submittingReply) return;
    await submitReplyValue(shell, taskId, value);
  }

  async function submitReplyValue(shell: ShellState, taskId: string, value: string): Promise<void> {
    if (!value.trim() || submittingReply) return;
    submittingReply = true;
    replyError = "";
    // Sending snaps the thread back to the newest content (and re-arms
    // auto-follow), even if the user had scrolled up to read.
    streamScroll.set(taskId, { top: 0, pinned: true });
    shell.rerender();
    try {
      const snapshot = await invoke<AgentTaskSnapshot>("agent_task_reply", {
        taskId,
        message: value,
      });
      upsertTask(snapshot);
      replyDraft = "";
    } catch (err) {
      replyError = `${err}`;
    } finally {
      submittingReply = false;
      shell.rerender();
    }
  }

  function upsertTask(snapshot: AgentTaskSnapshot): AgentTaskView {
    const existing = tasks.find((task) => task.task_id === snapshot.task_id);
    const pending = pendingEvents.get(snapshot.task_id) ?? [];
    pendingEvents.delete(snapshot.task_id);
    if (existing) {
      const wasActive = statusRank(existing.status) < 2;
      const merged = mergeSnapshot(existing, snapshot);
      Object.assign(existing, merged, { events: mergeEvents(existing.events, pending) });
      // The answer landing auto-folds the activity: drop the task's explicit
      // fold choices so the default (closed once finished) takes over.
      if (wasActive && statusRank(existing.status) >= 2) {
        for (const key of [...activityOpen.keys()]) {
          if (key.startsWith(`${snapshot.task_id}#`)) activityOpen.delete(key);
        }
      }
      return existing;
    }
    const created = { ...snapshot, events: mergeEvents([], pending) };
    tasks = [...tasks, created];
    if (!selectedTaskId) selectedTaskId = snapshot.task_id;
    return created;
  }

  // Delete-confirm dialog wiring: keep, a click on the scrim itself, or Esc
  // dismisses; delete commits. Focus lands on keep (the safe action) — a full
  // rerender drops focus to <body>, so re-take it whenever it's loose.
  function bindDeleteDialog(shell: ShellState): void {
    escShell = shell;
    bindDeleteDialogEscape();
    const scrim = document.querySelector<HTMLDivElement>("[data-delete-dismiss]");
    if (!scrim) return;
    scrim.addEventListener("click", (event) => {
      if (event.target !== scrim) return;
      deleteRequestTaskId = null;
      shell.rerender();
    });
    const keep = document.getElementById("confirm-delete-keep") as HTMLButtonElement | null;
    keep?.addEventListener("click", () => {
      deleteRequestTaskId = null;
      shell.rerender();
    });
    const commit = document.getElementById("confirm-delete-commit") as HTMLButtonElement | null;
    commit?.addEventListener("click", async () => {
      const taskId = deleteRequestTaskId;
      if (!taskId) return;
      commit.disabled = true;
      deleteRequestTaskId = null;
      try {
        await invoke("agent_task_delete", { taskId });
        removeTask(taskId);
        loadSelectedTaskNotes(shell);
      } catch (err) {
        console.error("agent_task_delete failed:", err);
      } finally {
        shell.rerender();
      }
    });
    if (keep && !scrim.contains(document.activeElement)) keep.focus();
  }

  // Esc must work with focus anywhere, so it lives on document — bound once
  // (bind() runs per render); escShell tracks the latest shell for rerender.
  let escBound = false;
  let escShell: ShellState | null = null;

  function bindDeleteDialogEscape(): void {
    if (escBound) return;
    escBound = true;
    document.addEventListener("keydown", (event) => {
      if (event.key !== "Escape" || !deleteRequestTaskId) return;
      deleteRequestTaskId = null;
      escShell?.rerender();
    });
  }

  // Hand the pane to the neighbouring row (mail-style): the next row below in
  // the newest-first list, else the one above; compose when the list empties.
  function removeTask(taskId: string): void {
    const sorted = [...tasks].sort((a, b) => b.created_at - a.created_at);
    const idx = sorted.findIndex((task) => task.task_id === taskId);
    tasks = tasks.filter((task) => task.task_id !== taskId);
    pendingEvents.delete(taskId);
    streamScroll.delete(taskId);
    for (const key of [...activityOpen.keys()]) {
      if (key.startsWith(`${taskId}#`)) activityOpen.delete(key);
    }
    if (selectedTaskId === taskId) {
      const next = sorted[idx + 1] ?? sorted[idx - 1];
      selectedTaskId = next?.task_id ?? null;
      if (!selectedTaskId) view = "compose";
    }
  }

  function mergeSnapshot(existing: AgentTaskView, incoming: AgentTaskSnapshot): AgentTaskSnapshot {
    // A reply starts a new run_dir on the same task_id. Its first snapshot
    // legitimately regresses status (completed → queued) and nulls out the
    // previous run's final_text/steps/tokens — that's not stale/out-of-order
    // data to guard against, it's a fresh run, so trust it outright instead
    // of falling back to the old run's now-irrelevant values.
    const sameRun = incoming.run_dir === existing.run_dir;
    if (!sameRun) {
      return { ...incoming };
    }
    const status = statusRank(existing.status) > statusRank(incoming.status) ? existing.status : incoming.status;
    const terminalIncoming = statusRank(incoming.status) >= 2;
    return {
      ...incoming,
      status,
      started_at: incoming.started_at ?? existing.started_at,
      finished_at: incoming.finished_at ?? existing.finished_at,
      run_id: incoming.run_id ?? existing.run_id,
      run_dir: incoming.run_dir ?? existing.run_dir,
      target_id: terminalIncoming ? incoming.target_id : incoming.target_id ?? existing.target_id,
      final_text: incoming.final_text ?? existing.final_text,
      error: incoming.error ?? existing.error,
      steps: incoming.steps ?? existing.steps,
      input_tokens: incoming.input_tokens ?? existing.input_tokens,
      output_tokens: incoming.output_tokens ?? existing.output_tokens,
      cached_input_tokens: incoming.cached_input_tokens ?? existing.cached_input_tokens,
      cache_creation_input_tokens:
        incoming.cache_creation_input_tokens ?? existing.cache_creation_input_tokens,
      estimated_cost: incoming.estimated_cost ?? existing.estimated_cost,
      cost_currency: incoming.cost_currency ?? existing.cost_currency,
      points_used: incoming.points_used ?? existing.points_used,
    };
  }

  function statusRank(status: AgentTaskStatus): number {
    switch (status) {
      case "queued": return 0;
      case "running": return 1;
      case "completed": return 2;
      case "failed": return 2;
      case "cancelled": return 2;
      case "interrupted": return 2;
    }
  }

  function stashPendingEvent(payload: AgentTaskEventPayload): void {
    if (!payload.text.trim()) return;
    pendingEvents.set(payload.task_id, mergeEvents(pendingEvents.get(payload.task_id) ?? [], [payload]));
  }

  function appendUniqueEvent(task: AgentTaskView, payload: AgentTaskEventPayload): boolean {
    if (payload.kind === "tool_progress") {
      const index = task.events.findIndex((event) =>
        event.kind === "tool_progress"
        && event.id === payload.id
        && event.phase === payload.phase);
      if (index >= 0) {
        task.events = task.events.map((event, eventIndex) => eventIndex === index ? payload : event);
        return true;
      }
    }
    const key = stableEventKey(payload);
    if (key && task.events.some((event) => stableEventKey(event) === key)) return false;
    task.events = [...task.events, payload];
    return true;
  }

  function mergeEvents(
    existing: AgentTaskEventPayload[],
    incoming: AgentTaskEventPayload[],
  ): AgentTaskEventPayload[] {
    const merged: AgentTaskEventPayload[] = [];
    const stableIndexes = new Map<string, number>();
    for (const event of [...existing, ...incoming]) {
      if (!event.text.trim()) continue;
      const key = stableEventKey(event);
      if (!key) {
        merged.push(event);
        continue;
      }
      const existingIndex = stableIndexes.get(key);
      if (existingIndex === undefined) {
        stableIndexes.set(key, merged.length);
        merged.push(event);
      } else {
        merged[existingIndex] = event;
      }
    }
    return merged.sort(compareEvents);
  }

  function stableEventKey(event: AgentTaskEventPayload): string | null {
    return event.sequence > 0 ? `${event.task_id}:sequence:${event.sequence}` : null;
  }

  function compareEvents(a: AgentTaskEventPayload, b: AgentTaskEventPayload): number {
    if (a.sequence > 0 && b.sequence > 0 && a.sequence !== b.sequence) return a.sequence - b.sequence;
    if (a.created_at !== b.created_at) return a.created_at - b.created_at;
    return 0;
  }

  function selectedTask(): AgentTaskView | undefined {
    if (selectedTaskId) {
      const selected = tasks.find((task) => task.task_id === selectedTaskId);
      if (selected) return selected;
    }
    return newestTask(tasks);
  }

  function newestTask(items: AgentTaskView[]): AgentTaskView | undefined {
    return [...items].sort((a, b) => b.created_at - a.created_at)[0];
  }

  // Patch a streamed row into the live turn without a full render (which
  // would drop the thread's scroll offset on every reasoning/tool row). Run
  // boundaries ("queued"/"started") never reach here — appendTaskEvent asks
  // for a full render that rebuilds the turn structure. The row lands in the
  // last turn's activity fold (skipped when the user folded it — the next
  // full render/toggle rebuilds from state); a result's notes land in the
  // always-visible notes container beneath the fold.
  function appendEventRowIfSelected(payload: AgentTaskEventPayload): void {
    if (payload.task_id !== selectedTaskId) return;
    const stream = document.querySelector<HTMLDivElement>(`[data-agent-events="${payload.task_id}"]`);
    if (!stream) return;

    const pinned = isPinnedToBottom(stream);
    // A result row claims its notes (its own group renders them), so refresh
    // the live strip right away instead of waiting for the next poll tick.
    if (payload.kind === "tool_result") {
      stream.querySelector("[data-live-strip]")?.remove();
      liveStripKey = "";
    }

    const turn = stream.querySelector<HTMLDivElement>(".thread-inner > .turn:last-child");
    const activity = turn?.querySelector<HTMLDivElement>(".activity");
    if (activity) {
      if (payload.kind === "tool_progress") {
        const progressKey = `${payload.id ?? ""}:${payload.phase ?? ""}`;
        const existing = [...activity.querySelectorAll<HTMLElement>("[data-tool-progress]")]
          .find((row) => row.dataset.toolProgress === progressKey);
        if (existing) {
          existing.outerHTML = renderEventRow(payload);
          if (pinned) stream.scrollTop = stream.scrollHeight;
          return;
        }
      }
      const working = activity.querySelector(".act-row--working");
      if (working) working.insertAdjacentHTML("beforebegin", renderEventRow(payload));
      else activity.insertAdjacentHTML("beforeend", renderEventRow(payload));
    }

    if (payload.kind === "tool_result") {
      const group = renderSearchGroupForEvent(payload);
      if (group) lastTurnNotesContainer(stream)?.insertAdjacentHTML("beforeend", group);
    }

    if (pinned) stream.scrollTop = stream.scrollHeight;
    if (payload.kind === "tool_result") {
      const task = tasks.find((item) => item.task_id === payload.task_id);
      if (task) updateLiveStrip(task);
    }
  }
}
