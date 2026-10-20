import { invoke } from "@tauri-apps/api/core";
import { esc } from "../lib/html";
import { platformLabel, RESEARCH_PLATFORMS } from "../lib/platforms";
import { getTimezone, t } from "../lib/i18n";
import type { ShellState } from "../main";
import type { ConversationSharePayload } from "./conversation";

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

interface PromotionInvite {
  code: string;
  url: string;
  status: "available" | "used" | "expired";
  expires_at: string;
}

interface Promotion {
  reward_credits: number;
  signup_credits: number;
  regular_signup_credits: number;
  daily_tasks: number;
  cutoff: string;
  invites: PromotionInvite[];
}

interface ConversationShare {
  id: string;
  url: string;
  title: string;
}

const AUTOMATION_ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3v3M12 18v3M3 12h3M18 12h3"/><circle cx="12" cy="12" r="4"/><path d="m5.6 5.6 2.1 2.1m8.6 8.6 2.1 2.1m0-12.8-2.1 2.1m-8.6 8.6-2.1 2.1"/></svg>`;
const INVITE_ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 7v14"></path><path d="M20 11v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-8"></path><path d="M7.5 7a1 1 0 0 1 0-5A4.8 8 0 0 1 12 7a4.8 8 0 0 1 4.5-5 1 1 0 0 1 0 5"></path><rect x="3" y="7" width="18" height="4" rx="1"></rect></svg>`;
const CLOSE_ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" aria-hidden="true"><path d="M18 6 6 18"></path><path d="m6 6 12 12"></path></svg>`;
const LINK_ICON = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M10 13a5 5 0 0 0 7.1.1l2-2a5 5 0 0 0-7.1-7.1l-1.1 1.1"/><path d="M14 11a5 5 0 0 0-7.1-.1l-2 2A5 5 0 0 0 12 20l1.1-1.1"/></svg>`;

export namespace growthPanel {
  let open = false;
  let loading = false;
  let loaded = false;
  let saving = false;
  let rows: ScheduledTask[] = [];
  let editorId: string | null | undefined;
  let error = "";
  let promotionOpen = false;
  let promotionLoading = false;
  let promotionValue: Promotion | null = null;
  let promotionError = "";
  let copiedInvite = -1;
  let shareOpen = false;
  let shareLoading = false;
  let shareValue: ConversationShare | null = null;
  let shareError = "";

  export function isOpen(): boolean { return open; }

  export function renderPromotionTrigger(loggedIn: boolean): string {
    if (!loggedIn) return "";
    return `<button type="button" class="growth-promotion-trigger" data-promotion-open aria-label="${esc(t("promotion.action"))}" title="${esc(t("promotion.action"))}">${INVITE_ICON}<span>${esc(t("promotion.action"))}</span></button>`;
  }

  export function renderOverlays(): string {
    const promotion = promotionOpen ? renderPromotionDialog() : "";
    const share = shareOpen ? renderShareDialog() : "";
    if (!promotion && !share) return "";
    return `<div class="workspace-overlay-root growth-overlay-root">${promotion}${share}</div>`;
  }

  export async function openShare(input: ConversationSharePayload, shell: ShellState): Promise<void> {
    shareOpen = true;
    shareLoading = true;
    shareValue = null;
    shareError = "";
    shell.rerender();
    try {
      shareValue = await invoke<ConversationShare>("growth_share_create", { input });
    } catch (reason) {
      shareError = String(reason);
    } finally {
      shareLoading = false;
      shell.rerender();
    }
  }

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
    document.querySelector<HTMLButtonElement>("[data-promotion-open]")?.addEventListener("click", () => {
      promotionOpen = true;
      promotionError = "";
      copiedInvite = -1;
      shell.rerender();
      void loadPromotion(false, shell);
    });
    bindOverlays(shell);
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

  function bindOverlays(shell: ShellState): void {
    document.querySelectorAll<HTMLElement>("[data-promotion-close],[data-share-close]").forEach(button => {
      button.addEventListener("click", () => {
        promotionOpen = false;
        shareOpen = false;
        shell.rerender();
      });
    });
    document.querySelectorAll<HTMLElement>("[data-growth-dismiss]").forEach(scrim => {
      scrim.addEventListener("click", event => {
        if (event.target !== scrim) return;
        promotionOpen = false;
        shareOpen = false;
        shell.rerender();
      });
    });
    document.querySelector<HTMLButtonElement>("[data-promotion-generate]")?.addEventListener("click", () => void loadPromotion(true, shell));
    document.querySelector<HTMLButtonElement>("[data-promotion-retry]")?.addEventListener("click", () => void loadPromotion(false, shell));
    document.querySelectorAll<HTMLButtonElement>("[data-copy-invite]").forEach(button => {
      button.addEventListener("click", async () => {
        const index = Number(button.dataset.copyInvite);
        const invite = promotionValue?.invites[index];
        if (!invite) return;
        try {
          await copyText(invite.url);
          copiedInvite = index;
          shell.rerender();
        } catch {
          promotionError = t("promotion.copyFailed");
          shell.rerender();
        }
      });
    });
    document.querySelector<HTMLButtonElement>("[data-share-copy]")?.addEventListener("click", async () => {
      if (!shareValue) return;
      try {
        await copyText(shareValue.url);
        const button = document.querySelector<HTMLButtonElement>("[data-share-copy]");
        if (button) button.textContent = t("share.copied");
      } catch {
        shareError = t("share.copyFailed");
        shell.rerender();
      }
    });
    document.querySelector<HTMLButtonElement>("[data-share-open]")?.addEventListener("click", () => {
      if (shareValue) void invoke("open_external", { url: shareValue.url });
    });
    document.querySelector<HTMLButtonElement>("[data-share-revoke]")?.addEventListener("click", async () => {
      if (!shareValue || shareLoading) return;
      shareLoading = true;
      shell.rerender();
      try {
        await invoke("growth_share_revoke", { shareId: shareValue.id });
        shareOpen = false;
        shareValue = null;
      } catch (reason) {
        shareError = String(reason);
      } finally {
        shareLoading = false;
        shell.rerender();
      }
    });
  }

  async function loadPromotion(create: boolean, shell: ShellState): Promise<void> {
    if (promotionLoading) return;
    promotionLoading = true;
    promotionError = "";
    shell.rerender();
    try {
      promotionValue = await invoke<Promotion>(create ? "growth_promotion_create" : "growth_promotion");
    } catch (reason) {
      promotionError = String(reason);
    } finally {
      promotionLoading = false;
      shell.rerender();
    }
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
    const platforms = ["web", ...RESEARCH_PLATFORMS.filter(value => value !== "auto")];
    return `<form class="growth-editor" data-cron-editor><div class="growth-editor-head"><h2>${esc(row ? t("cron.edit") : t("cron.create"))}</h2><button type="button" class="icon-button" data-cron-editor-close aria-label="${esc(t("cron.close"))}">${CLOSE_ICON}</button></div><label><span>${esc(t("cron.prompt"))}</span><textarea name="prompt" required maxlength="3000">${esc(row?.prompt || "")}</textarea></label><div class="growth-editor-grid"><label><span>${esc(t("cron.when"))}</span><input name="scheduled_for" type="datetime-local" required value="${esc(local)}"></label><label><span>${esc(t("cron.repeat"))}</span><select name="recurrence">${(["once", "daily", "weekly"] as const).map(value => `<option value="${value}"${(row?.recurrence || "once") === value ? " selected" : ""}>${esc(t(`cron.${value}`))}</option>`).join("")}</select></label><label><span>${esc(t("cron.platform"))}</span><select name="platform">${platforms.map(value => `<option value="${value}"${platform === value ? " selected" : ""}>${esc(platformLabel(value))}</option>`).join("")}</select></label></div><div class="growth-editor-actions"><button type="submit" class="btn-primary" ${saving ? "disabled" : ""}>${esc(saving ? t("cron.saving") : t("cron.save"))}</button></div></form>`;
  }

  function renderPromotionDialog(): string {
    let body = `<span class="growth-dialog-pulse" aria-label="${esc(t("common.loading"))}"></span>`;
    if (!promotionLoading && promotionError) {
      body = `<p class="growth-error" role="alert">${esc(t("promotion.loadFailed"))}</p><button type="button" class="btn-ghost" data-promotion-retry>${esc(t("promotion.retry"))}</button>`;
    } else if (!promotionLoading && promotionValue) {
      const value = promotionValue;
      const invites = value.invites.length
        ? `<ol class="growth-invite-list">${value.invites.map((invite, index) => `<li><div><strong>${esc(t("promotion.invitation", { number: index + 1 }))}</strong><span class="growth-invite-state" data-state="${esc(invite.status)}">${esc(t(`promotion.status.${invite.status}`))}</span><small>${esc(t("promotion.expires", { date: new Date(invite.expires_at).toLocaleDateString() }))}</small></div><button type="button" class="btn-ghost btn-compact" data-copy-invite="${index}" ${invite.status !== "available" ? "disabled" : ""}>${esc(copiedInvite === index ? t("promotion.copied") : t("promotion.copy"))}</button></li>`).join("")}</ol><p class="growth-dialog-terms">${esc(t("promotion.singleUse"))}</p>`
        : `<button type="button" class="btn-primary growth-dialog-primary" data-promotion-generate>${esc(t("promotion.generate"))}</button>`;
      body = `<p>${esc(t("promotion.reward", { credits: value.reward_credits }))}</p><p class="growth-dialog-terms">${esc(t("promotion.terms", { credits: value.signup_credits, tasks: value.daily_tasks, cutoff: new Date(value.cutoff).toLocaleString() }))}</p>${invites}`;
    }
    return `<div class="modal-scrim" data-growth-dismiss><section class="growth-dialog-card" role="dialog" aria-modal="true" aria-labelledby="promotion-title"><button type="button" class="icon-button growth-dialog-close" data-promotion-close aria-label="${esc(t("promotion.close"))}">${CLOSE_ICON}</button><div class="growth-dialog-symbol">${INVITE_ICON}</div><h2 id="promotion-title">${esc(t("promotion.action"))}</h2><div class="growth-dialog-body" aria-live="polite">${body}</div></section></div>`;
  }

  function renderShareDialog(): string {
    let body = `<span class="growth-dialog-pulse" aria-label="${esc(t("common.loading"))}"></span><p class="growth-dialog-terms">${esc(t("share.creating"))}</p>`;
    if (!shareLoading && shareError) {
      body = `<p class="growth-error" role="alert">${esc(t("share.failed"))}</p><p class="growth-dialog-terms">${esc(shareError)}</p>`;
    } else if (!shareLoading && shareValue) {
      body = `<p>${esc(t("share.ready"))}</p><div class="growth-share-url"><span>${LINK_ICON}</span><code>${esc(shareValue.url)}</code></div><div class="growth-share-actions"><button type="button" class="btn-primary" data-share-copy>${esc(t("share.copy"))}</button><button type="button" class="btn-ghost" data-share-open>${esc(t("share.open"))}</button><button type="button" class="btn-ghost" data-share-revoke>${esc(t("share.revoke"))}</button></div><p class="growth-dialog-terms">${esc(t("share.snapshotHint"))}</p>`;
    }
    return `<div class="modal-scrim" data-growth-dismiss><section class="growth-dialog-card" role="dialog" aria-modal="true" aria-labelledby="share-title"><button type="button" class="icon-button growth-dialog-close" data-share-close aria-label="${esc(t("share.close"))}">${CLOSE_ICON}</button><div class="growth-dialog-symbol">${LINK_ICON}</div><h2 id="share-title">${esc(t("share.title"))}</h2><div class="growth-dialog-body" aria-live="polite">${body}</div></section></div>`;
  }

  async function copyText(value: string): Promise<void> {
    if (navigator.clipboard?.writeText) return navigator.clipboard.writeText(value);
    const input = document.createElement("textarea");
    input.value = value;
    input.style.position = "fixed";
    input.style.opacity = "0";
    document.body.append(input);
    input.select();
    const copied = document.execCommand("copy");
    input.remove();
    if (!copied) throw new Error("copy failed");
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
