import { invoke } from "@tauri-apps/api/core";
import QRCode from "qrcode";

import type { ShellState } from "../main";
import { esc } from "../lib/html";
import { t } from "../lib/i18n";

interface PaymentPlan {
  enabled: boolean;
  wechat_enabled: boolean;
  alipay_enabled: boolean;
  plan_id: string;
  name: string;
  amount_fen: number;
  points: number;
  duration_days: number;
  auto_renews: boolean;
}

interface PaymentOrder {
  order_id: string;
  status: string;
  code_url: string | null;
  payment_url: string | null;
  amount_fen: number;
  points: number;
  duration_days: number;
  expires_at: string | null;
  paid_at: string | null;
  active_until: string | null;
}

type Phase = "idle" | "loading" | "creating" | "waiting" | "paid";
type PaymentProvider = "wechat" | "alipay";

export namespace subscriptionMenu {
  let phase: Phase = "idle";
  let plan: PaymentPlan | null = null;
  let order: PaymentOrder | null = null;
  let qrDataUrl = "";
  let paymentProvider: PaymentProvider | null = null;
  let error = "";
  let pollTimer: number | null = null;
  let polling = false;
  let walletChanged: (() => Promise<void>) | null = null;

  export async function refresh(loggedIn: boolean): Promise<void> {
    if (!loggedIn) {
      plan = null;
      order = null;
      qrDataUrl = "";
      paymentProvider = null;
      phase = "idle";
      error = "";
      stopPolling();
      return;
    }
    if (phase === "idle") phase = "loading";
    try {
      plan = await invoke<PaymentPlan>("billing_plan");
      error = "";
    } catch (err) {
      console.error("billing_plan failed:", err);
      plan = null;
      error = t("subscription.loadFailed");
    } finally {
      if (phase === "loading") phase = "idle";
    }
  }

  export function render(): string {
    return `
      <div class="subscription-content">
        ${renderContent()}
        ${error ? `<p class="t-small result-error subscription-error" role="alert">${esc(error)}</p>` : ""}
      </div>
    `;
  }

  function renderContent(): string {
    if ((phase === "loading" || plan === null) && !error) {
      return `<p class="t-small subtle subscription-copy">${esc(t("common.loading"))}</p>`;
    }
    if (phase === "paid" && order) {
      return `
        <div class="subscription-success-mark" aria-hidden="true">✓</div>
        <div class="subscription-centered">
          <p class="t-h2 subscription-success-title">${esc(t("subscription.success"))}</p>
          <p class="t-small subtle subscription-copy">${esc(t("subscription.successHint", {
            points: order.points,
            date: formatDate(order.active_until),
          }))}</p>
        </div>
        <button id="subscription-done" type="button" class="btn-primary subscription-full-button">${esc(t("subscription.done"))}</button>
      `;
    }
    if ((phase === "waiting" || phase === "creating") && order) {
      return renderCheckout(order);
    }
    if (!plan?.enabled) {
      return `<p class="t-small subtle subscription-copy">${esc(t("subscription.unavailable"))}</p>`;
    }
    return `
      <div class="subscription-plan-head">
        <p class="t-h2 subscription-plan-name">${esc(plan.name)}</p>
        <p class="t-h2 subscription-price">${esc(formatCny(plan.amount_fen))}</p>
      </div>
      <div class="subscription-plan-details">
        <div><span class="t-small">${esc(plan.duration_days === 30
          ? t("subscription.oneMonth")
          : t("subscription.days", { days: plan.duration_days }))}</span></div>
      </div>
      <div class="subscription-payment-options">
        ${plan.wechat_enabled ? `<button id="subscription-buy-wechat" type="button" class="btn-primary subscription-full-button">${esc(t("subscription.wechatPay"))}</button>` : ""}
        ${plan.alipay_enabled ? `<button id="subscription-buy-alipay" type="button" class="${plan.wechat_enabled ? "btn-ghost" : "btn-primary"} subscription-full-button">${esc(t("subscription.alipay"))}</button>` : ""}
      </div>
    `;
  }

  function renderCheckout(value: PaymentOrder): string {
    const isAlipay = paymentProvider === "alipay";
    const waitingForQr = phase === "creating" || !qrDataUrl;
    return `
      <div class="subscription-checkout-head">
        <div>
          <p class="t-eyebrow">${esc(t(isAlipay ? "subscription.alipay" : "subscription.wechatPay"))}</p>
          <p class="t-h2 subscription-price">${esc(formatCny(value.amount_fen))}</p>
        </div>
        <span class="badge"><i class="badge-dot badge-dot-hollow" aria-hidden="true"></i>${esc(t("subscription.awaitingPayment"))}</span>
      </div>
      ${isAlipay ? `
        <div class="subscription-browser-payment">
          <span class="subscription-browser-glyph" aria-hidden="true">↗</span>
          <p class="t-small">${esc(t("subscription.alipayOpened"))}</p>
        </div>
        <button id="subscription-open-payment" type="button" class="btn-primary subscription-full-button">${esc(t("subscription.openAlipay"))}</button>
      ` : `
        <div class="subscription-qr-wrap">
          ${waitingForQr
            ? `<p class="t-small subtle">${esc(t("common.loading"))}</p>`
            : `<img class="subscription-qr" src="${esc(qrDataUrl)}" alt="${esc(t("subscription.qrAria"))}" />`}
        </div>
        <p class="t-small subscription-scan-hint">${esc(t("subscription.scanHint"))}</p>
      `}
      <p class="t-small subtle subscription-copy">${esc(t("subscription.expires", { time: formatTime(value.expires_at) }))}</p>
      <button id="subscription-cancel" type="button" class="btn-ghost subscription-full-button">${esc(t("common.cancel"))}</button>
    `;
  }

  export function bind(shell: ShellState, onWalletChanged: () => Promise<void>): void {
    walletChanged = onWalletChanged;
    document.getElementById("subscription-buy-wechat")?.addEventListener("click", () => {
      void createOrder("wechat", shell);
    });
    document.getElementById("subscription-buy-alipay")?.addEventListener("click", () => {
      void createOrder("alipay", shell);
    });
    document.getElementById("subscription-open-payment")?.addEventListener("click", () => {
      if (order?.payment_url) void openPaymentUrl(order.payment_url);
    });
    document.getElementById("subscription-cancel")?.addEventListener("click", () => {
      order = null;
      qrDataUrl = "";
      paymentProvider = null;
      phase = "idle";
      stopPolling();
      shell.rerender();
    });
    document.getElementById("subscription-done")?.addEventListener("click", () => {
      order = null;
      qrDataUrl = "";
      paymentProvider = null;
      phase = "idle";
      shell.rerender();
    });
  }

  async function createOrder(provider: PaymentProvider, shell: ShellState): Promise<void> {
    if (!plan || phase === "creating") return;
    phase = "creating";
    error = "";
    qrDataUrl = "";
    paymentProvider = provider;
    shell.rerender();
    try {
      const requestId = typeof crypto.randomUUID === "function"
        ? crypto.randomUUID()
        : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
      const command = provider === "wechat"
        ? "billing_create_wechat_order"
        : "billing_create_alipay_order";
      order = await invoke<PaymentOrder>(command, {
        planId: plan.plan_id,
        requestId,
      });
      if (provider === "wechat") {
        if (!order.code_url) throw new Error("payment order has no code_url");
        qrDataUrl = await QRCode.toDataURL(order.code_url, {
          errorCorrectionLevel: "M",
          margin: 2,
          width: 220,
          color: { dark: "#171717", light: "#ffffff" },
        });
      } else {
        if (!order.payment_url) throw new Error("payment order has no payment_url");
        await openPaymentUrl(order.payment_url);
      }
      phase = "waiting";
      startPolling(shell);
    } catch (err) {
      console.error(`billing_create_${provider}_order failed:`, err);
      order = null;
      paymentProvider = null;
      phase = "idle";
      error = friendlyError(err);
    } finally {
      shell.rerender();
    }
  }

  async function openPaymentUrl(url: string): Promise<void> {
    await invoke("open_external", { url });
  }

  function startPolling(shell: ShellState): void {
    stopPolling();
    if (!order || order.status !== "pending") return;
    pollTimer = window.setInterval(() => void pollOrder(shell), 3000);
    void pollOrder(shell);
  }

  async function pollOrder(shell: ShellState): Promise<void> {
    if (!order || polling) return;
    polling = true;
    try {
      order = await invoke<PaymentOrder>("billing_order_status", { orderId: order.order_id });
      if (order.status === "paid") {
        phase = "paid";
        error = "";
        stopPolling();
        if (walletChanged) await walletChanged();
      } else if (["expired", "closed", "revoked", "payerror"].includes(order.status)) {
        phase = "idle";
        qrDataUrl = "";
        paymentProvider = null;
        stopPolling();
        error = t("subscription.orderExpired");
      }
      shell.rerender();
    } catch (err) {
      console.error("billing_order_status failed:", err);
    } finally {
      polling = false;
    }
  }

  function stopPolling(): void {
    if (pollTimer !== null) window.clearInterval(pollTimer);
    pollTimer = null;
  }

  function formatCny(amountFen: number): string {
    return `¥${(amountFen / 100).toFixed(amountFen % 100 === 0 ? 0 : 2)}`;
  }

  function formatDate(value: string | null): string {
    if (!value) return "—";
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return value;
    return new Intl.DateTimeFormat(undefined, {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    }).format(date);
  }

  function formatTime(value: string | null): string {
    if (!value) return "—";
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return value;
    return new Intl.DateTimeFormat(undefined, {
      hour: "2-digit",
      minute: "2-digit",
    }).format(date);
  }

  function friendlyError(value: unknown): string {
    const message = String(value).toLowerCase();
    if (message.includes("sign in")) return t("subscription.loginHint");
    if (message.includes("not enabled") || message.includes("not configured")) {
      return t("subscription.unavailable");
    }
    return t("subscription.paymentFailed");
  }
}
