//! Task workspace coordinator: shared state, agent configuration popover,
//! task event intake, and bindings. The sidebar (history list) + detail pane
//! live in `task_history.ts`; the compose view lives in `task_new.ts`.

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
import { noteRefsFromEvent, renderAgentEvent, renderConfirmDeleteDialog, renderSidebar as renderSidebarMarkup, renderTaskDetail } from "./task_history";
import { bindNoteInteractions, renderTimelineEmbed, setNoteRegistry } from "./notes";
import { renderComposePane } from "./task_new";

// The workspace shows one of two views: the compose form (default / "new task")
// or the selected task's detail. The sidebar history list is always present.
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
  let tasks: AgentTaskView[] = [];
  let pendingEvents = new Map<string, AgentTaskEventPayload[]>();
  let selectedTaskId: string | null = null;
  // Confirm-first delete: every affordance (row ×, detail-head button) opens
  // the centered dialog by setting this; the delete only runs on confirm.
  let deleteRequestTaskId: string | null = null;
  let modelsCache: ModelInfo[] = [];

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

  let liveStripKey = "";

  // Per-task scroll memory for the event stream. A full render rebuilds the
  // stream DOM (dropping its scroll offset), so a scroll listener records the
  // position here and the post-render bind restores it. No entry, or a pinned
  // entry, means auto-follow: keep the stream glued to its newest row.
  const streamScroll = new Map<string, { top: number; pinned: boolean }>();

  function isPinnedToBottom(stream: HTMLDivElement): boolean {
    return stream.scrollTop + stream.clientHeight >= stream.scrollHeight - 8;
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
    const key = refs.join(",");
    const existing = stream.querySelector("[data-live-strip]");
    if (key === liveStripKey && existing) return;
    liveStripKey = key;
    existing?.remove();
    if (refs.length === 0) return;
    const html = renderTimelineEmbed(refs, "rich");
    if (!html) return;
    const pinned = isPinnedToBottom(stream);
    stream.insertAdjacentHTML("beforeend", html);
    stream.lastElementChild?.setAttribute("data-live-strip", "1");
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

  function renderAgentBadge(): string {
    const selected = selectedModel();
    const expanded = configOpen || selectedNeedsKey() ? "true" : "false";
    if (!selected) {
      return `<button id="agent-config-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}"><i class="badge-dot badge-dot-muted" aria-hidden="true"></i><span class="badge-text">${esc(t("agent.label"))} · ${esc(t("agent.loading"))}</span></button>`;
    }
    const label = modelDisplayLabel(selected);
    if (!selected.has_key) {
      return `<button id="agent-config-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}"><i class="badge-dot badge-dot-hollow" aria-hidden="true"></i><span class="badge-text">${esc(t("agent.label"))} · ${esc(label)} · ${esc(t("agent.keyNeeded"))}</span></button>`;
    }
    return `<button id="agent-config-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i><span class="badge-text">${esc(t("agent.label"))} · ${esc(label)}</span></button>`;
  }

  function renderConfigPopover(): string {
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
        const hint = selectedForProvider ? modelNameLabel(selectedForProvider) : t("common.loading");
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
      <div class="topbar-popover agent-config-popover" role="dialog" aria-label="${esc(t("agent.configurationAria"))}">
        <section class="agent-config-field">
          <p class="t-eyebrow agent-config-title">${esc(t("agent.provider"))}</p>
          <div class="agent-model-list agent-provider-list" role="listbox" aria-label="${esc(t("agent.selectProviderAria"))}">
            ${providerOptions || `<p class="t-small subtle agent-picker-empty">${esc(t("common.loading"))}</p>`}
          </div>
        </section>
        ${renderCredentialSection(activeModel)}
        <section class="agent-config-field">
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
        </section>
      </div>
    `;
  }

  function renderCredentialSection(selected: ModelInfo | undefined): string {
    if (!selected) return "";
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
            : t("agent.credentialConfigured", { provider: providerDisplayLabel(selected) }),
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

  function persistModelChoice(info: ModelInfo): void {
    const id = modelId(info);
    if (!id) return;
    invoke("agent_set_default_model", { provider: info.provider, model: id }).catch(
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
  // should re-render because a task snapshot/status changed.
  export function appendTaskEvent(payload: AgentTaskEventPayload): boolean {
    if (payload.snapshot) upsertTask(payload.snapshot);

    const task = tasks.find((item) => item.task_id === payload.task_id);
    if (!task) {
      stashPendingEvent(payload);
      return !!payload.snapshot;
    }

    if (payload.text.trim()) {
      const added = appendUniqueEvent(task, payload);
      if (added) appendEventRowIfSelected(payload);
    }

    return !!payload.snapshot;
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

  // The main pane: the compose form, or the selected task's detail. The
  // delete-confirm dialog (a fixed overlay) rides along with either view.
  export function renderWorkspace(shell: ShellState): string {
    const deleteRequest = tasks.find((task) => task.task_id === deleteRequestTaskId);
    const dialog = deleteRequest ? renderConfirmDeleteDialog(deleteRequest) : "";
    if (view === "compose") {
      return `${renderComposePane({
        shell,
        draft,
        submittingTask,
        submitError,
        selectedModel: selectedModel(),
      })}${dialog}`;
    }
    return `
      <section class="task-detail" aria-label="${esc(t("task.selectedAria"))}">
        ${renderTaskDetail(selectedTask())}
      </section>
      ${dialog}
    `;
  }

  export function bind(shell: ShellState): void {
    document.getElementById("sidebar-new")?.addEventListener("click", () => {
      view = "compose";
      shell.rerender();
    });

    const taskEl = document.getElementById("task-input") as HTMLTextAreaElement | null;
    taskEl?.addEventListener("input", () => {
      draft = taskEl.value;
      updateSubmitButton(shell);
    });

    // The row's open control is a native <button>, so click/Enter/Space are
    // handled for free — no hand-bound keydown, no role/tabindex.
    document.querySelectorAll<HTMLButtonElement>("[data-task-id]").forEach((btn) => {
      btn.addEventListener("click", () => {
        selectedTaskId = btn.dataset.taskId ?? null;
        view = "detail";
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

    document.getElementById("task-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      await startAgentTask(shell);
    });

    document.getElementById("overlay-chrome-connect")?.addEventListener("click", () => {
      invoke("cdp_connect").catch((e) => console.error("cdp_connect failed:", e));
    });

    // Re-apply the selected task's timeline scroll position after a render:
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

  function updateSubmitButton(shell: ShellState): void {
    const button = document.getElementById("task-submit") as HTMLButtonElement | null;
    if (!button) return;
    const connected = shell.status.state === "connected";
    const selected = selectedModel();
    const modelReady = !!selected && selected.has_key;
    button.disabled = submittingTask || !draft.trim() || !connected || !modelReady;
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

  function upsertTask(snapshot: AgentTaskSnapshot): AgentTaskView {
    const existing = tasks.find((task) => task.task_id === snapshot.task_id);
    const pending = pendingEvents.get(snapshot.task_id) ?? [];
    pendingEvents.delete(snapshot.task_id);
    if (existing) {
      const merged = mergeSnapshot(existing, snapshot);
      Object.assign(existing, merged, { events: mergeEvents(existing.events, pending) });
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
    if (selectedTaskId === taskId) {
      const next = sorted[idx + 1] ?? sorted[idx - 1];
      selectedTaskId = next?.task_id ?? null;
      if (!selectedTaskId) view = "compose";
    }
  }

  function mergeSnapshot(existing: AgentTaskView, incoming: AgentTaskSnapshot): AgentTaskSnapshot {
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
      turns: incoming.turns ?? existing.turns,
      input_tokens: incoming.input_tokens ?? existing.input_tokens,
      output_tokens: incoming.output_tokens ?? existing.output_tokens,
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

  function appendEventRowIfSelected(payload: AgentTaskEventPayload): void {
    if (payload.task_id !== selectedTaskId) return;
    const stream = document.querySelector<HTMLDivElement>(`[data-agent-events="${payload.task_id}"]`);
    if (!stream) return;

    const placeholder = stream.querySelector("[data-events-placeholder]");
    if (placeholder) placeholder.remove();

    // A result row claims its notes (its own embed renders them), so refresh
    // the live strip right away instead of waiting for the next poll tick.
    if (payload.kind === "tool_result") {
      stream.querySelector("[data-live-strip]")?.remove();
      liveStripKey = "";
    }
    const pinned = isPinnedToBottom(stream);
    stream.insertAdjacentHTML("beforeend", renderAgentEvent(payload));
    if (pinned) stream.scrollTop = stream.scrollHeight;
    if (payload.kind === "tool_result") {
      const task = tasks.find((item) => item.task_id === payload.task_id);
      if (task) updateLiveStrip(task);
    }
  }
}
