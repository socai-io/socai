import { invoke } from "@tauri-apps/api/core";
import QRCode from "qrcode";

import type { ShellState } from "../main";
import { esc } from "../lib/html";
import { t } from "../lib/i18n";

interface StripePlan {
  plan_id: string;
  amount_minor: number;
  currency: string;
  points: number;
  sandbox: boolean;
  subscription_status: string | null;
  cancel_at_period_end: boolean;
}

interface PaymentPlan {
  stripe?: StripePlan | null;
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
  amount_minor?: number | null;
  currency?: string | null;
  points: number;
  duration_days: number;
  expires_at: string | null;
  paid_at: string | null;
  active_until: string | null;
}

type Phase = "idle" | "loading" | "creating" | "waiting" | "paid";
type PaymentProvider = "wechat" | "alipay" | "stripe";

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

  export function render(globalUser: boolean): string {
    return `
      <div class="subscription-content">
        ${renderContent(globalUser)}
        ${error ? `<p class="t-small result-error subscription-error" role="alert">${esc(error)}</p>` : ""}
      </div>
    `;
  }

  function renderContent(globalUser: boolean): string {
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
    if (globalUser) {
      return plan.stripe ? renderStripePlan(plan.stripe) : `<p class="t-small subtle subscription-copy">${esc(t("subscription.unavailable"))}</p>`;
    }
    return `
      <div class="subscription-payment-options subscription-payment-choice">
        ${plan.alipay_enabled ? `<button id="subscription-buy-alipay" type="button" class="btn-primary subscription-option" ${phase === "creating" ? "disabled" : ""}>
          <span>${esc(t("subscription.alipay"))}</span>
          <span class="t-small">${esc(t("subscription.pointCount", { points: plan.points }))}</span>
          <span class="t-small">${esc(formatCny(plan.amount_fen))}</span>
        </button>` : ""}
        ${plan.stripe && !plan.stripe.subscription_status ? `<button id="subscription-buy-stripe" type="button" class="btn-primary subscription-option" ${phase === "creating" ? "disabled" : ""}>
          <span>${esc(t("subscription.stripePayment"))}</span>
          <span class="t-small">${esc(t("subscription.pointCount", { points: plan.stripe.points }))}</span>
          <span class="t-small">${esc(formatMoney(plan.stripe.amount_minor, plan.stripe.currency))}${esc(t("subscription.perMonth"))}</span>
        </button>` : ""}
      </div>
      ${plan.stripe ? renderStripeStatus(plan.stripe) : ""}
    `;
  }

  function renderStripeStatus(stripe: StripePlan): string {
    return `
      ${stripe.sandbox ? `<p class="t-small subtle subscription-copy">${esc(t("subscription.sandbox"))}</p>` : ""}
      ${stripe.subscription_status ? `<p class="t-small">${esc(t(stripe.cancel_at_period_end ? "subscription.renewalCancelled" : stripe.subscription_status === "active" ? "subscription.active" : "subscription.paymentAttention"))}</p>
        ${!stripe.cancel_at_period_end ? `<button id="subscription-cancel-renewal" type="button" class="btn-ghost subscription-full-button">${esc(t("subscription.cancelRenewal"))}</button>` : ""}` : ""}
    `;
  }

  function renderStripePlan(stripe: StripePlan): string {
    return `
      <div class="subscription-plan-head">
        <p class="t-h2 subscription-plan-name">socai pro</p>
        <p class="t-h2 subscription-price">${esc(formatMoney(stripe.amount_minor, stripe.currency))}${esc(t("subscription.perMonth"))}</p>
      </div>
      <p class="t-small subtle subscription-copy">${esc(t("subscription.monthlyPoints", { points: stripe.points }))}</p>
      ${renderStripeStatus(stripe)}
      ${!stripe.subscription_status ? `<button id="subscription-buy-stripe" type="button" class="btn-primary subscription-full-button" ${phase === "creating" ? "disabled" : ""}>${esc(t("subscription.stripe"))}</button>` : ""}
    `;
  }

  function renderCheckout(value: PaymentOrder): string {
    const isStripe = paymentProvider === "stripe";
    const browserPayment = paymentProvider === "alipay" || isStripe;
    const waitingForQr = phase === "creating" || !qrDataUrl;
    return `
      <div class="subscription-checkout-head">
        <div>
          <p class="t-eyebrow">${esc(t(isStripe ? "subscription.stripe" : browserPayment ? "subscription.alipay" : "subscription.wechatPay"))}</p>
          <p class="t-h2 subscription-price">${esc(formatMoney(value.amount_minor ?? value.amount_fen, value.currency ?? "CNY"))}</p>
        </div>
      </div>
      ${browserPayment ? `
        <div class="subscription-browser-payment">
          <span class="subscription-browser-glyph" aria-hidden="true">↗</span>
          <p class="t-small">${esc(t(isStripe ? "subscription.stripeOpened" : "subscription.alipayOpened"))}</p>
        </div>
        <button id="subscription-open-payment" type="button" class="btn-primary subscription-full-button">${esc(t(isStripe ? "subscription.openCheckout" : "subscription.openAlipay"))}</button>
      ` : `
        <div class="subscription-qr-wrap">
          ${waitingForQr
            ? `<p class="t-small subtle">${esc(t("common.loading"))}</p>`
            : `<img class="subscription-qr" src="${esc(qrDataUrl)}" alt="${esc(t("subscription.qrAria"))}" />`}
        </div>
        <p class="t-small subscription-scan-hint">${esc(t("subscription.scanHint"))}</p>
      `}
      <button id="subscription-cancel" type="button" class="btn-ghost subscription-full-button">${esc(t("common.cancel"))}</button>
    `;
  }

  export function bind(shell: ShellState, onWalletChanged: () => Promise<void>): void {
    walletChanged = onWalletChanged;
    document.getElementById("subscription-buy-stripe")?.addEventListener("click", () => {
      void createOrder("stripe", shell);
    });
    document.getElementById("subscription-cancel-renewal")?.addEventListener("click", async (event) => {
      const button = event.currentTarget as HTMLButtonElement;
      button.disabled = true;
      try {
        await invoke("billing_cancel_stripe_subscription");
        await refresh(true);
      } catch (err) { error = friendlyError(err); }
      shell.rerender();
    });
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
      const command = provider === "stripe"
        ? "billing_create_stripe_order"
        : provider === "wechat"
        ? "billing_create_wechat_order"
        : "billing_create_alipay_order";
      order = await invoke<PaymentOrder>(command, {
        planId: provider === "stripe" ? plan.stripe?.plan_id : plan.plan_id,
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
      phase = order.status === "paid" ? "paid" : "waiting";
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
    const orderId = order.order_id;
    polling = true;
    try {
      const updated = await invoke<PaymentOrder>("billing_order_status", { orderId });
      if (order?.order_id !== orderId) return;
      order = updated;
      if (order.status === "paid") {
        phase = "paid";
        error = "";
        stopPolling();
        if (walletChanged) await walletChanged();
        if (paymentProvider === "stripe") await refresh(true);
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

  function formatMoney(amount: number, currency: string): string {
    return new Intl.NumberFormat(undefined, { style: "currency", currency }).format(amount / 100);
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

  function friendlyError(value: unknown): string {
    const message = String(value).toLowerCase();
    if (message.includes("sign in")) return t("subscription.loginHint");
    if (message.includes("not enabled") || message.includes("not configured")) {
      return t("subscription.unavailable");
    }
    return t("subscription.paymentFailed");
  }
}
