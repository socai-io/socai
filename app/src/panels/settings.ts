//! Topbar settings menu. A gear button opens a popover that carries the
//! language toggle plus the output preference that mirrors
//! `~/.socai/config.json` and a display-only timezone. Chrome source selection
//! lives under the dedicated chrome status pill.
//!
//! Settings save automatically: discrete controls (language and timezone)
//! persist on change; path fields persist on commit (blur/enter). The
//! UI updates optimistically, then re-seeds from the persisted truth so it
//! reflects any normalization the core applies (e.g. relative → absolute paths).
//! Chrome + output write through the `config_set`/`config_unset` Tauri commands
//! (validated and saved by `socai_core::config`); timezone is a frontend-only
//! display preference kept in localStorage via `setTimezone`.

import { invoke } from "@tauri-apps/api/core";
import type { ShellState } from "../main";
import { esc } from "../lib/html";
import {
  getLanguage,
  getTimezone,
  isSupportedLanguage,
  setLanguage,
  setTimezone,
  t,
} from "../lib/i18n";
import { authMenu } from "./auth";

/** Mirrors the `DesktopConfig` returned by the `config_get` command. */
interface DesktopConfig {
  chrome_source: string;
  chrome_profile_dir: string;
  chrome_profile_dir_default: string;
  output_dir: string;
  output_dir_default: string;
}

interface SettingsDraft {
  timezone: string;
  output_dir: string;
  chrome_source: string;
  invite_code: string;
}

type SaveStatus = "" | "saving" | "saved" | "error";

// Curated IANA zones — the same shortlist the prototype offered. "system"
// (the local zone) is rendered separately as the first option.
const TIMEZONES = [
  "Asia/Shanghai",
  "Asia/Hong_Kong",
  "Asia/Tokyo",
  "Asia/Singapore",
  "Europe/London",
  "Europe/Berlin",
  "America/New_York",
  "America/Los_Angeles",
  "UTC",
];

const GEAR_SVG = `
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <circle cx="12" cy="12" r="3.2"></circle>
    <path d="M19.4 13a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1 1.56V21a2 2 0 1 1-4 0v-.09a1.7 1.7 0 0 0-1.11-1.56 1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.56-1H3a2 2 0 1 1 0-4h.09a1.7 1.7 0 0 0 1.56-1.11 1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34H9a1.7 1.7 0 0 0 1-1.56V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1 1.56 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87V9a1.7 1.7 0 0 0 1.56 1H21a2 2 0 1 1 0 4h-.09a1.7 1.7 0 0 0-1.51 1z"></path>
  </svg>
`;

export namespace settingsMenu {
  let open = false;
  let config: DesktopConfig | null = null;
  let loadError = false;
  let draft: SettingsDraft | null = null;
  let status: SaveStatus = "";
  let inviteMessage = "";
  let statusTimer: number | null = null;
  let appVersion = "";

  /** Installed app version — set once at startup from Tauri's `getVersion()`. */
  export function setAppVersion(version: string): void {
    appVersion = version.trim();
  }

  /** Load the persisted config once at startup. Best-effort: a failure leaves
   *  the menu in a "could not load" state without blocking the app. */
  export async function loadConfig(): Promise<void> {
    try {
      config = await invoke<DesktopConfig>("config_get");
      loadError = false;
    } catch (err) {
      console.error("config_get failed:", err);
      config = null;
      loadError = true;
    }
  }

  /// Whether the configured browser source is socai's remote hosted browser.
  /// The composer consults this: remote sessions are minted on demand at run
  /// start, so a disconnected status is routine there, not a setup problem.
  export function isRemoteProfile(): boolean {
    return (draft?.chrome_source ?? config?.chrome_source) === "remote" && authMenu.hasProAccess();
  }

  export function isRemoteSelected(): boolean {
    return (draft?.chrome_source ?? config?.chrome_source) === "remote";
  }

  export function isSaving(): boolean {
    return status === "saving";
  }

  /** Select the hosted browser after a user first gains Pro. This is called
   *  only for the purchase transition; later refreshes preserve any local
   *  browser choice the user makes. */
  export async function selectRemoteForNewPro(shell: ShellState): Promise<void> {
    if (!authMenu.hasProAccess()) return;
    if (!config) await loadConfig();
    if (!draft) seedDraft();
    await persistSource("remote", shell);
  }

  export function renderChromeManager(): string {
    if (loadError) return `<p class="t-small result-error">${esc(t("settings.loadFailed"))}</p>`;
    if (!draft) seedDraft();
    if (!config || !draft) return `<p class="t-small subtle">${esc(t("common.loading"))}</p>`;

    const managed = draft.chrome_source === "managed";
    const remote = draft.chrome_source === "remote";
    const hasPro = authMenu.hasProAccess();
    return `
      <div class="chrome-manager">
        <div class="seg-toggle chrome-manager-toggle" role="group" aria-label="${esc(t("settings.source"))}">
          <button type="button" class="seg-toggle__button" data-settings-source="existing" aria-pressed="${!managed && !remote ? "true" : "false"}" ${status === "saving" ? "disabled" : ""}>${esc(t("settings.sourceExisting"))}</button>
          <button type="button" class="seg-toggle__button" data-settings-source="managed" aria-pressed="${managed ? "true" : "false"}" ${status === "saving" ? "disabled" : ""}>${esc(t("settings.sourceManaged"))}</button>
          <button type="button" class="seg-toggle__button" data-settings-source="remote" aria-pressed="${remote ? "true" : "false"}" ${hasPro && status !== "saving" ? "" : "disabled"}>${esc(t("settings.sourceRemotePro"))}</button>
        </div>
      </div>
    `;
  }

  export function isOpen(): boolean {
    return open;
  }

  export function closePopover(): boolean {
    if (!open) return false;
    open = false;
    return true;
  }

  export function render(_shell: ShellState): string {
    const expanded = open ? "true" : "false";
    return `
      <div class="settings-menu">
        <button
          id="settings-toggle"
          type="button"
          class="icon-button ${open ? "icon-button-active" : ""}"
          aria-label="${esc(t("settings.aria"))}"
          aria-expanded="${expanded}"
        >${GEAR_SVG}</button>
        ${open ? renderPopover() : ""}
      </div>
    `;
  }

  function renderPopover(): string {
    if (loadError) {
      return `
        <div class="topbar-popover settings-popover" role="dialog" aria-label="${esc(t("settings.title"))}">
          <p class="t-small subtle result-error">${esc(t("settings.loadFailed"))}</p>
          ${renderVersionFooter()}
        </div>
      `;
    }
    if (!draft) seedDraft();
    if (!config || !draft) {
      return `
        <div class="topbar-popover settings-popover" role="dialog" aria-label="${esc(t("settings.title"))}">
          <p class="t-small subtle">${esc(t("common.loading"))}</p>
          ${renderVersionFooter()}
        </div>
      `;
    }
    return `
      <div class="topbar-popover settings-popover" role="dialog" aria-label="${esc(t("settings.title"))}">
        ${renderGeneralGroup(draft)}
        ${renderOutputGroup(config, draft)}
        ${renderInviteGroup(draft)}
        ${renderStatus()}
        ${renderVersionFooter()}
      </div>
    `;
  }

  function renderVersionFooter(): string {
    if (!appVersion) return "";
    return `<p class="settings-version" aria-label="${esc(t("settings.version"))}">${esc(appVersion)}</p>`;
  }

  function renderInviteGroup(d: SettingsDraft): string {
    return `
      <section class="settings-group">
        <div class="settings-field">
          <label class="t-small settings-field-label" for="settings-invite-code">${esc(t("settings.inviteCode"))}</label>
          <div class="settings-path-row">
            <input
              id="settings-invite-code"
              class="input-field settings-input-mono"
              type="text"
              spellcheck="false"
              autocomplete="off"
              value="${esc(d.invite_code)}"
            />
            <button type="button" class="btn-ghost btn-compact" data-settings-redeem-invite ${status === "saving" ? "disabled" : ""}>${esc(t("settings.enter"))}</button>
          </div>
          ${inviteMessage ? `<p id="settings-invite-message" class="t-small result-error settings-field-hint">${esc(inviteMessage)}</p>` : ""}
        </div>
      </section>
    `;
  }

  function renderGeneralGroup(d: SettingsDraft): string {
    const language = getLanguage();
    const zone = localZone();
    const systemLabel = zone ? `${t("settings.timezoneSystem")} · ${zone}` : t("settings.timezoneSystem");
    const options = [
      `<option value="system" ${d.timezone === "system" ? "selected" : ""}>${esc(systemLabel)}</option>`,
      ...TIMEZONES.map(
        (tz) => `<option value="${esc(tz)}" ${d.timezone === tz ? "selected" : ""}>${esc(tz)}</option>`,
      ),
    ].join("");
    return `
      <section class="settings-group">
        <p class="settings-group-label">${esc(t("settings.general"))}</p>
        <div class="settings-row">
          <span class="t-small settings-row-label">${esc(t("settings.language"))}</span>
          <div class="language-toggle" role="group" aria-label="${esc(t("language.switcherAria"))}">
            <button class="language-toggle__button" type="button" data-settings-lang="zh" aria-pressed="${language === "zh" ? "true" : "false"}">中文</button>
            <button class="language-toggle__button" type="button" data-settings-lang="en" aria-pressed="${language === "en" ? "true" : "false"}">en</button>
          </div>
        </div>
        <div class="settings-row">
          <label class="t-small settings-row-label" for="settings-timezone">${esc(t("settings.timezone"))}</label>
          <select id="settings-timezone" class="input-field settings-row-select">${options}</select>
        </div>
      </section>
    `;
  }

  function renderOutputGroup(c: DesktopConfig, d: SettingsDraft): string {
    return `
      <section class="settings-group">
        <p class="settings-group-label">${esc(t("settings.output"))}</p>
        <div class="settings-field">
          <label class="t-small settings-field-label" for="settings-output-dir">${esc(t("settings.outputDir"))}</label>
          <div class="settings-path-row">
            <input
              id="settings-output-dir"
              class="input-field settings-input-mono"
              type="text"
              spellcheck="false"
              placeholder="${esc(c.output_dir_default)}"
              value="${esc(d.output_dir)}"
            />
            <button type="button" class="btn-ghost btn-compact" data-settings-browse="settings-output-dir">${esc(t("settings.browse"))}</button>
          </div>
          <p class="t-small subtle settings-field-hint">${esc(t("settings.outputHint"))}</p>
        </div>
      </section>
    `;
  }

  function renderStatus(): string {
    const text =
      status === "saving"
        ? t("common.saving")
        : status === "saved"
          ? t("settings.saved")
          : status === "error"
            ? t("settings.saveFailed")
            : "";
    if (!text) return "";
    return `<p class="t-small subtle settings-status${status === "error" ? " result-error" : ""}">${esc(text)}</p>`;
  }

  export function bind(
    shell: ShellState,
    onOpen: () => void = () => {},
    onInviteRedeemed: () => Promise<void> = async () => {},
  ): void {
    document.getElementById("settings-toggle")?.addEventListener("click", (event) => {
      event.stopPropagation();
      if (!open) onOpen();
      open = !open;
      if (open) {
        inviteMessage = "";
        // Reopening retries a failed initial load so the error state isn't a
        // dead-end (the in-menu controls that re-fetch don't render on failure).
        if (loadError || !config) {
          void loadConfig().then(() => {
            seedDraft();
            shell.rerender();
          });
        }
        seedDraft();
      }
      shell.rerender();
    });

    document.querySelectorAll<HTMLButtonElement>("[data-settings-source]").forEach((button) => {
      button.addEventListener("click", () => {
        const next = button.dataset.settingsSource;
        if (next) void persistSource(next, shell);
      });
    });

    if (!open || !draft) return;

    document.querySelectorAll<HTMLButtonElement>("[data-settings-lang]").forEach((button) => {
      button.addEventListener("click", () => {
        const next = button.dataset.settingsLang;
        if (!isSupportedLanguage(next) || getLanguage() === next) return;
        setLanguage(next);
        shell.rerender();
      });
    });

    const timezone = document.getElementById("settings-timezone") as HTMLSelectElement | null;
    timezone?.addEventListener("change", () => {
      if (!draft) return;
      draft.timezone = timezone.value;
      setTimezone(timezone.value);
      // Re-render the whole shell so task timestamps everywhere pick up the zone.
      flashSaved(shell);
    });

    // Path fields commit on change (blur/enter), not per keystroke.
    bindPathField("settings-output-dir", shell);

    const invite = document.getElementById("settings-invite-code") as HTMLInputElement | null;
    invite?.addEventListener("input", () => {
      if (draft) draft.invite_code = invite.value;
      inviteMessage = "";
      document.getElementById("settings-invite-message")?.remove();
    });
    document.querySelector<HTMLButtonElement>("[data-settings-redeem-invite]")?.addEventListener("click", () => {
      void redeemInvite(shell, onInviteRedeemed);
    });

    document.querySelectorAll<HTMLButtonElement>("[data-settings-browse]").forEach((button) => {
      // No native directory picker is wired (no dialog plugin); focus the field
      // so the path stays directly editable.
      button.addEventListener("click", () => {
        const target = document.getElementById(button.dataset.settingsBrowse ?? "") as HTMLInputElement | null;
        target?.focus();
      });
    });

  }

  function bindPathField(inputId: string, shell: ShellState): void {
    const input = document.getElementById(inputId) as HTMLInputElement | null;
    input?.addEventListener("change", () => {
      void persistOutputPath(input.value, shell);
    });
  }

  function seedDraft(): void {
    draft = {
      timezone: getTimezone(),
      output_dir: config?.output_dir ?? "",
      chrome_source: config?.chrome_source || "existing",
      invite_code: "",
    };
  }

  async function persistSource(value: string, shell: ShellState): Promise<void> {
    if (value === "remote" && !authMenu.hasProAccess()) return;
    if (!draft || draft.chrome_source === value) return;
    draft.chrome_source = value; // optimistic — toggle + sub-field update immediately
    status = "saving";
    shell.rerender();
    try {
      await invoke("config_set", { key: "chrome.profile", value });
      await loadConfig();
      seedDraft();
      flashSaved(shell);
    } catch (err) {
      console.error("config_set chrome.profile failed:", err);
      await loadConfig();
      seedDraft();
      setError(shell);
    }
  }

  async function persistOutputPath(raw: string, shell: ShellState): Promise<void> {
    if (!draft || !config) return;
    const value = raw.trim();
    const current = config.output_dir.trim();
    if (value === current) return; // no change vs the persisted value
    draft.output_dir = value;
    status = "saving";
    shell.rerender();
    try {
      if (value) {
        await invoke("config_set", { key: "runs.dir", value });
      } else {
        // Clearing a field reverts it to the product default.
        await invoke("config_unset", { key: "runs.dir" });
      }
      await loadConfig();
      seedDraft();
      flashSaved(shell);
    } catch (err) {
      console.error("config write runs.dir failed:", err);
      await loadConfig();
      seedDraft();
      setError(shell);
    }
  }

  async function redeemInvite(
    shell: ShellState,
    onInviteRedeemed: () => Promise<void>,
  ): Promise<void> {
    if (!draft || status === "saving") return;
    if (!authMenu.isLoggedIn()) {
      inviteMessage = t("settings.loginForInvite");
      shell.rerender();
      return;
    }
    const inviteCode = draft.invite_code.trim();
    if (!inviteCode) {
      inviteMessage = t("settings.inviteRequired");
      shell.rerender();
      return;
    }
    inviteMessage = "";
    status = "saving";
    shell.rerender();
    try {
      await invoke("pro_activate", { inviteCode, label: "desktop" });
      await loadConfig();
      seedDraft();
      await onInviteRedeemed();
      flashSaved(shell);
    } catch (err) {
      console.error("pro_activate failed:", err);
      await loadConfig();
      seedDraft();
      status = "";
      inviteMessage = t("settings.inviteInvalid");
      shell.rerender();
    }
  }

  function flashSaved(shell: ShellState): void {
    status = "saved";
    if (statusTimer !== null) window.clearTimeout(statusTimer);
    statusTimer = window.setTimeout(() => {
      status = "";
      shell.rerender();
    }, 1600);
    shell.rerender();
  }

  function setError(shell: ShellState): void {
    status = "error";
    if (statusTimer !== null) window.clearTimeout(statusTimer);
    statusTimer = window.setTimeout(() => {
      status = "";
      shell.rerender();
    }, 2600);
    shell.rerender();
  }

  // The local zone the "system (local)" option resolves to, e.g. "Asia/Shanghai".
  function localZone(): string {
    try {
      return Intl.DateTimeFormat().resolvedOptions().timeZone || "";
    } catch {
      return "";
    }
  }
}
