//! Topbar settings menu. A gear button opens a popover that carries the
//! language toggle plus the preferences that mirror `~/.socai/config.json`
//! (output directory, chrome source/profile) and a display-only timezone.
//!
//! Settings save automatically: discrete controls (language, timezone, chrome
//! source) persist on change; path fields persist on commit (blur/enter). The
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

/** Mirrors the `DesktopConfig` returned by the `config_get` command. */
interface DesktopConfig {
  chrome_source: string;
  chrome_profile_dir: string;
  chrome_profile_dir_default: string;
  output_dir: string;
  output_dir_default: string;
  pro_activated: boolean;
  pro_device_id: string;
}

interface SettingsDraft {
  timezone: string;
  output_dir: string;
  chrome_source: string;
  chrome_profile_dir: string;
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
  let statusTimer: number | null = null;

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

  export function isOpen(): boolean {
    return open;
  }

  export function closePopover(): boolean {
    if (!open) return false;
    open = false;
    return true;
  }

  export function render(shell: ShellState): string {
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
        ${open ? renderPopover(shell) : ""}
      </div>
    `;
  }

  function renderPopover(shell: ShellState): string {
    if (loadError) {
      return `
        <div class="topbar-popover settings-popover" role="dialog" aria-label="${esc(t("settings.title"))}">
          <p class="t-small subtle result-error">${esc(t("settings.loadFailed"))}</p>
        </div>
      `;
    }
    if (!draft) seedDraft();
    if (!config || !draft) {
      return `
        <div class="topbar-popover settings-popover" role="dialog" aria-label="${esc(t("settings.title"))}">
          <p class="t-small subtle">${esc(t("common.loading"))}</p>
        </div>
      `;
    }
    return `
      <div class="topbar-popover settings-popover" role="dialog" aria-label="${esc(t("settings.title"))}">
        ${renderGeneralGroup(draft)}
        ${renderOutputGroup(config, draft)}
        ${renderProGroup(config, draft)}
        ${renderChromeGroup(shell, config, draft)}
        ${renderStatus()}
      </div>
    `;
  }

  function renderProGroup(c: DesktopConfig, d: SettingsDraft): string {
    const activation = c.pro_activated
      ? t("settings.proActivated")
      : t("settings.proNotActivated");
    const device = c.pro_device_id ? ` · ${c.pro_device_id.slice(0, 8)}` : "";
    return `
      <section class="settings-group">
        <div class="settings-field">
          <span class="settings-group-label">${esc(t("settings.pro"))}</span>
          <p class="t-small subtle settings-field-hint">${esc(activation)}${esc(device)}</p>
        </div>
        <div class="settings-field">
          <label class="t-small settings-field-label" for="settings-invite-code">${esc(t("settings.inviteCode"))}</label>
          <div class="settings-path-row">
            <input
              id="settings-invite-code"
              class="input-field settings-input-mono"
              type="text"
              spellcheck="false"
              value="${esc(d.invite_code)}"
            />
            <button type="button" class="btn-ghost btn-compact" data-settings-activate-pro ${status === "saving" ? "disabled" : ""}>${esc(t("settings.activate"))}</button>
          </div>
          <p class="t-small subtle settings-field-hint">${esc(t("settings.proHint"))}</p>
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

  function renderChromeGroup(shell: ShellState, c: DesktopConfig, d: SettingsDraft): string {
    const managed = d.chrome_source === "managed";
    const detail = managed
      ? `
        <div class="settings-field">
          <label class="t-small settings-field-label" for="settings-profile-dir">${esc(t("settings.profileDir"))}</label>
          <div class="settings-path-row">
            <input
              id="settings-profile-dir"
              class="input-field settings-input-mono"
              type="text"
              spellcheck="false"
              placeholder="${esc(c.chrome_profile_dir_default)}"
              value="${esc(d.chrome_profile_dir)}"
            />
            <button type="button" class="btn-ghost btn-compact" data-settings-browse="settings-profile-dir">${esc(t("settings.browse"))}</button>
          </div>
          <p class="t-small subtle settings-field-hint">${esc(t("settings.profileHint"))}</p>
        </div>
      `
      : renderEndpointField(shell);
    // "existing browser" is the product default, so it leads the toggle.
    return `
      <section class="settings-group">
        <p class="settings-group-label">${esc(t("settings.chrome"))}</p>
        <div class="settings-field">
          <span class="t-small settings-field-label">${esc(t("settings.source"))}</span>
          <div class="seg-toggle" role="group" aria-label="${esc(t("settings.source"))}">
            <button type="button" class="seg-toggle__button" data-settings-source="existing" aria-pressed="${managed ? "false" : "true"}">${esc(t("settings.sourceExisting"))}</button>
            <button type="button" class="seg-toggle__button" data-settings-source="managed" aria-pressed="${managed ? "true" : "false"}">${esc(t("settings.sourceManaged"))}</button>
          </div>
        </div>
        ${detail}
      </section>
    `;
  }

  // The "existing browser" endpoint is auto-discovered by the core (there is no
  // stored endpoint to edit), so the field is informational: show the live
  // endpoint when connected, otherwise a "not connected" placeholder.
  function renderEndpointField(shell: ShellState): string {
    const status = shell.status;
    const value = status.state === "connected" ? status.endpoint : t("settings.endpointDisconnected");
    return `
      <div class="settings-field">
        <span class="t-small settings-field-label">${esc(t("settings.endpoint"))}</span>
        <input class="input-field settings-input-mono" type="text" value="${esc(value)}" readonly disabled />
        <p class="t-small subtle settings-field-hint">${esc(t("settings.endpointHint"))}</p>
      </div>
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
            : t("settings.autosaveHint");
    return `<p class="t-small subtle settings-status${status === "error" ? " result-error" : ""}">${esc(text)}</p>`;
  }

  export function bind(shell: ShellState): void {
    document.getElementById("settings-toggle")?.addEventListener("click", (event) => {
      event.stopPropagation();
      open = !open;
      if (open) {
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
    bindPathField("settings-output-dir", "runs.dir", shell);
    bindPathField("settings-profile-dir", "chrome.profile_dir", shell);

    const invite = document.getElementById("settings-invite-code") as HTMLInputElement | null;
    invite?.addEventListener("input", () => {
      if (draft) draft.invite_code = invite.value;
    });
    document.querySelector<HTMLButtonElement>("[data-settings-activate-pro]")?.addEventListener("click", () => {
      void activatePro(shell);
    });

    document.querySelectorAll<HTMLButtonElement>("[data-settings-browse]").forEach((button) => {
      // No native directory picker is wired (no dialog plugin); focus the field
      // so the path stays directly editable.
      button.addEventListener("click", () => {
        const target = document.getElementById(button.dataset.settingsBrowse ?? "") as HTMLInputElement | null;
        target?.focus();
      });
    });

    document.querySelectorAll<HTMLButtonElement>("[data-settings-source]").forEach((button) => {
      button.addEventListener("click", () => {
        const next = button.dataset.settingsSource;
        if (next) void persistSource(next, shell);
      });
    });
  }

  function bindPathField(inputId: string, key: string, shell: ShellState): void {
    const input = document.getElementById(inputId) as HTMLInputElement | null;
    input?.addEventListener("change", () => {
      void persistPath(key, input.value, shell);
    });
  }

  function seedDraft(): void {
    draft = {
      timezone: getTimezone(),
      output_dir: config?.output_dir ?? "",
      chrome_source: config?.chrome_source || "existing",
      chrome_profile_dir: config?.chrome_profile_dir ?? "",
      invite_code: "",
    };
  }

  async function persistSource(value: string, shell: ShellState): Promise<void> {
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

  async function persistPath(key: string, raw: string, shell: ShellState): Promise<void> {
    if (!draft || !config) return;
    const value = raw.trim();
    const current = (key === "runs.dir" ? config.output_dir : config.chrome_profile_dir).trim();
    if (value === current) return; // no change vs the persisted value
    if (key === "runs.dir") draft.output_dir = value;
    else draft.chrome_profile_dir = value;
    status = "saving";
    shell.rerender();
    try {
      if (value) {
        await invoke("config_set", { key, value });
      } else {
        // Clearing a field reverts it to the product default.
        await invoke("config_unset", { key });
      }
      await loadConfig();
      seedDraft();
      flashSaved(shell);
    } catch (err) {
      console.error(`config write ${key} failed:`, err);
      await loadConfig();
      seedDraft();
      setError(shell);
    }
  }

  async function activatePro(shell: ShellState): Promise<void> {
    // An in-flight activation must not be doubled — each success consumes an
    // invite use and registers a new device.
    if (!draft || status === "saving") return;
    const inviteCode = draft.invite_code.trim();
    if (!inviteCode) {
      setError(shell);
      return;
    }
    status = "saving";
    shell.rerender();
    try {
      await invoke("pro_activate", { inviteCode, label: "desktop" });
      await loadConfig();
      seedDraft();
      flashSaved(shell);
    } catch (err) {
      console.error("pro_activate failed:", err);
      await loadConfig();
      seedDraft();
      setError(shell);
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
