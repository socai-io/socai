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
}

interface WalletBalance {
  balance_points: number;
  points_per_cny: number;
  starter_points: number;
  active_until: string | null;
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
  let expiryTimer: number | null = null;
  let rerender: (() => void) | null = null;
  let wallet: WalletBalance | null = null;
  let walletUnavailable = false;
  let loginExpanded = false;
  let byokExpanded = false;
  let modelExpanded = false;
  let upgradeExpanded = false;

  export async function loadSession(): Promise<void> {
    phase = "loading";
    try {
      session = await invoke<AuthSession>("auth_session");
      error = "";
      if (session.logged_in) await refreshWallet();
    } catch (err) {
      console.error("auth_session failed:", err);
      session = EMPTY_SESSION;
      error = t("auth.sessionLoadFailed");
    } finally {
      phase = "idle";
    }
  }

  export async function refreshWallet(): Promise<void> {
    if (!session.logged_in) {
      wallet = null;
      walletUnavailable = false;
      stopExpiryTimer();
      return;
    }
    try {
      wallet = await invoke<WalletBalance>("billing_wallet");
      walletUnavailable = false;
      scheduleExpiryRefresh();
    } catch (err) {
      console.error("billing_wallet failed:", err);
      wallet = null;
      walletUnavailable = true;
    }
  }

  export function isOpen(): boolean {
    return open;
  }

  export function isLoggedIn(): boolean {
    return session.logged_in;
  }

  export function hasProAccess(): boolean {
    return activeUntilDate() !== null;
  }

  export function closePopover(): boolean {
    if (!open) return false;
    open = false;
    byokExpanded = false;
    modelExpanded = false;
    if (!challenge) loginExpanded = false;
    stopCountdown();
    return true;
  }

  export function render(
    modelLabel: string,
    modelConfigContent = "",
    subscriptionContent = "",
  ): string {
    const accountLabel = session.logged_in ? maskPhone(session.phone) : t("auth.loggedOut");
    const label = `${modelLabel} · ${accountLabel}`;
    const dot = session.logged_in ? "badge-dot-ink" : "badge-dot-hollow";
    return `
      <div class="auth-menu">
        <button
          id="auth-toggle"
          type="button"
          class="badge badge-button auth-chip"
          aria-label="${esc(session.logged_in ? t("auth.accountAria") : t("auth.loginAria"))}"
          aria-expanded="${open ? "true" : "false"}"
        ><i class="badge-dot ${dot}" aria-hidden="true"></i><span class="badge-text">${esc(label)}</span></button>
        ${open ? renderPopover(modelLabel, modelConfigContent, subscriptionContent) : ""}
      </div>
    `;
  }

  function renderPopover(
    modelLabel: string,
    modelConfigContent: string,
    subscriptionContent: string,
  ): string {
    if (phase === "loading") {
      return `<div class="topbar-popover auth-popover" role="dialog" aria-label="${esc(t("auth.title"))}"><p class="t-small subtle auth-copy">${esc(t("common.loading"))}</p></div>`;
    }
    return `
      <div class="topbar-popover auth-popover" role="dialog" aria-label="${esc(t("auth.title"))}">
        ${session.logged_in
          ? renderAccount(modelLabel, modelConfigContent, subscriptionContent)
          : renderSignedOut(modelConfigContent)}
        ${error ? `<p class="t-small result-error auth-error" role="alert">${esc(error)}</p>` : ""}
      </div>
    `;
  }

  function renderSignedOut(modelConfigContent: string): string {
    return `
      <section class="auth-choice">
        <div class="auth-choice-copy">
          <p class="t-small subtle">${esc(t("auth.loginAgentHint"))}</p>
        </div>
        <button id="auth-start-login" type="button" class="btn-primary btn-compact" aria-expanded="${loginExpanded || challenge ? "true" : "false"}">${esc(t("auth.login"))}</button>
      </section>
      ${loginExpanded || challenge ? `<div class="auth-expanded-panel">${challenge ? renderCodeForm() : renderPhoneForm()}</div>` : ""}
      <section class="auth-byok">
        <button id="auth-byok-toggle" type="button" class="auth-disclosure" aria-expanded="${byokExpanded ? "true" : "false"}">
          <span>${esc(t("auth.useOwnApiKey"))}</span>
          <span class="auth-disclosure-mark" aria-hidden="true">${byokExpanded ? "−" : "+"}</span>
        </button>
        ${byokExpanded ? `<div class="auth-expanded-panel">${modelConfigContent}</div>` : ""}
      </section>
    `;
  }

  function renderAccount(
    modelLabel: string,
    modelConfigContent: string,
    subscriptionContent: string,
  ): string {
    const activeUntil = activeUntilDate();
    return `
      <div class="auth-account-summary">
        <span class="t-mono">${esc(formatPhone(session.phone))}</span>
        <span class="badge"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i>${esc(t("auth.loggedIn"))}</span>
      </div>
      <div class="auth-balance-summary">
        <span class="t-small subtle">${esc(t("billing.remaining"))}</span>
        <span class="t-h2 auth-wallet-points">${wallet
          ? esc(t("billing.points", { points: wallet.balance_points }))
          : esc(walletUnavailable ? t("billing.unavailable") : t("common.loading"))}</span>
      </div>
      <section class="auth-byok auth-model-picker">
        <button id="auth-model-toggle" type="button" class="auth-disclosure" aria-expanded="${modelExpanded ? "true" : "false"}">
          <span>${esc(t("agent.label"))}</span>
          <span class="auth-disclosure-value">${esc(modelLabel)}</span>
          <span class="auth-disclosure-chevron ${modelExpanded ? "is-open" : ""}" aria-hidden="true">
            <svg viewBox="0 0 20 20"><path d="m5 7.5 5 5 5-5" /></svg>
          </span>
        </button>
        <p class="t-small subtle auth-model-hint">${esc(t("auth.useOwnApiKeyNoPoints"))}</p>
        ${modelExpanded ? modelConfigContent : ""}
      </section>
      ${activeUntil ? `
        <div class="auth-pro-status">
          <span class="badge"><i class="badge-dot badge-dot-ink" aria-hidden="true"></i>Pro</span>
          <span class="t-small subtle">${esc(t("billing.activeUntil", { date: formatDate(activeUntil) }))}</span>
        </div>
        <button id="auth-upgrade" type="button" class="btn-primary auth-full-button" aria-expanded="${upgradeExpanded ? "true" : "false"}">${esc(t("subscription.renewPro"))}</button>
      ` : `
        <button id="auth-upgrade" type="button" class="btn-primary auth-full-button" aria-expanded="${upgradeExpanded ? "true" : "false"}">${esc(t("subscription.upgradePro"))}</button>
        <ul class="auth-pro-benefits t-small">
          <li>${esc(t("subscription.proPoints"))}</li>
          <li>${esc(t("subscription.proXhs"))}</li>
          <li>${esc(t("subscription.proTranscript"))}</li>
        </ul>
      `}
      ${upgradeExpanded ? `<section class="auth-upgrade-panel" aria-label="${esc(t("subscription.upgradePro"))}">${subscriptionContent}</section>` : ""}
      <div class="auth-session-actions">
        <button id="auth-logout" type="button" class="auth-text-button" ${phase === "logging_out" ? "disabled" : ""}>${esc(phase === "logging_out" ? t("auth.loggingOut") : t("auth.logout"))}</button>
      </div>
    `;
  }

  function renderPhoneForm(): string {
    return `
      <div>
        <p class="settings-group-label">${esc(t("auth.loginTitle"))}</p>
      </div>
      <form id="auth-phone-form" class="auth-form">
        <div class="auth-phone-row">
          <span class="auth-country-code t-mono">+86</span>
          <input
            id="auth-phone"
            class="input-field auth-input"
            type="tel"
            inputmode="numeric"
            autocomplete="tel-national"
            aria-label="${esc(t("auth.phone"))}"
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
        <input
          id="auth-code"
          class="input-field auth-input auth-code-input"
          type="text"
          inputmode="numeric"
          autocomplete="one-time-code"
          aria-label="${esc(t("auth.code"))}"
          maxlength="6"
          placeholder="000000"
          value="${esc(code)}"
          ${phase === "verifying" || phase === "sending" ? "disabled" : ""}
        />
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
    rerender = shell.rerender;
    document.getElementById("auth-toggle")?.addEventListener("click", () => {
      const opening = !open;
      if (opening) onOpen();
      open = !open;
      error = "";
      shell.rerender();
      if (opening && session.logged_in) {
        void refreshWallet().then(shell.rerender);
      }
    });

    if (!open) return;

    if (session.logged_in) {
      document.getElementById("auth-model-toggle")?.addEventListener("click", () => {
        modelExpanded = !modelExpanded;
        upgradeExpanded = false;
        error = "";
        shell.rerender();
      });
      document.getElementById("auth-upgrade")?.addEventListener("click", () => {
        upgradeExpanded = !upgradeExpanded;
        modelExpanded = false;
        shell.rerender();
      });
      document.getElementById("auth-logout")?.addEventListener("click", () => {
        void logout(shell, onSessionChanged);
      });
      return;
    }

    document.getElementById("auth-start-login")?.addEventListener("click", () => {
      loginExpanded = true;
      byokExpanded = false;
      error = "";
      shell.rerender();
    });
    document.getElementById("auth-byok-toggle")?.addEventListener("click", () => {
      byokExpanded = !byokExpanded;
      if (byokExpanded && !challenge) loginExpanded = false;
      error = "";
      shell.rerender();
    });

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
      loginExpanded = true;
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
      code = "";
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
      loginExpanded = false;
      byokExpanded = false;
      modelExpanded = false;
      upgradeExpanded = false;
      stopExpiryTimer();
      stopCountdown();
      await refreshWallet();
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
      wallet = null;
      walletUnavailable = false;
      loginExpanded = false;
      byokExpanded = false;
      modelExpanded = false;
      upgradeExpanded = false;
      stopExpiryTimer();
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

  function scheduleExpiryRefresh(): void {
    stopExpiryTimer();
    const activeUntil = activeUntilDate();
    if (!activeUntil) return;
    const remaining = activeUntil.getTime() - Date.now();
    if (remaining <= 0) return;
    const delay = Math.min(remaining + 250, 2_147_483_647);
    expiryTimer = window.setTimeout(() => {
      expiryTimer = null;
      void refreshWallet().then(() => rerender?.());
    }, delay);
  }

  function stopExpiryTimer(): void {
    if (expiryTimer !== null) window.clearTimeout(expiryTimer);
    expiryTimer = null;
  }

  function activeUntilDate(): Date | null {
    if (!wallet?.active_until) return null;
    const date = new Date(wallet.active_until);
    if (Number.isNaN(date.getTime()) || date.getTime() <= Date.now()) return null;
    return date;
  }

  function formatDate(value: Date): string {
    return new Intl.DateTimeFormat(undefined, {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    }).format(value);
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
