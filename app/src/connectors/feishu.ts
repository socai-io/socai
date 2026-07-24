import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ShellState } from "../main";
import { esc } from "../lib/html";
import { t } from "../lib/i18n";
import feishuLogo from "../assets/connectors/feishu.png";
import "./feishu.css";

export interface ConnectorTask {
  task_id: string;
  task: string;
}

type FeishuStatus = {
  configured: boolean;
  connected: boolean;
  identity: string;
  profile: string;
  user_name?: string | null;
  message?: string | null;
};
type FeishuAccount = {
  profile: string;
  user_name?: string | null;
  avatar_url?: string | null;
  tenant_key?: string | null;
  connected: boolean;
  active: boolean;
};
type FeishuDocument = { document_id: string; url: string; title: string };
type FeishuChat = { chat_id: string; name: string; description?: string | null };
type FeishuConnectEvent = {
  stage: "app" | "user";
  state: "starting" | "awaiting_authorization" | "completed" | "open_failed";
  url?: string | null;
};
type Destination = "document" | "group";
type FeishuPhase =
  | "closed"
  | "loading_accounts"
  | "choose"
  | "connecting"
  | "exporting"
  | "ready"
  | "sending"
  | "sent"
  | "confirm_disconnect"
  | "disconnecting"
  | "error";

type AnswerResolver = (taskId: string, turnIndex: number) => string | null;

let phase: FeishuPhase = "closed";
let phaseBeforeDisconnect: FeishuPhase = "choose";
let taskId: string | null = null;
let answerContent = "";
let destination: Destination | null = null;
let accounts: FeishuAccount[] = [];
let selectedProfile = "";
let documentResult: FeishuDocument | null = null;
let chats: FeishuChat[] = [];
let chatsLoading = false;
let chatError = "";
let errorMessage = "";
let connectMessage = "";
let eventsBound = false;
let operation = 0;

const CONNECT_ACCOUNT_VALUE = "__connect_account__";
const CHAT_STORAGE_KEY = "socai-feishu-last-chat";
const PROFILE_STORAGE_KEY = "socai-feishu-profile";

export function renderFeishuConnector(tasks: ConnectorTask[]): string {
  if (phase === "closed") return "";
  const task = taskId ? tasks.find((item) => item.task_id === taskId) : null;
  let body = "";

  if (phase === "loading_accounts") {
    body = renderProgress(t("feishu.loadingAccounts"));
  } else if (phase === "connecting" && destination !== "document") {
    body = renderProgress(connectMessage || t("feishu.authorizing"));
  } else if (phase === "disconnecting") {
    body = renderProgress(t("feishu.disconnecting"));
  } else if (phase === "confirm_disconnect") {
    body = renderDisconnectConfirmation();
  } else if (phase === "error") {
    body = `
      <pre class="conv-error feishu-error">${esc(errorMessage)}</pre>
      ${renderAccountPicker()}
      <div class="confirm-dialog-actions">
        <button type="button" class="btn-primary" data-feishu-retry>${esc(t("feishu.retry"))}</button>
      </div>
    `;
  } else {
    body = renderDestinationChoice();
  }

  return `
    <div class="modal-scrim" data-feishu-scrim>
      <section class="confirm-dialog feishu-dialog" role="dialog" aria-modal="true" aria-label="${esc(t("feishu.dialogAria"))}">
        <div class="feishu-dialog-head">
          <div class="feishu-dialog-heading">
            <div class="feishu-dialog-title-row">
              <img class="feishu-dialog-title-icon" src="${feishuLogo}" alt="">
              <p class="t-h3 feishu-dialog-title">${esc(t("feishu.title"))}</p>
            </div>
            ${task ? `<p class="confirm-dialog-task">${esc(task.task)}</p>` : ""}
          </div>
          <button type="button" class="feishu-dialog-close" data-feishu-close aria-label="${esc(t("feishu.close"))}">×</button>
        </div>
        ${body}
      </section>
    </div>
  `;
}

export function bindFeishuConnector(
  shell: ShellState,
  resolveAnswer: AnswerResolver,
): void {
  bindConnectEvents(shell);
  document.querySelectorAll<HTMLButtonElement>("[data-feishu-export]").forEach((button) => {
    button.addEventListener("click", () => {
      const selectedTaskId = button.dataset.feishuExport;
      const selectedTurn = Number(button.dataset.feishuTurn ?? "0");
      if (!selectedTaskId || !Number.isInteger(selectedTurn)) return;
      const content = resolveAnswer(selectedTaskId, selectedTurn);
      if (content) void openDialog(shell, selectedTaskId, content);
    });
  });
  document.querySelectorAll<HTMLButtonElement>("[data-feishu-close]").forEach((button) => {
    button.addEventListener("click", () => closeDialog(shell));
  });
  document.querySelector<HTMLElement>("[data-feishu-scrim]")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget && !isBusy()) closeDialog(shell);
  });
  document.querySelector<HTMLSelectElement>("[data-feishu-account]")?.addEventListener("change", (event) => {
    const select = event.currentTarget as HTMLSelectElement;
    const profile = select.value;
    if (profile === CONNECT_ACCOUNT_VALUE) {
      select.value = selectedProfile;
      void connectNewAccount(shell);
    } else if (profile) {
      void selectAccount(shell, profile);
    }
  });
  document.querySelector<HTMLButtonElement>("[data-feishu-disconnect]")?.addEventListener("click", () => {
    phaseBeforeDisconnect = phase;
    phase = "confirm_disconnect";
    shell.rerender();
  });
  document.querySelector<HTMLButtonElement>("[data-feishu-disconnect-cancel]")?.addEventListener("click", () => {
    phase = phaseBeforeDisconnect;
    shell.rerender();
  });
  document.querySelector<HTMLButtonElement>("[data-feishu-disconnect-confirm]")?.addEventListener("click", () => {
    void disconnectAccount(shell);
  });
  document.querySelectorAll<HTMLButtonElement>("[data-feishu-destination]").forEach((button) => {
    button.addEventListener("click", () => {
      const nextDestination = button.dataset.feishuDestination;
      if (nextDestination === "document") void createDocument(shell);
    });
  });
  document.querySelector<HTMLButtonElement>("[data-feishu-retry]")?.addEventListener("click", () => {
    if (destination === "document") void createDocument(shell);
    else void loadAccounts(shell);
  });
  document.querySelector<HTMLAnchorElement>("[data-feishu-open-document]")?.addEventListener("click", (event) => {
    event.preventDefault();
    if (documentResult) void invoke("open_external", { url: documentResult.url });
  });
  document.querySelector<HTMLButtonElement>("[data-feishu-send]")?.addEventListener("click", () => {
    void sendToChat(shell);
  });
}

function renderDestinationChoice(): string {
  let documentAction = `
    <button type="button" class="feishu-destination-card" data-feishu-destination="document">
      <span class="feishu-destination-card__icon" aria-hidden="true">□</span>
      <strong>${esc(t("feishu.toDocument"))}</strong>
    </button>
  `;
  if (
    phase === "exporting" ||
    (phase === "connecting" && destination === "document")
  ) {
    const message =
      phase === "exporting"
        ? t("feishu.exporting")
        : connectMessage || t("feishu.authorizing");
    documentAction = `
      <div class="feishu-destination-card feishu-destination-card--busy" aria-live="polite">
        <i class="badge-dot badge-dot-ink badge-dot-pulse" aria-hidden="true"></i>
        <strong>${esc(message)}</strong>
      </div>
    `;
  } else if (documentResult) {
    documentAction = `
      <a class="feishu-destination-card feishu-destination-link" href="${esc(documentResult.url)}" data-feishu-open-document>
        <span class="feishu-destination-card__icon" aria-hidden="true">□</span>
        <span class="t-small">${esc(documentResult.url)}</span>
      </a>
    `;
  }
  return `
    <div class="feishu-destination-stack">
      ${documentAction}
      ${renderGroupAction()}
    </div>
    ${renderAccountPicker()}
  `;
}

function renderAccountPicker(): string {
  const selected = accounts.find((account) => account.profile === selectedProfile);
  const options = accounts
    .map((account) => {
      const user = account.user_name || t("feishu.unknownAccount");
      const status = account.connected ? "" : `（${t("feishu.disconnected")}）`;
      return `<option value="${esc(account.profile)}" ${account.profile === selectedProfile ? "selected" : ""}>${esc(`${user}${status}`)}</option>`;
    })
    .join("");
  return `
    <div class="feishu-account">
      <label class="t-small" for="feishu-account-select">${esc(t("feishu.account"))}</label>
      <div class="feishu-account-row">
        ${
          selected?.avatar_url
            ? `<img class="feishu-account-avatar" src="${esc(selected.avatar_url)}" alt="">`
            : `<span class="feishu-account-avatar feishu-account-avatar--empty" aria-hidden="true"></span>`
        }
        <select id="feishu-account-select" class="input-field" data-feishu-account>
          ${options || `<option value="" selected disabled>${esc(t("feishu.noAccount"))}</option>`}
          <option value="${CONNECT_ACCOUNT_VALUE}">${esc(t("feishu.connectCurrentAccount"))}</option>
        </select>
        ${
          selected
            ? `<button type="button" class="btn-ghost btn-compact" data-feishu-disconnect>${esc(t("feishu.disconnect"))}</button>`
            : ""
        }
      </div>
    </div>
  `;
}

function renderGroupAction(): string {
  const rememberedChat = localStorage.getItem(CHAT_STORAGE_KEY) ?? "";
  const lastChat = chats.some((chat) => chat.chat_id === rememberedChat) ? rememberedChat : "";
  const options = chats
    .map(
      (chat) =>
        `<option value="${esc(chat.chat_id)}" ${chat.chat_id === lastChat ? "selected" : ""}>${esc(chat.name)}</option>`,
    )
    .join("");
  const control = chatsLoading
    ? `<select class="input-field" disabled><option>${esc(t("feishu.loadingGroups"))}</option></select>`
    : chats.length > 0
      ? `
        <select id="feishu-chat-select" class="input-field" aria-label="${esc(t("feishu.chooseGroup"))}">
          <option value="" ${lastChat ? "" : "selected"} disabled>${esc(t("feishu.chooseGroup"))}</option>
          ${options}
        </select>
      `
      : `<select class="input-field" disabled><option>${esc(t("feishu.noGroups"))}</option></select>`;
  const status = [
    phase === "sending" && connectMessage
      ? `<p class="t-small subtle">${esc(connectMessage)}</p>`
      : "",
    chatError ? `<p class="t-small result-error">${esc(chatError)}</p>` : "",
    phase === "sent"
      ? `<p class="t-small feishu-sent"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i>${esc(t("feishu.sent"))}</p>`
      : "",
  ].join("");
  return `
    <div class="feishu-group-action">
      <span class="feishu-destination-card__icon" aria-hidden="true">↗</span>
      ${control}
      <button type="button" class="btn-primary" data-feishu-send ${chatsLoading || chats.length === 0 || phase === "sending" || phase === "exporting" || phase === "connecting" ? "disabled" : ""}>${esc(phase === "sending" ? t("feishu.sending") : t("feishu.toGroup"))}</button>
    </div>
    ${status ? `<div class="feishu-group-status">${status}</div>` : ""}
  `;
}

function renderDisconnectConfirmation(): string {
  const account = accounts.find((item) => item.profile === selectedProfile);
  const name = account?.user_name || t("feishu.unknownAccount");
  return `
    <p class="t-body feishu-disconnect-copy">${esc(t("feishu.disconnectConfirm").replace("{name}", name))}</p>
    <div class="confirm-dialog-actions">
      <button type="button" class="btn-ghost" data-feishu-disconnect-cancel>${esc(t("common.cancel"))}</button>
      <button type="button" class="btn-primary btn-compact" data-feishu-disconnect-confirm>${esc(t("feishu.disconnect"))}</button>
    </div>
  `;
}

function renderProgress(message: string): string {
  return `
    <div class="feishu-progress">
      <i class="badge-dot badge-dot-ink badge-dot-pulse" aria-hidden="true"></i>
      <p class="t-body">${esc(message)}</p>
    </div>
  `;
}

function bindConnectEvents(shell: ShellState): void {
  if (eventsBound) return;
  eventsBound = true;
  void listen<FeishuConnectEvent>("feishu:connect", (event) => {
    const payload = event.payload;
    if (payload.state === "open_failed") {
      connectMessage = payload.url ?? t("feishu.authorizing");
    } else if (payload.stage === "app") {
      connectMessage = t("feishu.creatingApp");
    } else {
      connectMessage = t("feishu.authorizing");
    }
    if (phase === "connecting" || phase === "sending") shell.rerender();
  }).catch((error) => {
    console.error("feishu event listener failed:", error);
    eventsBound = false;
  });
}

async function openDialog(
  shell: ShellState,
  selectedTaskId: string,
  content: string,
): Promise<void> {
  operation += 1;
  taskId = selectedTaskId;
  answerContent = content;
  destination = null;
  documentResult = null;
  chats = [];
  chatsLoading = false;
  chatError = "";
  errorMessage = "";
  connectMessage = "";
  phase = "loading_accounts";
  shell.rerender();
  await loadAccounts(shell);
}

async function loadAccounts(shell: ShellState): Promise<void> {
  const currentOperation = ++operation;
  phase = "loading_accounts";
  errorMessage = "";
  shell.rerender();
  try {
    accounts = await invoke<FeishuAccount[]>("feishu_accounts");
    if (currentOperation !== operation) return;
    const remembered = localStorage.getItem(PROFILE_STORAGE_KEY) ?? "";
    const preferred =
      accounts.find((account) => account.profile === remembered) ??
      accounts.find((account) => account.active) ??
      accounts[0];
    selectedProfile = preferred?.profile ?? "";
    phase = "choose";
    shell.rerender();
    await refreshChats(shell, currentOperation);
  } catch (error) {
    if (currentOperation !== operation) return;
    errorMessage = `${error}`;
    phase = "error";
  } finally {
    if (currentOperation === operation) shell.rerender();
  }
}

async function selectAccount(shell: ShellState, profile: string): Promise<void> {
  const currentOperation = ++operation;
  selectedProfile = profile;
  phase = "loading_accounts";
  shell.rerender();
  try {
    await invoke<FeishuStatus>("feishu_select_account", { profile });
    if (currentOperation !== operation) return;
    localStorage.setItem(PROFILE_STORAGE_KEY, profile);
    accounts = accounts.map((account) => ({ ...account, active: account.profile === profile }));
    destination = null;
    documentResult = null;
    chats = [];
    phase = "choose";
    shell.rerender();
    await refreshChats(shell, currentOperation);
  } catch (error) {
    if (currentOperation !== operation) return;
    errorMessage = `${error}`;
    phase = "error";
  } finally {
    if (currentOperation === operation) shell.rerender();
  }
}

async function connectNewAccount(shell: ShellState): Promise<void> {
  const currentOperation = ++operation;
  phase = "connecting";
  connectMessage = t("feishu.creatingApp");
  errorMessage = "";
  shell.rerender();
  try {
    const status = await invoke<FeishuStatus>("feishu_connect", {
      profile: null,
      newAccount: true,
    });
    if (currentOperation !== operation) return;
    selectedProfile = status.profile;
    localStorage.setItem(PROFILE_STORAGE_KEY, selectedProfile);
    accounts = await invoke<FeishuAccount[]>("feishu_accounts");
    if (currentOperation !== operation) return;
    destination = null;
    documentResult = null;
    chats = [];
    phase = "choose";
    shell.rerender();
    await refreshChats(shell, currentOperation);
  } catch (error) {
    if (currentOperation !== operation) return;
    errorMessage = `${error}`;
    phase = "error";
  } finally {
    if (currentOperation === operation) shell.rerender();
  }
}

async function disconnectAccount(shell: ShellState): Promise<void> {
  if (!selectedProfile) return;
  const currentOperation = ++operation;
  phase = "disconnecting";
  shell.rerender();
  try {
    await invoke("feishu_disconnect_account", { profile: selectedProfile });
    if (currentOperation !== operation) return;
    accounts = await invoke<FeishuAccount[]>("feishu_accounts");
    if (currentOperation !== operation) return;
    const preferred = accounts.find((account) => account.active) ?? accounts[0];
    selectedProfile = preferred?.profile ?? "";
    if (selectedProfile) {
      localStorage.setItem(PROFILE_STORAGE_KEY, selectedProfile);
    } else {
      localStorage.removeItem(PROFILE_STORAGE_KEY);
    }
    destination = null;
    documentResult = null;
    chats = [];
    chatsLoading = false;
    phase = "choose";
    shell.rerender();
    await refreshChats(shell, currentOperation);
  } catch (error) {
    if (currentOperation !== operation) return;
    errorMessage = `${error}`;
    phase = "error";
  } finally {
    if (currentOperation === operation) shell.rerender();
  }
}

async function ensureConnected(currentOperation: number): Promise<string> {
  let profile = selectedProfile;
  if (profile) {
    const status = await invoke<FeishuStatus>("feishu_status", { profile });
    if (currentOperation !== operation) throw new Error("cancelled");
    if (!status.connected) {
      const connected = await invoke<FeishuStatus>("feishu_connect", {
        profile,
        newAccount: false,
      });
      profile = connected.profile;
    }
  } else {
    const connected = await invoke<FeishuStatus>("feishu_connect", {
      profile: null,
      newAccount: true,
    });
    profile = connected.profile;
  }
  if (currentOperation !== operation) throw new Error("cancelled");
  selectedProfile = profile;
  localStorage.setItem(PROFILE_STORAGE_KEY, profile);
  return profile;
}

async function createDocument(shell: ShellState): Promise<void> {
  if (!taskId || !answerContent) return;
  const currentOperation = ++operation;
  destination = "document";
  documentResult = null;
  errorMessage = "";
  chatError = "";
  connectMessage = t("feishu.exporting");
  phase = "connecting";
  shell.rerender();

  try {
    const profile = await ensureConnected(currentOperation);
    if (currentOperation !== operation) return;
    phase = "exporting";
    shell.rerender();
    documentResult = await invoke<FeishuDocument>("feishu_export_task", {
      taskId,
      profile,
      content: answerContent,
    });
    if (currentOperation !== operation) return;
    phase = "ready";
    shell.rerender();
    await invoke("open_external", { url: documentResult.url });
  } catch (error) {
    if (currentOperation !== operation || `${error}` === "Error: cancelled") return;
    errorMessage = `${error}`;
    phase = "error";
    shell.rerender();
  }
}

async function sendToChat(shell: ShellState): Promise<void> {
  const select = document.getElementById("feishu-chat-select") as HTMLSelectElement | null;
  const chatId = select?.value ?? "";
  if (!chatId || !taskId || !selectedProfile || !answerContent || phase === "sending") return;
  localStorage.setItem(CHAT_STORAGE_KEY, chatId);
  destination = "group";
  phase = "sending";
  chatError = "";
  connectMessage = "";
  shell.rerender();
  try {
    await invoke("feishu_send_task_to_chat", {
      taskId,
      profile: selectedProfile,
      content: answerContent,
      chatId,
    });
    phase = "sent";
  } catch (error) {
    chatError = `${error}`;
    phase = "choose";
  } finally {
    shell.rerender();
  }
}

async function refreshChats(shell: ShellState, currentOperation: number): Promise<void> {
  const account = accounts.find((item) => item.profile === selectedProfile);
  if (!selectedProfile || !account?.connected) {
    chats = [];
    chatsLoading = false;
    return;
  }
  chatsLoading = true;
  chats = [];
  chatError = "";
  shell.rerender();
  try {
    const result = await invoke<FeishuChat[]>("feishu_list_chats", {
      profile: selectedProfile,
    });
    if (currentOperation !== operation) return;
    chats = result;
  } catch (error) {
    if (currentOperation !== operation) return;
    chatError = `${error}`;
  } finally {
    if (currentOperation === operation) {
      chatsLoading = false;
      shell.rerender();
    }
  }
}

function isBusy(): boolean {
  return [
    "loading_accounts",
    "connecting",
    "exporting",
    "sending",
    "disconnecting",
  ].includes(phase);
}

function closeDialog(shell: ShellState): void {
  const cancelConnection = phase === "connecting";
  operation += 1;
  phase = "closed";
  shell.rerender();
  if (cancelConnection) {
    void invoke("feishu_cancel_connect").catch((error) => {
      console.error("failed to cancel feishu connection:", error);
    });
  }
}
