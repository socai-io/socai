//! Compact account control for the desktop topbar. Phone login uses the
//! server's SMS challenge flow; the returned device token is persisted by the
//! Rust core and shared with existing managed cloud features.

import { invoke } from "@tauri-apps/api/core";
import type { ShellState } from "../main";
import { esc } from "../lib/html";
import { t } from "../lib/i18n";

interface AuthSession {
  logged_in: boolean;
  user_id: string;
  phone: string;
  device_id: string;
  status: string;
}

interface SmsChallenge {
  challenge_id: string;
  expires_in_seconds: number;
  retry_after_seconds: number;
  debug_code?: string | null;
}

type AuthPhase = "loading" | "idle" | "sending" | "verifying" | "logging_out";

const EMPTY_SESSION: AuthSession = {
  logged_in: false,
  user_id: "",
  phone: "",
  device_id: "",
  status: "",
};

export namespace authMenu {
  let open = false;
  let phase: AuthPhase = "loading";
  let session: AuthSession = EMPTY_SESSION;
  let challenge: SmsChallenge | null = null;
  let phone = "";
  let code = "";
  let retryAt = 0;
  let error = "";
  let countdownTimer: number | null = null;

  export async function loadSession(): Promise<void> {
    phase = "loading";
    try {
      session = await invoke<AuthSession>("auth_session");
      error = "";
    } catch (err) {
      console.error("auth_session failed:", err);
      session = EMPTY_SESSION;
      error = t("auth.sessionLoadFailed");
    } finally {
      phase = "idle";
    }
  }

  export function isOpen(): boolean {
    return open;
  }

  export function closePopover(): boolean {
    if (!open) return false;
    open = false;
    stopCountdown();
    return true;
  }

  export function render(): string {
    const label = session.logged_in ? maskPhone(session.phone) : t("auth.login");
    const dot = session.logged_in ? "badge-dot-ink" : "badge-dot-hollow";
    return `
      <div class="auth-menu">
        <button
          id="auth-toggle"
          type="button"
          class="badge badge-button auth-chip"
          aria-label="${esc(session.logged_in ? t("auth.accountAria") : t("auth.loginAria"))}"
          aria-expanded="${open ? "true" : "false"}"
        ><i class="badge-dot ${dot}" aria-hidden="true"></i>${esc(label)}</button>
        ${open ? renderPopover() : ""}
      </div>
    `;
  }

  function renderPopover(): string {
    if (phase === "loading") {
      return `<div class="topbar-popover auth-popover" role="dialog" aria-label="${esc(t("auth.title"))}"><p class="t-small subtle auth-copy">${esc(t("common.loading"))}</p></div>`;
    }
    return `
      <div class="topbar-popover auth-popover" role="dialog" aria-label="${esc(t("auth.title"))}">
        ${session.logged_in ? renderAccount() : challenge ? renderCodeForm() : renderPhoneForm()}
        ${error ? `<p class="t-small result-error auth-error" role="alert">${esc(error)}</p>` : ""}
      </div>
    `;
  }

  function renderAccount(): string {
    return `
      <div class="auth-heading">
        <div>
          <p class="settings-group-label">${esc(t("auth.account"))}</p>
          <p class="t-small subtle auth-copy">${esc(t("auth.accountHint"))}</p>
        </div>
        <span class="badge"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i>${esc(t("auth.loggedIn"))}</span>
      </div>
      <div class="auth-account-row">
        <span class="t-small subtle">${esc(t("auth.phone"))}</span>
        <span class="t-mono">${esc(formatPhone(session.phone))}</span>
      </div>
      <button id="auth-logout" type="button" class="btn-ghost auth-full-button" ${phase === "logging_out" ? "disabled" : ""}>${esc(phase === "logging_out" ? t("auth.loggingOut") : t("auth.logout"))}</button>
    `;
  }

  function renderPhoneForm(): string {
    return `
      <div>
        <p class="settings-group-label">${esc(t("auth.loginTitle"))}</p>
        <p class="t-small subtle auth-copy">${esc(t("auth.loginHint"))}</p>
      </div>
      <form id="auth-phone-form" class="auth-form">
        <label class="t-small settings-field-label" for="auth-phone">${esc(t("auth.phone"))}</label>
        <div class="auth-phone-row">
          <span class="auth-country-code t-mono">+86</span>
          <input
            id="auth-phone"
            class="input-field auth-input"
            type="tel"
            inputmode="numeric"
            autocomplete="tel-national"
            maxlength="13"
            placeholder="138 0000 0000"
            value="${esc(displayNationalPhone(phone))}"
            ${phase === "sending" ? "disabled" : ""}
          />
        </div>
        <button type="submit" class="btn-primary auth-full-button" ${phase === "sending" ? "disabled" : ""}>${esc(phase === "sending" ? t("auth.sending") : t("auth.sendCode"))}</button>
      </form>
    `;
  }

  function renderCodeForm(): string {
    const seconds = retrySeconds();
    return `
      <div>
        <p class="settings-group-label">${esc(t("auth.enterCode"))}</p>
        <p class="t-small subtle auth-copy">${esc(t("auth.codeSent", { phone: formatPhone(phone) }))}</p>
      </div>
      <form id="auth-code-form" class="auth-form">
        <label class="t-small settings-field-label" for="auth-code">${esc(t("auth.code"))}</label>
        <input
          id="auth-code"
          class="input-field auth-input auth-code-input"
          type="text"
          inputmode="numeric"
          autocomplete="one-time-code"
          maxlength="6"
          placeholder="000000"
          value="${esc(code)}"
          ${phase === "verifying" || phase === "sending" ? "disabled" : ""}
        />
        ${challenge?.debug_code ? `<p class="t-small subtle auth-copy">${esc(t("auth.debugCode", { code: challenge.debug_code }))}</p>` : ""}
        <button type="submit" class="btn-primary auth-full-button" ${phase === "verifying" || phase === "sending" ? "disabled" : ""}>${esc(phase === "verifying" ? t("auth.verifying") : t("auth.login"))}</button>
      </form>
      <div class="auth-secondary-actions">
        <button id="auth-change-phone" type="button" class="auth-text-button" ${phase === "sending" ? "disabled" : ""}>${esc(t("auth.changePhone"))}</button>
        <button id="auth-resend" type="button" class="auth-text-button" ${seconds > 0 || phase === "sending" ? "disabled" : ""}>${esc(seconds > 0 ? t("auth.resendCountdown", { seconds }) : t("auth.resend"))}</button>
      </div>
    `;
  }

  export function bind(
    shell: ShellState,
    onOpen: () => void,
    onSessionChanged: () => Promise<void>,
  ): void {
    document.getElementById("auth-toggle")?.addEventListener("click", () => {
      if (!open) onOpen();
      open = !open;
      error = "";
      shell.rerender();
    });

    if (!open) return;

    if (session.logged_in) {
      document.getElementById("auth-logout")?.addEventListener("click", () => {
        void logout(shell, onSessionChanged);
      });
      return;
    }

    const phoneInput = document.getElementById("auth-phone") as HTMLInputElement | null;
    phoneInput?.addEventListener("input", () => {
      phone = phoneInput.value;
      error = "";
    });
    document.getElementById("auth-phone-form")?.addEventListener("submit", (event) => {
      event.preventDefault();
      void sendCode(shell);
    });

    const codeInput = document.getElementById("auth-code") as HTMLInputElement | null;
    codeInput?.addEventListener("input", () => {
      code = codeInput.value.replace(/\D/g, "").slice(0, 6);
      if (codeInput.value !== code) codeInput.value = code;
      error = "";
    });
    document.getElementById("auth-code-form")?.addEventListener("submit", (event) => {
      event.preventDefault();
      void verifyCode(shell, onSessionChanged);
    });
    document.getElementById("auth-change-phone")?.addEventListener("click", () => {
      challenge = null;
      code = "";
      error = "";
      retryAt = 0;
      stopCountdown();
      shell.rerender();
    });
    document.getElementById("auth-resend")?.addEventListener("click", () => {
      void sendCode(shell);
    });
    if (challenge) startCountdown();
  }

  async function sendCode(shell: ShellState): Promise<void> {
    if (phase !== "idle") return;
    if (!isValidPhone(phone)) {
      error = t("auth.invalidPhone");
      shell.rerender();
      return;
    }
    phase = "sending";
    error = "";
    shell.rerender();
    try {
      challenge = await invoke<SmsChallenge>("auth_sms_send", { phone });
      code = challenge.debug_code ?? "";
      retryAt = Date.now() + Math.max(0, challenge.retry_after_seconds) * 1000;
    } catch (err) {
      console.error("auth_sms_send failed:", err);
      error = friendlyError(err);
    } finally {
      phase = "idle";
      shell.rerender();
    }
  }

  async function verifyCode(shell: ShellState, onSessionChanged: () => Promise<void>): Promise<void> {
    if (!challenge || phase !== "idle") return;
    if (!/^\d{6}$/.test(code)) {
      error = t("auth.invalidCodeFormat");
      shell.rerender();
      return;
    }
    phase = "verifying";
    error = "";
    shell.rerender();
    try {
      session = await invoke<AuthSession>("auth_sms_verify", {
        challengeId: challenge.challenge_id,
        phone,
        code,
      });
      challenge = null;
      code = "";
      retryAt = 0;
      stopCountdown();
      await onSessionChanged();
    } catch (err) {
      console.error("auth_sms_verify failed:", err);
      error = friendlyError(err);
    } finally {
      phase = "idle";
      shell.rerender();
    }
  }

  async function logout(shell: ShellState, onSessionChanged: () => Promise<void>): Promise<void> {
    if (phase === "logging_out") return;
    phase = "logging_out";
    error = "";
    shell.rerender();
    try {
      await invoke("auth_logout");
    } catch (err) {
      // The Rust side clears the local token even when remote revocation fails.
      console.error("auth_logout failed:", err);
      error = friendlyError(err);
    } finally {
      session = EMPTY_SESSION;
      phase = "idle";
      await onSessionChanged();
      shell.rerender();
    }
  }

  function startCountdown(): void {
    stopCountdown();
    if (retrySeconds() <= 0) return;
    countdownTimer = window.setInterval(() => {
      const button = document.getElementById("auth-resend") as HTMLButtonElement | null;
      if (!button) {
        stopCountdown();
        return;
      }
      const seconds = retrySeconds();
      button.disabled = seconds > 0;
      button.textContent = seconds > 0 ? t("auth.resendCountdown", { seconds }) : t("auth.resend");
      if (seconds <= 0) stopCountdown();
    }, 1000);
  }

  function stopCountdown(): void {
    if (countdownTimer !== null) window.clearInterval(countdownTimer);
    countdownTimer = null;
  }

  function retrySeconds(): number {
    return Math.max(0, Math.ceil((retryAt - Date.now()) / 1000));
  }

  function normalizedNationalPhone(value: string): string {
    let compact = value.replace(/[\s()-]/g, "");
    if (compact.startsWith("+86")) compact = compact.slice(3);
    else if (compact.length === 13 && compact.startsWith("86")) compact = compact.slice(2);
    return compact;
  }

  function isValidPhone(value: string): boolean {
    return /^1[3-9]\d{9}$/.test(normalizedNationalPhone(value));
  }

  function displayNationalPhone(value: string): string {
    return normalizedNationalPhone(value);
  }

  function formatPhone(value: string): string {
    const national = normalizedNationalPhone(value);
    if (national.length !== 11) return value;
    return `+86 ${national.slice(0, 3)} ${national.slice(3, 7)} ${national.slice(7)}`;
  }

  function maskPhone(value: string): string {
    const national = normalizedNationalPhone(value);
    if (national.length !== 11) return t("auth.loggedIn");
    return `${national.slice(0, 3)}****${national.slice(-4)}`;
  }

  function friendlyError(value: unknown): string {
    const message = String(value).toLowerCase();
    if (message.includes("invalid mainland china phone")) return t("auth.invalidPhone");
    if (message.includes("too frequently") || message.includes("429")) return t("auth.tooFrequent");
    if (message.includes("invalid sms code")) return t("auth.invalidCode");
    if (message.includes("expired")) return t("auth.expiredCode");
    if (message.includes("already used")) return t("auth.usedCode");
    if (message.includes("too many verification")) return t("auth.tooManyAttempts");
    if (message.includes("server url is not configured")) return t("auth.serverNotConfigured");
    return t("auth.requestFailed");
  }
}
