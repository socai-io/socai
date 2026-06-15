//! Task workspace coordinator: shared state, agent configuration popover,
//! task event intake, and bindings. The tab bodies live in `task_new.ts` and
//! `task_history.ts`.

import { invoke } from "@tauri-apps/api/core";
import type {
  AgentTaskEventPayload,
  AgentTaskSnapshot,
  AgentTaskStatus,
  ModelInfo,
  ShellState,
} from "../main";
import { esc } from "../lib/html";
import { t } from "../lib/i18n";
import { renderAgentEvent, renderHistoryPage } from "./task_history";
import { renderNewTaskPage } from "./task_new";

export type TaskMode = "agent" | "tools";
type TaskPage = "new" | "history";
export type ToolCommand = "search_notes" | "topic_scan" | "extract_note";
export type AgentTaskView = AgentTaskSnapshot & { events: AgentTaskEventPayload[] };
type CodexLoginStart = { message: string };

// ── Agent task workspace ──────────────────────────────────────────────────

export namespace agentPanel {
  let page: TaskPage = "new";
  let draft = "";
  let mode: TaskMode = "agent";
  let toolCommand: ToolCommand = "search_notes";
  let model = "";
  let modelProvider = "";
  let modelByProvider = new Map<string, string>();
  let submittingTask = false;
  let toolInFlight = false;
  let toolResult: unknown = null;
  let toolError = "";
  let submitError = "";
  let tasks: AgentTaskView[] = [];
  let pendingEvents = new Map<string, AgentTaskEventPayload[]>();
  let selectedTaskId: string | null = null;
  let modelsCache: ModelInfo[] = [];

  // Key-entry sub-state — used by the header configuration popover.
  let pendingKey = "";
  let savingKey = false;
  let codexStarting = false;
  let keyMessage = "";
  let keyError = "";
  let configOpen = false;

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
    document.getElementById("agent-config-toggle")?.addEventListener("click", () => {
      configOpen = selectedNeedsKey() ? true : !configOpen;
      shell.rerender();
    });

    document.querySelectorAll<HTMLButtonElement>(".agent-provider-option").forEach((opt) => {
      opt.addEventListener("click", () => {
        const nextProvider = opt.dataset.provider;
        if (!nextProvider || nextProvider === modelProvider) return;
        keyMessage = "";
        keyError = "";
        pendingKey = "";
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
    return true;
  }

  function renderAgentBadge(): string {
    const selected = selectedModel();
    const expanded = configOpen || selectedNeedsKey() ? "true" : "false";
    if (!selected) {
      return `<button id="agent-config-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}"><i class="badge-dot badge-dot-muted" aria-hidden="true"></i>${esc(t("agent.label"))} · ${esc(t("agent.loading"))}</button>`;
    }
    const label = modelDisplayLabel(selected);
    if (!selected.has_key) {
      return `<button id="agent-config-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}"><i class="badge-dot badge-dot-hollow" aria-hidden="true"></i>${esc(t("agent.label"))} · ${esc(label)} · ${esc(t("agent.keyNeeded"))}</button>`;
    }
    return `<button id="agent-config-toggle" type="button" class="badge badge-button" aria-expanded="${expanded}"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i>${esc(t("agent.label"))} · ${esc(label)}</button>`;
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
    return selected.has_key ? renderCredentialConfigured(selected) : renderHeaderKeyEntry(selected);
  }

  function renderCredentialConfigured(selected: ModelInfo): string {
    return `
      <div class="agent-config-key agent-config-key-ready">
        <p class="t-eyebrow agent-config-title">${esc(t("agent.apiKey"))}</p>
        <p class="t-small subtle">${esc(t("agent.credentialConfigured", { provider: providerDisplayLabel(selected) }))}</p>
      </div>
    `;
  }

  function renderHeaderKeyEntry(selected: ModelInfo): string {
    const openai = selected.provider === "openai";
    return `
      <div class="agent-config-key">
        <p class="t-eyebrow agent-config-title">${esc(t("agent.apiKey"))}</p>
        <p class="t-small subtle">${esc(t("agent.needsCredential", { model: providerDisplayLabel(selected) }))}</p>
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
    const id = modelId(info);
    const name = modelNameLabel(info);
    const recommended = info.recommended ? ` (${t("agent.defaultModel")})` : "";
    return name === id ? `${name}${recommended}` : `${name} — ${id}${recommended}`;
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

  export function render(shell: ShellState): string {
    return `
      <div class="task-interface">
        ${renderPageTabs()}
        ${page === "new"
          ? renderNewTaskPage({
              shell,
              mode,
              toolCommand,
              draft,
              submittingTask,
              toolInFlight,
              toolResult,
              toolError,
              submitError,
              tasks,
              selectedModel: selectedModel(),
            })
          : renderHistoryPage({ tasks, selectedTask: selectedTask(), selectedTaskId })}
      </div>
    `;
  }

  function renderPageTabs(): string {
    return `
      <div class="page-switch" aria-label="${esc(t("task.pagesAria"))}">
        <button id="page-new" type="button" class="page-switch-button ${page === "new" ? "page-switch-button-active" : ""}">${esc(t("task.new"))}</button>
        <button id="page-history" type="button" class="page-switch-button ${page === "history" ? "page-switch-button-active" : ""}">${esc(t("task.history"))}</button>
      </div>
    `;
  }

  export function bind(shell: ShellState): void {
    document.getElementById("page-new")?.addEventListener("click", () => {
      page = "new";
      shell.rerender();
    });
    document.getElementById("page-history")?.addEventListener("click", () => {
      page = "history";
      shell.rerender();
    });
    document.getElementById("history-new-task")?.addEventListener("click", () => {
      page = "new";
      shell.rerender();
    });
    document.getElementById("recent-history-link")?.addEventListener("click", () => {
      page = "history";
      shell.rerender();
    });

    const taskEl = document.getElementById("task-input") as HTMLTextAreaElement | null;
    taskEl?.addEventListener("input", () => {
      draft = taskEl.value;
      updateSubmitButton(shell);
    });

    document.getElementById("mode-agent")?.addEventListener("click", () => {
      mode = "agent";
      submitError = "";
      shell.rerender();
    });
    document.getElementById("mode-tools")?.addEventListener("click", () => {
      mode = "tools";
      submitError = "";
      shell.rerender();
    });
    document.querySelectorAll<HTMLButtonElement>("[data-tool]").forEach((btn) => {
      btn.addEventListener("click", () => {
        toolCommand = btn.dataset.tool as ToolCommand;
        toolError = "";
        toolResult = null;
        shell.rerender();
      });
    });
    document.querySelectorAll<HTMLButtonElement>("[data-task-id]").forEach((btn) => {
      btn.addEventListener("click", () => {
        selectedTaskId = btn.dataset.taskId ?? null;
        page = "history";
        shell.rerender();
      });
    });
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

    document.getElementById("task-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (mode === "agent") await startAgentTask(shell);
      else await runDedicatedTool(shell);
    });

    document.getElementById("overlay-chrome-connect")?.addEventListener("click", () => {
      invoke("cdp_connect").catch((e) => console.error("cdp_connect failed:", e));
    });

    // Tauri's webview won't hand target="_blank" off to the OS browser, so open
    // the setup guide via the backend instead of relying on the anchor default.
    document
      .getElementById("overlay-remote-debugging-help")
      ?.addEventListener("click", (e) => {
        e.preventDefault();
        const url = (e.currentTarget as HTMLAnchorElement).href;
        invoke("open_external", { url }).catch((err) =>
          console.error("open_external failed:", err),
        );
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
    const agentMode = mode === "agent";
    const selected = selectedModel();
    const modelReady = !!selected && selected.has_key;
    const running = agentMode ? submittingTask : toolInFlight;
    button.disabled = running || !draft.trim() || !connected || (agentMode && !modelReady);
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
      page = "history";
      draft = "";
    } catch (err) {
      submitError = `${err}`;
    } finally {
      submittingTask = false;
      shell.rerender();
    }
  }

  async function runDedicatedTool(shell: ShellState): Promise<void> {
    const value = draft.trim();
    if (!value || toolInFlight) return;
    toolInFlight = true;
    toolError = "";
    toolResult = null;
    shell.rerender();
    try {
      if (toolCommand === "extract_note") {
        toolResult = await invoke("tool_extract_note", { noteId: value });
      } else if (toolCommand === "topic_scan") {
        toolResult = await invoke("tool_topic_scan", { query: value });
      } else {
        toolResult = await invoke("tool_search_notes", { query: value });
      }
    } catch (err) {
      toolError = `${err}`;
    } finally {
      toolInFlight = false;
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

    stream.insertAdjacentHTML("beforeend", renderAgentEvent(payload));
    stream.scrollTop = stream.scrollHeight;
  }
}
