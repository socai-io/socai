import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AgentTaskSnapshot } from "../main";
import { esc } from "./html";
import { getLanguage } from "./i18n";

interface FileInfo { id: string; name: string; size: number; sha256: string; status: string }
interface Draft { upload_id: string; name: string; size: number; remote: FileInfo | null; attempted: boolean }
interface Caps { owner: string; enabled: boolean; desktop_admission_ready: boolean; native_admission_cancellation: boolean; max_attachments: number }
interface Pending {
  request_id: string; task_id: string; scope_id: string; request: Record<string, unknown> | null;
  uploads: string[]; attachments: string[]; claimed: boolean; abandoned: boolean;
}
interface Mount { key: string; task?: AgentTaskSnapshot; disabled: boolean; changed: () => void; recovered: (task: AgentTaskSnapshot) => void }
const label = (zh: string, en: string) => getLanguage() === "zh" ? zh : en;
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
let identity = "", owner = "", generation = 0, busy = false, expanded = false, error = "", notice = "";
let caps: Caps | null = null, mount: Mount | null = null, drafts: Draft[] = [], pending: Pending[] = [];
let accepted: FileInfo[] = [], acceptedKey = "", confirmCancel = "", listening = false;
let composing = false, notifyAfterComposition = false;
let localReady = false;
const selected = new Map<string, Set<string>>();
const phases = new Map<string, string>();
const cacheKey = () => `socai.input-intents.v1.${owner}`;
const target = (item: Pending) => typeof item.request?.task_id === "string" ? item.request.task_id : "new";
const chosen = (key: string) => drafts.filter(file => selected.get(key)?.has(file.upload_id));
const available = () => !!caps?.enabled && caps.desktop_admission_ready && caps.native_admission_cancellation;

function changed(): void {
  if (composing) { notifyAfterComposition = true; return; }
  const key = mount?.key;
  const input = document.activeElement instanceof HTMLTextAreaElement && document.activeElement.id === "composer-input"
    ? document.activeElement : null;
  const start = input?.selectionStart, end = input?.selectionEnd;
  mount?.changed();
  if (input) requestAnimationFrame(() => {
    const next = document.getElementById("composer-input") as HTMLTextAreaElement | null;
    if (mount?.key === key && next && !next.disabled) {
      next.focus({ preventScroll: true });
      next.setSelectionRange(start ?? next.value.length, end ?? next.value.length);
    }
  });
}

function persist(): void {
  // Only IDs and the original message/options, never file bytes or source paths.
  // Failure before Send is fatal: do not dispatch an intent we cannot recover.
  window.localStorage.setItem(cacheKey(), JSON.stringify(pending));
}

function restore(): Pending[] {
  const value: unknown = JSON.parse(window.localStorage.getItem(cacheKey()) || "[]");
  if (!Array.isArray(value) || value.length > 20 || value.some(item => !item || typeof item !== "object"
    || !uuid.test(item.request_id) || !Array.isArray(item.uploads) || !Array.isArray(item.attachments)
    || item.uploads.length > 10 || item.uploads.length !== item.attachments.length
    || [...item.uploads, ...item.attachments].some(id => typeof id !== "string" || !uuid.test(id)))) {
    throw new Error(label("附件提交记录损坏，请勿重新发送；先检查原任务。", "Invalid input intent cache; inspect original tasks before sending again."));
  }
  return value as Pending[];
}

async function loadLocal(expectedGeneration = generation): Promise<void> {
  const account = owner;
  const saved = await invoke<Pending[]>("input_submissions", { owner: account });
  const localDrafts = await invoke<Draft[]>("input_file_drafts", { owner: account });
  if (expectedGeneration !== generation || owner !== account) return;
  const merged = new Map(restore().map(item => [item.request_id, item]));
  for (const item of saved) merged.set(item.request_id, { ...item,
    abandoned: item.claimed ? false : item.abandoned || !!merged.get(item.request_id)?.abandoned });
  if (merged.size > 20) throw new Error(label("待确认提交超过20条，请先处理。", "Review pending submissions before sending more."));
  pending = [...merged.values()]; drafts = localDrafts; persist(); localReady = true;
}

async function capabilities(): Promise<void> {
  const account = owner, current = generation;
  const value = await invoke<Caps>("input_files_capabilities");
  if (current !== generation || account !== owner || value.owner !== account) throw new Error(label("账号或服务已变更，请刷新附件。", "Account/service changed; refresh inputs."));
  caps = value;
  if (!available()) throw new Error(label("当前服务尚未开放完整附件提交与撤销能力。", "This service has not enabled complete input admission and cancellation."));
}

async function loadAccepted(): Promise<void> {
  const context = mount?.task?.input_context;
  if (!context) { accepted = []; acceptedKey = ""; return; }
  if (context.owner !== owner) throw new Error(label("请切回此会话原来的账号及服务。", "Return to this conversation's original account/service."));
  const key = `${owner}:${context.scope_id}:${context.request_id}`, current = generation;
  const result = await invoke<{ scope_id: string; input_files: FileInfo[] }>("input_scope_files", { owner, scopeId: context.scope_id });
  const active = mount?.task?.input_context;
  if (current === generation && active?.scope_id === context.scope_id && active.request_id === context.request_id) {
    accepted = result.input_files; acceptedKey = key;
  }
}

async function operation(action: () => Promise<void>): Promise<void> {
  if (busy) return;
  busy = true; error = ""; notice = ""; const current = generation;
  changed();
  try { await action(); } catch (failure) { if (current === generation) error = String(failure); }
  finally { if (current === generation) { busy = false; changed(); } }
}

function argumentsFor(item: Pending): Record<string, unknown> {
  if (!item.request) throw new Error(label("旧提交缺少原始参数，请检查历史，不要重发。", "Legacy intent has no original parameters; inspect history, do not replay."));
  const args: Record<string, unknown> = {};
  const names: Record<string, string> = { task_id: "taskId", research_mode: "researchMode", custom_workflow: "customWorkflow" };
  for (const [key, value] of Object.entries(item.request)) args[names[key] ?? key] = value;
  return { ...args, inputOwner: owner, requestId: item.request_id, inputUploadIds: item.uploads, attachmentIds: item.attachments };
}

async function dispatch(item: Pending): Promise<AgentTaskSnapshot> {
  const current = generation, account = owner;
  const snapshot = item.claimed
    ? await invoke<AgentTaskSnapshot>("input_submission_recover", { owner: account, requestId: item.request_id })
    : await invoke<AgentTaskSnapshot>(target(item) === "new" ? "agent_task_start" : "agent_task_reply", argumentsFor(item));
  if (current !== generation || owner !== account) throw new Error(label("账号已变更。原提交仍需在原账号核对，不要重发。", "Account changed; reconcile this request in the original account, do not resend."));
  if (snapshot.input_context?.owner !== account || snapshot.input_context.request_id !== item.request_id) throw new Error("Input task acknowledgement does not match the original request");
  item.claimed = true;
  item.scope_id = snapshot.input_context.scope_id;
  try {
    persist();
    await invoke("input_submission_ack", { owner: account, requestId: item.request_id });
    if (current === generation) { pending = pending.filter(value => value.request_id !== item.request_id); persist(); }
  } catch (failure) {
    // A housekeeping failure after a valid response must not hide the actual
    // task or turn a successful dispatch into another Send.
    if (current === generation) error = label("任务已接纳；提交记录清理待重试：", "Task accepted; intent cleanup pending: ") + String(failure);
  }
  const sent = new Set(item.uploads);
  drafts = drafts.filter(file => !sent.has(file.upload_id)); selected.delete(target(item)); acceptedKey = "";
  return snapshot;
}

export namespace nativeInputs {
  export function blocked(key: string, task?: AgentTaskSnapshot): boolean {
    return (!!identity && !localReady) || busy || pending.some(item => target(item) === key)
      || chosen(key).some(file => !file.remote)
      || (!!task?.input_context && (!owner || task.input_context.owner !== owner));
  }

  export function render(key: string, task?: AgentTaskSnapshot, disabled = false): string {
    if (!identity) return task?.input_context ? `<p class="t-small" role="alert">${label("请登录此附件会话原来的账号后再继续。", "Sign in to this input conversation's original account to continue.")}</p>` : "";
    const locked = busy || disabled || pending.some(item => target(item) === key);
    const files = expanded ? drafts.map(file => `<li>
      <label><input type="checkbox" data-input-select="${esc(file.upload_id)}" ${selected.get(key)?.has(file.upload_id) ? "checked" : ""} ${locked ? "disabled" : ""}> ${esc(file.name)} · ${Math.ceil(file.size / 1024)} KiB</label>
      <span>${esc(phases.get(file.upload_id) || (file.remote ? label("已上传，未发送", "Uploaded, not sent") : file.attempted ? label("上传未确认", "Upload unconfirmed") : label("待上传", "Not uploaded")))}</span>
      ${!file.remote ? `<button type="button" data-input-upload="${esc(file.upload_id)}" ${locked ? "disabled" : ""}>${label("上传 / 原ID重试", "Upload / retry same ID")}</button>` : ""}
      <button type="button" data-input-remove="${esc(file.upload_id)}" ${locked ? "disabled" : ""}>${label("移除草稿", "Remove draft")}</button></li>`).join("") : "";
    const context = task?.input_context;
    const sameScope = context && acceptedKey === `${owner}:${context.scope_id}:${context.request_id}`;
    const stored = expanded && sameScope ? accepted.map(file => `<li><span>${esc(file.name)} · ${Math.ceil(file.size / 1024)} KiB</span><button type="button" data-input-save="${esc(file.id)}" ${busy ? "disabled" : ""}>${label("另存原件", "Save original as")}</button></li>`).join("") : "";
    const waiting = pending.map(item => `<li><p>${esc(String(item.request?.task ?? item.request?.message ?? item.task_id))}</p><small>${esc(item.request_id)} · ${item.attachments.length} ${label("份附件", "inputs")}</small>
      ${!item.abandoned ? `<button type="button" data-input-recover="${esc(item.request_id)}" ${busy ? "disabled" : ""}>${item.claimed ? label("核对原任务，不重发", "Inspect original, no replay") : label("按原内容继续发送", "Continue original Send")}</button>` : `<span>${label("已放弃；撤销尚待确认", "Abandoned; cancellation pending")}</span>`}
      ${!item.claimed ? (confirmCancel === item.request_id ? `<p>${label("确认撤销这次提交及本次附件？其他既有附件不受影响；未绑定草稿可能保留，存储容量不会立即释放。", "Cancel this submission and its inputs? Earlier inputs stay; unbound drafts may remain and quota is not released immediately.")}</p><button type="button" data-input-cancel-confirm="${esc(item.request_id)}" ${busy ? "disabled" : ""}>${label("确认撤销", "Confirm cancellation")}</button><button type="button" data-input-keep>${label("保留", "Keep")}</button>` : `<button type="button" data-input-cancel="${esc(item.request_id)}" ${busy ? "disabled" : ""}>${label("撤销本次提交", "Cancel this submission")}</button>`) : ""}</li>`).join("");
    return `<section class="native-inputs t-small" aria-label="${label("对话附件", "Conversation inputs")}">
      <button type="button" data-input-toggle ${busy || !owner ? "disabled" : ""}>${label("附件", "Inputs")}${chosen(key).length ? ` (${chosen(key).length})` : ""}</button>
      ${expanded ? `<button type="button" data-input-choose ${locked || !available() ? "disabled" : ""}>${label("选择图片、Word或文本", "Choose image, Word or text")}</button><button type="button" data-input-refresh ${busy ? "disabled" : ""}>${label("刷新账号/服务及附件", "Refresh account/service and inputs")}</button><p>${label("勾选的文件随本次消息发送；恢复的草稿不会自动加入。上传不代表已读取。", "Only checked inputs accompany Send. Restored drafts are not auto-selected. Upload does not mean read.")}</p><ul>${files}${stored}</ul>` : ""}
      ${waiting ? `<details open><summary>${label("待确认提交", "Unconfirmed submissions")} (${pending.length})</summary><ul>${waiting}</ul></details>` : ""}
      ${context && owner && context.owner !== owner ? `<p role="alert">${label("当前账号或服务与此附件会话不同，请切回原账号及服务。", "This input conversation belongs to another account/service. Return to the original identity.")}</p>` : ""}
      ${busy ? `<p role="status">${label("正在处理，请勿重复提交…", "Working; do not submit again…")}</p>` : ""}
      ${notice ? `<p role="status">${esc(notice)}</p>` : ""}${error ? `<p role="alert">${esc(error)}</p>` : ""}</section>`;
  }

  export function bind(next: Mount, accountIdentity: string): void {
    mount = next;
    const input = document.getElementById("composer-input");
    input?.addEventListener("compositionstart", () => { composing = true; });
    input?.addEventListener("compositionend", () => { composing = false; if (notifyAfterComposition) { notifyAfterComposition = false; changed(); } });
    if (identity !== accountIdentity) {
      identity = accountIdentity; owner = ""; caps = null; drafts = []; pending = []; accepted = []; acceptedKey = "";
      selected.clear(); phases.clear(); error = ""; notice = ""; busy = false; expanded = false; confirmCancel = ""; localReady = !identity;
      const current = ++generation;
      if (identity) void (async () => {
        try {
          const account = await invoke<string>("input_files_identity");
          if (current !== generation) return;
          owner = account; await loadLocal(current);
        } catch (failure) { if (current === generation) error = String(failure); }
        finally { if (current === generation) changed(); }
      })();
    }
    if (!listening) {
      listening = true;
      void listen<{upload_id: string; phase: string}>("input-file-state", ({ payload }) => {
        if (!drafts.some(file => file.upload_id === payload.upload_id)) return;
        const text = payload.phase === "ready" ? label("已上传，未发送", "Uploaded, not sent") : payload.phase === "unconfirmed" ? label("上传未确认，请原ID重试", "Unconfirmed; retry same ID") : label("上传并校验中", "Uploading and validating");
        phases.set(payload.upload_id, text); changed();
      }).catch(() => { listening = false; });
    }
    document.querySelector("[data-input-toggle]")?.addEventListener("click", () => {
      expanded = !expanded; changed();
      if (expanded) void operation(async () => { await capabilities(); await loadLocal(); await loadAccepted(); });
    });
    document.querySelector("[data-input-refresh]")?.addEventListener("click", () => void operation(async () => {
      const account = await invoke<string>("input_files_identity");
      if (account !== owner) { owner = account; caps = null; pending = []; drafts = []; selected.clear(); accepted = []; acceptedKey = ""; localReady = false; }
      await loadLocal(); await capabilities(); await loadAccepted();
    }));
    document.querySelector("[data-input-choose]")?.addEventListener("click", () => {
      const key = next.key, current = generation;
      void operation(async () => {
        await capabilities();
        const account = owner;
        const result = await invoke<{inputs: Draft[]; errors: string[]}>("input_file_choose", { owner: account });
        if (current !== generation) return;
        const ids = selected.get(key) ?? new Set<string>();
        for (const file of result.inputs) { if (ids.size < 10) ids.add(file.upload_id); }
        selected.set(key, ids); error = result.errors.join("\n"); await loadLocal(); changed();
        for (const file of result.inputs) {
          if (current !== generation || owner !== account) return;
          try {
            const remote = await invoke<FileInfo>("input_file_upload", { owner: account, uploadId: file.upload_id });
            if (current !== generation) return;
            const staged = drafts.find(item => item.upload_id === file.upload_id); if (staged) staged.remote = remote;
          } catch (failure) { if (current === generation) error += `\n${file.name}: ${String(failure)}`; }
          changed();
        }
      });
    });
    document.querySelectorAll<HTMLInputElement>("[data-input-select]").forEach(element => element.addEventListener("change", () => {
      const ids = selected.get(next.key) ?? new Set<string>(), id = element.dataset.inputSelect!;
      if (element.checked && ids.size >= 10) { element.checked = false; error = label("每次最多10份附件", "At most 10 inputs per Send"); }
      else if (element.checked) ids.add(id); else ids.delete(id);
      selected.set(next.key, ids); changed();
    }));
    document.querySelectorAll<HTMLElement>("[data-input-upload]").forEach(element => element.addEventListener("click", () => void operation(async () => {
      const uploadId = element.dataset.inputUpload!, account = owner, current = generation;
      const remote = await invoke<FileInfo>("input_file_upload", { owner: account, uploadId });
      if (current === generation) { const file = drafts.find(file => file.upload_id === uploadId); if (file) file.remote = remote; }
    })));
    document.querySelectorAll<HTMLElement>("[data-input-remove]").forEach(element => element.addEventListener("click", () => void operation(async () => {
      const uploadId = element.dataset.inputRemove!, current = generation;
      const result = await invoke<{status: string}>("input_file_remove", { owner, uploadId });
      if (current !== generation) return;
      drafts = drafts.filter(file => file.upload_id !== uploadId); for (const ids of selected.values()) ids.delete(uploadId);
      notice = result.status === "unconfirmed-reservation-may-remain" ? label("本地草稿已移除；远端上传未确认，预留可能保留至过期。", "Local draft removed; an unconfirmed remote reservation may remain until expiry.") : label("草稿已移除；远端容量是否释放以实际回收为准。", "Draft removed; remote capacity awaits actual cleanup.");
    })));
    document.querySelectorAll<HTMLElement>("[data-input-save]").forEach(element => element.addEventListener("click", () => void operation(async () => {
      const context = next.task?.input_context; if (!context) return;
      const current = generation;
      const path = await invoke<string | null>("input_file_original_save_as", { owner, scopeId: context.scope_id, fileId: element.dataset.inputSave });
      if (current === generation && path) notice = label("已另存原件：", "Original saved: ") + path;
    })));
    document.querySelectorAll<HTMLElement>("[data-input-recover]").forEach(element => element.addEventListener("click", () => {
      const item = pending.find(item => item.request_id === element.dataset.inputRecover); if (!item || item.abandoned) return;
      void operation(async () => {
        if (!item.claimed) await capabilities();
        const snapshot = await dispatch(item); mount?.recovered(snapshot);
      });
    }));
    document.querySelectorAll<HTMLElement>("[data-input-cancel]").forEach(element => element.addEventListener("click", () => { confirmCancel = element.dataset.inputCancel!; changed(); }));
    document.querySelector("[data-input-keep]")?.addEventListener("click", () => { confirmCancel = ""; changed(); });
    document.querySelectorAll<HTMLElement>("[data-input-cancel-confirm]").forEach(element => element.addEventListener("click", () => void operation(async () => {
      const item = pending.find(item => item.request_id === element.dataset.inputCancelConfirm); if (!item || item.claimed) return;
      item.abandoned = true; persist();
      const current = generation;
      await invoke("input_submission_cancel", { owner, requestId: item.request_id });
      if (current !== generation) return;
      pending = pending.filter(value => value.request_id !== item.request_id); selected.delete(target(item)); persist();
      confirmCancel = ""; await loadLocal(); notice = label("已撤销本次提交。未绑定草稿如仍保留，可单独移除；远端物理回收尚待确认。", "Submission cancelled. Remaining unbound drafts can be removed separately; remote physical cleanup is not confirmed.");
    })));
  }

  export async function submit(key: string, command: "agent_task_start" | "agent_task_reply", args: Record<string, unknown>, task?: AgentTaskSnapshot): Promise<AgentTaskSnapshot> {
    const files = chosen(key);
    if (identity && !localReady) throw new Error(label("本地提交记录尚未完成核对，请先刷新附件。", "Local intent reconciliation is not ready; refresh inputs first."));
    if (!files.length && !task?.input_context && !pending.some(item => target(item) === key)) return invoke(command, args);
    if (blocked(key, task)) throw new Error(label("请先完成上传或核对待确认提交，不要创建第二次发送。", "Finish uploads or reconcile the pending request; do not create a second Send."));
    if (!available()) await capabilities();
    const request: Record<string, unknown> = {};
    const names: Record<string, string> = { taskId: "task_id", researchMode: "research_mode", customWorkflow: "custom_workflow" };
    for (const [name, value] of Object.entries(args)) request[names[name] ?? name] = value ?? null;
    const requestId = crypto.randomUUID();
    const item: Pending = { request_id: requestId, task_id: task?.task_id ?? `task-input-${requestId}`, scope_id: task?.input_context?.scope_id ?? "",
      request, uploads: files.map(file => file.upload_id), attachments: files.map(file => file.remote!.id), claimed: false, abandoned: false };
    if (pending.length >= 20) throw new Error(label("请先处理待确认提交", "Review pending submissions first"));
    pending.push(item);
    try { persist(); } catch (failure) { pending = pending.filter(value => value !== item); throw failure; }
    return dispatch(item);
  }
}
