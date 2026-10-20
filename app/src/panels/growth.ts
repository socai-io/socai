import { invoke } from "@tauri-apps/api/core";
import { esc } from "../lib/html";
import { platformLabel, RESEARCH_PLATFORMS } from "../lib/platforms";
import { getTimezone, t } from "../lib/i18n";
import type { ShellState } from "../main";

interface ScheduledTask {
  id: string;
  title: string;
  prompt: string;
  platform: string;
  platforms: string[];
  scheduled_for: string;
  timezone: string;
  recurrence: "once" | "daily" | "weekly";
  status: string;
  conversation_id?: string | null;
  screenshot_count: number;
  error?: string | null;
}

interface ScheduledTaskInput {
  prompt: string;
  platforms: string[];
  scheduled_for: string;
  timezone: string;
  recurrence: string;
  client_request_id?: string;
}

const AUTOMATION_ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3v3M12 18v3M3 12h3M18 12h3"/><circle cx="12" cy="12" r="4"/><path d="m5.6 5.6 2.1 2.1m8.6 8.6 2.1 2.1m0-12.8-2.1 2.1m-8.6 8.6-2.1 2.1"/></svg>`;

export namespace growthPanel {
  let open = false;
  let loading = false;
  let loaded = false;
  let saving = false;
  let rows: ScheduledTask[] = [];
  let editorId: string | null | undefined;
  let error = "";

  export function isOpen(): boolean { return open; }

  export function renderWorkspace(): string {
    if (loading && !loaded) return `<section class="growth-page"><div class="growth-page-head"><h1>${esc(t("cron.title"))}</h1></div>${skeleton()}</section>`;
    const editor = editorId === undefined ? "" : renderEditor(editorId ? rows.find(row => row.id === editorId) : undefined);
    return `<section class="growth-page" aria-label="${esc(t("cron.title"))}">
      <div class="growth-page-head"><div><p class="t-eyebrow">${esc(t("cron.automation"))}</p><h1>${esc(t("cron.title"))}</h1></div><button type="button" class="btn-primary" data-cron-create>+ ${esc(t("cron.create"))}</button></div>
      ${error ? `<p class="growth-error" role="alert">${esc(error)}</p>` : ""}
      ${editor}
      <div class="growth-list">${rows.length ? rows.map(renderRow).join("") : `<div class="growth-empty"><span>${AUTOMATION_ICON}</span><h2>${esc(t("cron.empty"))}</h2><p>${esc(t("cron.emptyHint"))}</p></div>`}</div>
    </section>`;
  }

  export function bind(shell: ShellState): void {
    document.querySelector<HTMLButtonElement>("[data-cron-nav]")?.addEventListener("click", () => {
      open = true;
      document.body.dataset.socaiGrowthSurface = "cron";
      shell.rerender();
      void load(shell);
    });
    document.querySelectorAll<HTMLElement>("#sidebar-new,[data-task-id]").forEach(element => {
      element.addEventListener("click", () => {
        if (!open) return;
        open = false;
        delete document.body.dataset.socaiGrowthSurface;
        editorId = undefined;
        shell.rerender();
      });
    });
    if (!open) return;
    document.querySelector<HTMLButtonElement>("[data-cron-create]")?.addEventListener("click", () => {
      editorId = null; error = ""; shell.rerender();
    });
    document.querySelectorAll<HTMLButtonElement>("[data-cron-edit]").forEach(button => {
      button.addEventListener("click", () => { editorId = button.dataset.cronEdit || null; error = ""; shell.rerender(); });
    });
    document.querySelector<HTMLButtonElement>("[data-cron-editor-close]")?.addEventListener("click", () => {
      editorId = undefined; shell.rerender();
    });
    document.querySelector<HTMLFormElement>("[data-cron-editor]")?.addEventListener("submit", event => {
      event.preventDefault();
      void save(event.currentTarget as HTMLFormElement, shell);
    });
    document.querySelectorAll<HTMLButtonElement>("[data-cron-cancel]").forEach(button => {
      button.addEventListener("click", () => void mutate("growth_schedule_cancel", button.dataset.cronCancel, shell));
    });
    document.querySelectorAll<HTMLButtonElement>("[data-cron-delete]").forEach(button => {
      button.addEventListener("click", () => void mutate("growth_schedule_delete", button.dataset.cronDelete, shell));
    });
  }

  async function load(shell: ShellState): Promise<void> {
    if (loading) return;
    loading = true; error = ""; shell.rerender();
    try {
      rows = await invoke<ScheduledTask[]>("growth_schedules");
      loaded = true;
    } catch (reason) { error = String(reason); }
    finally { loading = false; shell.rerender(); }
  }

  async function save(form: HTMLFormElement, shell: ShellState): Promise<void> {
    if (saving) return;
    const data = new FormData(form);
    const when = new Date(String(data.get("scheduled_for") || ""));
    if (Number.isNaN(when.getTime())) return;
    const input: ScheduledTaskInput = {
      prompt: String(data.get("prompt") || "").trim(),
      platforms: [String(data.get("platform") || "web")],
      scheduled_for: when.toISOString(),
      timezone: getTimezone() === "system" ? Intl.DateTimeFormat().resolvedOptions().timeZone : getTimezone(),
      recurrence: String(data.get("recurrence") || "once"),
      ...(editorId ? {} : { client_request_id: crypto.randomUUID() }),
    };
    saving = true; error = ""; shell.rerender();
    try {
      if (editorId) await invoke("growth_schedule_update", { scheduleId: editorId, input });
      else await invoke("growth_schedule_create", { input });
      editorId = undefined;
      rows = await invoke<ScheduledTask[]>("growth_schedules");
      loaded = true;
    } catch (reason) { error = String(reason); }
    finally { saving = false; shell.rerender(); }
  }

  async function mutate(command: string, scheduleId: string | undefined, shell: ShellState): Promise<void> {
    if (!scheduleId || saving) return;
    saving = true; error = ""; shell.rerender();
    try {
      await invoke(command, { scheduleId });
      rows = await invoke<ScheduledTask[]>("growth_schedules");
      if (editorId === scheduleId) editorId = undefined;
    } catch (reason) { error = String(reason); }
    finally { saving = false; shell.rerender(); }
  }

  function renderRow(row: ScheduledTask): string {
    const active = ["scheduled", "dispatching", "dispatched"].includes(row.status);
    return `<article class="growth-row" data-state="${esc(row.status)}"><button type="button" class="growth-row-main" data-cron-edit="${esc(row.id)}"><span class="growth-row-icon">${AUTOMATION_ICON}</span><span class="growth-row-copy"><strong>${esc(row.title)}</strong><small>${esc(row.platforms.map(platformLabel).join(" · "))} · ${esc(t(`cron.${row.recurrence}`))}</small><time>${esc(new Date(row.scheduled_for).toLocaleString())}</time></span><span class="growth-row-state">${esc(scheduleStatusLabel(row.status))}</span></button><div class="growth-row-actions">${active ? `<button type="button" class="btn-ghost btn-compact" data-cron-cancel="${esc(row.id)}">${esc(t("cron.cancel"))}</button>` : `<button type="button" class="icon-button" data-cron-delete="${esc(row.id)}" aria-label="${esc(t("cron.delete"))}">×</button>`}</div>${row.error ? `<p class="growth-error">${esc(row.error)}</p>` : ""}</article>`;
  }

  function renderEditor(row?: ScheduledTask): string {
    const local = row ? localDateTime(row.scheduled_for) : localDateTime(new Date(Date.now() + 3600_000).toISOString());
    const platform = row?.platforms[0] || row?.platform || "web";
    return `<form class="growth-editor" data-cron-editor><div class="growth-editor-head"><h2>${esc(row ? t("cron.edit") : t("cron.create"))}</h2><button type="button" class="icon-button" data-cron-editor-close aria-label="${esc(t("cron.close"))}">×</button></div><label><span>${esc(t("cron.prompt"))}</span><textarea name="prompt" required maxlength="3000">${esc(row?.prompt || "")}</textarea></label><div class="growth-editor-grid"><label><span>${esc(t("cron.when"))}</span><input name="scheduled_for" type="datetime-local" required value="${esc(local)}"></label><label><span>${esc(t("cron.repeat"))}</span><select name="recurrence">${(["once", "daily", "weekly"] as const).map(value => `<option value="${value}"${(row?.recurrence || "once") === value ? " selected" : ""}>${esc(t(`cron.${value}`))}</option>`).join("")}</select></label><label><span>${esc(t("cron.platform"))}</span><select name="platform">${RESEARCH_PLATFORMS.filter(value => value !== "auto").map(value => `<option value="${value}"${platform === value ? " selected" : ""}>${esc(platformLabel(value))}</option>`).join("")}</select></label></div><div class="growth-editor-actions"><button type="submit" class="btn-primary" ${saving ? "disabled" : ""}>${esc(saving ? t("cron.saving") : t("cron.save"))}</button></div></form>`;
  }

  function localDateTime(value: string): string {
    const date = new Date(value), offset = date.getTimezoneOffset() * 60_000;
    return new Date(date.getTime() - offset).toISOString().slice(0, 16);
  }

  function scheduleStatusLabel(value: string): string {
    switch (value) {
      case "scheduled": return t("cron.status.scheduled");
      case "dispatching": return t("cron.status.dispatching");
      case "dispatched": return t("cron.status.dispatched");
      case "completed": return t("cron.status.completed");
      case "failed": return t("cron.status.failed");
      case "cancelled": return t("cron.status.cancelled");
      default: return value;
    }
  }

  function skeleton(): string { return `<div class="growth-skeleton"><i></i><i></i><i></i></div>`; }
}
