export type Language = "en" | "zh";
export type TaskStatusKey =
  "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted";

const DEFAULT_LANGUAGE: Language = "zh";
const STORAGE_KEY = "socai-language";
const TIMEZONE_STORAGE_KEY = "socai-timezone";
const supportedLanguages: Language[] = ["zh", "en"];

/** Agent points granted on first sign-in. Shown by the account menu and by the
 *  api-error copy that offers the built-in model as a way out of a spent key. */
export const SIGNUP_BONUS_POINTS = 50;

// Active IANA timezone for displaying task timestamps. `undefined` follows the
// system local zone. This is a display-only preference (no backend field) — it
// is persisted to localStorage and threaded into the timestamp formatters.
let activeTimezone: string | undefined = readInitialTimezone();

const messages = {
  "language.switcherAria": { en: "language", zh: "语言" },

  "common.loading": { en: "loading…", zh: "加载中…" },
  "common.or": { en: "or,", zh: "或" },
  "common.save": { en: "save", zh: "保存" },
  "common.saving": { en: "saving…", zh: "保存中…" },
  "common.cancel": { en: "cancel", zh: "取消" },

  "chrome.label": { en: "chrome", zh: "chrome" },
  "chrome.connectAria": { en: "connect chrome", zh: "连接 chrome" },
  "chrome.statusToggleAria": {
    en: "show chrome connection status",
    zh: "显示 chrome 连接状态",
  },
  "chrome.dialogAria": {
    en: "chrome connection status",
    zh: "chrome 连接状态",
  },
  "chrome.requiredAria": { en: "chrome required", zh: "需要 chrome" },
  "chrome.disconnected": { en: "disconnected", zh: "未连接" },
  "chrome.connecting": { en: "connecting", zh: "连接中" },
  "chrome.connected": { en: "connected", zh: "已连接" },
  "chrome.tabs": { en: "tabs", zh: "标签页" },
  "chrome.browser": { en: "browser", zh: "浏览器" },
  "chrome.endpoint": { en: "endpoint", zh: "端点" },
  "chrome.source": { en: "source", zh: "来源" },
  "chrome.sourceManaged": { en: "isolated profile", zh: "独立配置文件" },
  "chrome.sourceExisting": { en: "existing browser", zh: "现有浏览器" },
  "chrome.sourceRemote": { en: "remote (beta)", zh: "远程（beta）" },
  "chrome.profile": { en: "profile", zh: "资料目录" },
  "chrome.disconnect": { en: "disconnect", zh: "断开连接" },
  "chrome.lookingForChrome": {
    en: "looking for chrome…",
    zh: "正在寻找 chrome…",
  },
  "chrome.connectToStart": {
    en: "connect chrome to start",
    zh: "连接 chrome 后开始",
  },
  "chrome.setupTitle": {
    en: "connect agent to chrome",
    zh: "帮Agent连接到Chrome",
  },
  "chrome.connectCta": { en: "connect chrome →", zh: "连接 chrome →" },
  "chrome.connectingCta": { en: "connecting…", zh: "连接中…" },
  "chrome.remoteAutoReconnect": {
    en: "hosted browser not connected — it reconnects when you send.",
    zh: "云端浏览器未连接 — 发送时会自动重连。",
  },
  "chrome.remoteDebuggingHelp": {
    en: "open chrome remote debugging ↗",
    zh: "打开 chrome 远程调试 ↗",
  },
  "chrome.setupEnableTitle": {
    en: "enable remote debugging and check the box",
    zh: "开启远程调试，在方框里打钩",
  },
  "chrome.setupAllowTitle": {
    en: "click Allow in chrome",
    zh: "在 chrome 弹窗中点击 Allow",
  },
  "chrome.setupOpenSettings": {
    en: "open chrome settings ↗",
    zh: "打开 chrome 设置 ↗",
  },
  "chrome.setupConnect": { en: "connect chrome", zh: "连接 chrome" },
  "chrome.setupWaiting": { en: "waiting", zh: "等待操作" },
  "chrome.setupDone": { en: "done", zh: "已完成" },
  "chrome.setupDetecting": {
    en: "waiting for remote debugging…",
    zh: "正在等待开启远程调试…",
  },
  "chrome.setupWaitingAllow": {
    en: "detected — waiting for Allow…",
    zh: "已检测到，等待点击 Allow…",
  },
  "chrome.setupReady": {
    en: "remote debugging enabled",
    zh: "远程调试已开启",
  },
  "chrome.setupEnableImageAlt": {
    en: "allow remote debugging setting in chrome",
    zh: "chrome 开启远程调试设置",
  },
  "chrome.setupAllowImageAlt": {
    en: "chrome remote debugging Allow dialog",
    zh: "chrome 远程调试 Allow 弹窗",
  },

  "status.capsuleAria": {
    en: "chrome, model, and account status",
    zh: "chrome、模型与账号状态",
  },

  "sidebar.collapseAria": { en: "collapse sidebar", zh: "收起侧边栏" },
  "sidebar.expandAria": { en: "expand sidebar", zh: "展开侧边栏" },

  "update.restartToUpdate": { en: "restart to update", zh: "重启更新" },
  "update.chip": { en: "update", zh: "更新" },
  "update.downloading": { en: "downloading update…", zh: "更新下载中…" },
  "update.taskRunningWarn": {
    en: "a task is running — restarting will interrupt it.",
    zh: "有任务正在运行 — 重启会中断它。",
  },
  "update.restartAnyway": { en: "restart anyway", zh: "仍然重启" },
  "update.later": { en: "later", zh: "稍后" },

  "auth.title": { en: "account", zh: "账号" },
  "auth.login": { en: "sign in", zh: "登录" },
  "auth.loginAria": { en: "sign in to socai", zh: "登录 socai" },
  "auth.accountAria": { en: "open account menu", zh: "打开账号菜单" },
  "auth.loginTitle": { en: "sign in with phone", zh: "手机号登录" },
  "auth.loginAgentHint": {
    en: "sign in to get {points} points of agent credit",
    zh: "登录后获赠{points}点 agent额度",
  },
  "auth.useOwnApiKey": {
    en: "or use your own API key, no sign-in required",
    zh: "或自己输入API key，无需登录",
  },
  "auth.useOwnApiKeyNoPoints": {
    en: "or use your own API key without points",
    zh: "或自己输入API key，不使用点数",
  },
  "auth.phone": { en: "phone", zh: "手机号" },
  "auth.sendCode": { en: "send code", zh: "获取验证码" },
  "auth.sending": { en: "sending…", zh: "发送中…" },
  "auth.enterCode": { en: "enter verification code", zh: "输入验证码" },
  "auth.code": { en: "verification code", zh: "验证码" },
  "auth.codeSent": {
    en: "a 6-digit code was sent to {phone}.",
    zh: "6 位验证码已发送至 {phone}。",
  },
  "auth.verifying": { en: "signing in…", zh: "登录中…" },
  "auth.changePhone": { en: "change phone", zh: "更换手机号" },
  "auth.resend": { en: "resend code", zh: "重新发送" },
  "auth.resendCountdown": { en: "resend in {seconds}s", zh: "{seconds} 秒后重发" },
  "auth.account": { en: "account", zh: "账号" },
  "auth.accountHint": {
    en: "this account is active on this device.",
    zh: "此账号已在当前设备登录。",
  },
  "auth.loggedIn": { en: "signed in", zh: "已登录" },
  "auth.loggedOut": { en: "signed out", zh: "未登录" },
  "auth.logout": { en: "sign out", zh: "退出登录" },
  "auth.loggingOut": { en: "signing out…", zh: "退出中…" },
  "auth.invalidPhone": {
    en: "enter a valid mainland China phone number.",
    zh: "请输入有效的中国大陆手机号。",
  },
  "auth.invalidCodeFormat": {
    en: "enter the 6-digit verification code.",
    zh: "请输入 6 位验证码。",
  },
  "auth.tooFrequent": {
    en: "code requested too frequently. try again shortly.",
    zh: "验证码请求过于频繁，请稍后重试。",
  },
  "auth.invalidCode": { en: "the verification code is incorrect.", zh: "验证码不正确。" },
  "auth.expiredCode": { en: "the verification code has expired.", zh: "验证码已过期。" },
  "auth.usedCode": { en: "this verification code was already used.", zh: "此验证码已使用。" },
  "auth.tooManyAttempts": {
    en: "too many attempts. request a new code.",
    zh: "尝试次数过多，请重新获取验证码。",
  },
  "auth.serverNotConfigured": {
    en: "the socai service is not configured in this build.",
    zh: "当前版本尚未配置 socai 服务地址。",
  },
  "auth.requestFailed": {
    en: "request failed. check your connection and try again.",
    zh: "请求失败，请检查网络后重试。",
  },
  "auth.sessionLoadFailed": {
    en: "could not read the saved sign-in state.",
    zh: "无法读取已保存的登录状态。",
  },

  "billing.balance": { en: "point balance", zh: "点数余额" },
  "billing.remaining": { en: "points remaining", zh: "剩余点数" },
  "billing.points": { en: "{points} points", zh: "{points} 点" },
  "billing.pointsUsed": { en: "{points} points used", zh: "消耗 {points} 点" },
  "billing.unavailable": { en: "unavailable", zh: "暂不可用" },
  "billing.activeUntil": { en: "active until {date}", zh: "有效期至 {date}" },
  "billing.rechargeHint": {
    en: "mock recharge for the MVP; points are added immediately.",
    zh: "MVP 暂用 mock 充值，点击后点数立即到账。",
  },
  "billing.recharging": { en: "adding…", zh: "充值中…" },

  "subscription.label": { en: "subscribe", zh: "订阅" },
  "subscription.aria": { en: "open subscription", zh: "打开订阅" },
  "subscription.upgradePro": { en: "upgrade to pro", zh: "升级到 Pro" },
  "subscription.renewPro": { en: "renew pro", zh: "续订 Pro" },
  "subscription.proPoints": { en: "500 points", zh: "500 点数" },
  "subscription.proXhs": { en: "use xiaohongshu without signing in", zh: "免登录小红书" },
  "subscription.active": { en: "subscribed", zh: "已订阅" },
  "subscription.inactive": { en: "not subscribed", zh: "未订阅" },
  "subscription.loginHint": {
    en: "sign in with your phone number before subscribing.",
    zh: "请先使用手机号登录，再开通订阅。",
  },
  "subscription.login": { en: "sign in to continue", zh: "登录后继续" },
  "subscription.loadFailed": {
    en: "could not load the subscription plan.",
    zh: "暂时无法加载订阅方案。",
  },
  "subscription.unavailable": {
    en: "Payment is not available yet.",
    zh: "支付尚未开放。",
  },
  "subscription.duration": { en: "access", zh: "有效期" },
  "subscription.days": { en: "{days} days", zh: "{days} 天" },
  "subscription.oneMonth": { en: "one-month subscription", zh: "订阅一个月" },
  "subscription.renewal": { en: "renewal", zh: "续费方式" },
  "subscription.noAutoRenew": { en: "manual", zh: "到期不自动续费" },
  "subscription.planHint": {
    en: "Includes hosted AI access with no API key setup. Your own API key remains available in settings.",
    zh: "包含免配置 API Key 的云端模型用量；你仍可在设置中使用自己的 API Key。",
  },
  "subscription.wechatPay": { en: "pay with WeChat", zh: "微信扫码支付" },
  "subscription.alipay": { en: "pay with Alipay", zh: "支付宝支付" },
  "subscription.alipayOpened": {
    en: "Alipay checkout opened in your browser. Complete payment there, then return to socai.",
    zh: "已在浏览器打开支付宝收银台。完成支付后请返回 socai。",
  },
  "subscription.openAlipay": {
    en: "open Alipay checkout again",
    zh: "重新打开支付宝收银台",
  },
  "subscription.awaitingPayment": { en: "awaiting payment", zh: "等待支付" },
  "subscription.qrAria": { en: "WeChat Pay QR code", zh: "微信支付二维码" },
  "subscription.scanHint": {
    en: "scan with WeChat on your phone. Payment is confirmed automatically.",
    zh: "请使用手机微信扫码，支付成功后会自动确认。",
  },
  "subscription.expires": { en: "QR code expires at {time}.", zh: "二维码将在 {time} 失效。" },
  "subscription.success": { en: "subscription active", zh: "订阅已开通" },
  "subscription.successHint": {
    en: "{points} points added. Access is active until {date}.",
    zh: "已到账 {points} 点，有效期至 {date}。",
  },
  "subscription.done": { en: "done", zh: "完成" },
  "subscription.orderExpired": {
    en: "this payment QR code expired. create a new one to continue.",
    zh: "支付二维码已失效，请重新发起支付。",
  },
  "subscription.paymentFailed": {
    en: "payment could not be started. try again shortly.",
    zh: "暂时无法发起支付，请稍后重试。",
  },

  "voice.startCloud": { en: "start cloud voice input", zh: "开始云端语音输入" },
  "voice.startLocal": { en: "start local voice input", zh: "开始本地语音输入" },
  "voice.stop": { en: "stop recording and transcribe", zh: "停止录音并转写" },
  "voice.requesting": { en: "requesting microphone access…", zh: "正在请求麦克风权限…" },
  "voice.transcribingCloud": { en: "transcribing with cloud ASR…", zh: "正在使用云端 ASR 转写…" },
  "voice.transcribingLocal": { en: "transcribing with local Whisper small…", zh: "正在使用本地 Whisper small 转写…" },
  "voice.unavailable.taskBusy": {
    en: "voice input is unavailable while this task is running.",
    zh: "任务运行期间无法使用语音输入。",
  },
  "voice.unavailable.browser": {
    en: "voice input is unavailable in this webview.",
    zh: "当前 WebView 不支持语音输入。",
  },
  "voice.unavailable.checking": {
    en: "checking voice input availability…",
    zh: "正在检测语音输入状态…",
  },
  "voice.unavailable.login": {
    en: "you are not signed in, so voice input uses local Whisper small.",
    zh: "当前未登录，语音输入使用本地 Whisper small。",
  },
  "voice.unavailable.subscription": {
    en: "this account has no active paid subscription, so voice input uses local Whisper small.",
    zh: "当前账号未开通有效付费服务，语音输入使用本地 Whisper small。",
  },
  "voice.unavailable.credits": {
    en: "this account has no remaining credits, so voice input uses local Whisper small.",
    zh: "当前账号 credits 已用完，语音输入使用本地 Whisper small。",
  },
  "voice.unavailable.billing": {
    en: "cloud ASR access could not be verified, so voice input uses local Whisper small.",
    zh: "暂时无法确认云端 ASR 付费状态，语音输入使用本地 Whisper small。",
  },
  "voice.unavailable.localOnly": {
    en: "voice input uses local Whisper small.",
    zh: "语音输入使用本地 Whisper small。",
  },
  "voice.local.modelMissing": {
    en: "The first local transcription downloads the fixed {size} model.",
    zh: "首次执行本地转写时将下载固定的 {size} 模型。",
  },
  "voice.local.downloading": {
    en: "The local model is downloading ({percent}%).",
    zh: "本地模型正在下载（{percent}%）。",
  },
  "voice.local.ready": {
    en: "The local model is ready.",
    zh: "本地模型已就绪。",
  },
  "voice.local.helperMissing": {
    en: "The bundled local ASR helper is unavailable; reinstall socai to restore it.",
    zh: "内置本地 ASR 组件不可用，请重新安装 socai。",
  },
  "voice.local.failed": {
    en: "The local model setup failed and will retry on the next audio task.",
    zh: "本地模型准备失败，将在下次音频任务中重试。",
  },
  "voice.error.permission": {
    en: "microphone permission was denied. allow it in system settings and try again.",
    zh: "麦克风权限被拒绝，请在系统设置中允许后重试。",
  },
  "voice.error.noDevice": { en: "no microphone was found.", zh: "未找到可用的麦克风。" },
  "voice.error.capture": {
    en: "the microphone could not be started.",
    zh: "无法启动麦克风。",
  },
  "voice.error.tooShort": {
    en: "the recording is too short. speak for a little longer and try again.",
    zh: "录音太短，请多说一会儿后重试。",
  },
  "voice.error.maxDuration": {
    en: "recording stopped after two minutes. start a new recording to continue.",
    zh: "录音已在两分钟后自动停止，请重新开始录音。",
  },
  "voice.error.noSpeech": {
    en: "no speech was recognized.",
    zh: "没有识别到语音内容。",
  },

  "settings.aria": { en: "settings", zh: "设置" },
  "settings.title": { en: "settings", zh: "设置" },
  "settings.general": { en: "general", zh: "通用" },
  "settings.language": { en: "language", zh: "语言" },
  "settings.timezone": { en: "timezone", zh: "时区" },
  "settings.timezoneSystem": { en: "system (local)", zh: "系统（本地）" },
  "settings.output": { en: "output", zh: "输出" },
  "settings.outputDir": { en: "output directory", zh: "输出目录" },
  "settings.outputHint": {
    en: "where run reports, traces, and screenshots are saved.",
    zh: "运行报告、轨迹和截图的保存位置。",
  },
  "settings.inviteCode": { en: "invite code", zh: "邀请码" },
  "settings.enter": { en: "enter", zh: "输入" },
  "settings.loginForInvite": { en: "sign in first", zh: "请先登录" },
  "settings.inviteRequired": { en: "enter an invite code", zh: "请输入邀请码" },
  "settings.inviteInvalid": { en: "invite code could not be verified", zh: "邀请码验证失败" },
  "settings.browse": { en: "browse…", zh: "浏览…" },
  "settings.chrome": { en: "chrome", zh: "chrome" },
  "settings.source": { en: "source", zh: "来源" },
  "settings.sourceManaged": { en: "isolated profile", zh: "独立配置文件" },
  "settings.sourceExisting": { en: "existing browser", zh: "现有浏览器" },
  "settings.sourceRemote": { en: "remote (beta)", zh: "远程（beta）" },
  "settings.sourceRemotePro": {
    en: "remote (Pro, no local xiaohongshu connection)",
    zh: "远程（Pro，无需本地连接小红书）",
  },
  "settings.profileDir": { en: "profile directory", zh: "资料目录" },
  "settings.profileHint": {
    en: "socai launches a throwaway chrome with this profile.",
    zh: "socai 会用此资料目录启动临时 chrome。",
  },
  "settings.remoteHint": {
    en: "beta — runs on socai's hosted browser; no local chrome, no xiaohongshu login needed. daily session limits apply.",
    zh: "beta — 使用 socai 云端托管浏览器；无需本地 chrome，也无需登录小红书。每天有会话次数限制。",
  },
  "settings.endpoint": { en: "debugging endpoint", zh: "调试端点" },
  "settings.endpointHint": {
    en: "socai auto-detects a chrome started with --remote-debugging-port.",
    zh: "socai 会自动检测使用 --remote-debugging-port 启动的 chrome。",
  },
  "settings.endpointDisconnected": { en: "not connected", zh: "未连接" },
  "settings.saved": { en: "saved", zh: "已保存" },
  "settings.saveFailed": {
    en: "could not save settings.",
    zh: "无法保存设置。",
  },
  "settings.version": { en: "app version", zh: "应用版本" },
  "settings.loadFailed": {
    en: "could not load settings.",
    zh: "无法加载设置。",
  },
  "agent.label": { en: "model", zh: "模型" },
  "agent.configurationAria": { en: "agent configuration", zh: "智能体设置" },
  "agent.selectModelAria": { en: "select agent model", zh: "选择智能体模型" },
  "agent.selectProviderAria": {
    en: "select model source",
    zh: "选择模型来源",
  },
  "agent.apiKey": { en: "api key", zh: "api key" },
  "agent.credentialPreview": {
    en: "configured {preview}",
    zh: "已配置 {preview}",
  },
  "agent.managedModel": {
    en: "an LLM adapted for socai, used with points, no API key required",
    zh: "针对socai适配的LLM，通过点数使用，无需API key",
  },
  "agent.chatgptConnected": {
    en: "chatgpt connected",
    zh: "已连接 chatgpt",
  },
  "agent.updateCredential": { en: "update api key", zh: "更新 api key" },
  "agent.replaceCredential": {
    en: "saving replaces your current {provider} api key.",
    zh: "保存后将替换当前的 {provider} api key。",
  },
  "agent.modelVersion": { en: "model version", zh: "模型版本" },
  "agent.loading": { en: "loading", zh: "加载中" },
  "agent.keyNeeded": { en: "api key needed", zh: "需要 api key" },
  "agent.defaultModel": { en: "default", zh: "默认" },
  "agent.needsCredential": {
    en: "{model} needs an api key.",
    zh: "{model} 需要 api key。",
  },
  "agent.connectChatgpt": {
    en: "connect chatgpt subscription",
    zh: "连接 chatgpt 订阅",
  },
  "agent.opening": { en: "opening…", zh: "打开中…" },
  "agent.pasteApiKey": { en: "paste api key", zh: "粘贴 api key" },
  "agent.codexLoginMissing": {
    en: "codex login not detected yet. return to socai after login completes.",
    zh: "还没有检测到 codex 登录。登录完成后返回 socai。",
  },

  "task.new": { en: "new task", zh: "新任务" },
  "task.history": { en: "history", zh: "历史" },
  "task.historyAria": { en: "task history", zh: "任务历史" },
  "task.viewAria": { en: "task view", zh: "任务视图" },
  "task.noTasks": { en: "no tasks yet.", zh: "暂无任务。" },
  "task.cancel": { en: "cancel", zh: "取消" },
  "task.resume": { en: "continue", zh: "继续" },
  "task.resumePrompt": {
    en: "Continue the interrupted task from its saved progress. Reuse the completed work and saved artifacts, avoid repeating finished work unless necessary, and finish the original request.",
    zh: "继续刚才中断的任务。复用已经完成的工作和已保存的产物，除非确有必要，不要重复已经完成的步骤，并完成原始请求。",
  },
  "task.delete": { en: "delete", zh: "删除" },
  "task.deleteAria": { en: "delete task", zh: "删除任务" },
  "task.deleteQuestion": { en: "delete this task?", zh: "删除此任务？" },
  "task.deleteWarn": {
    en: "the task and all of its artifacts will be deleted. this can’t be undone.",
    zh: "该任务及其所有产物将被永久删除。此操作不可撤销。",
  },
  "task.deleteKeep": { en: "keep", zh: "保留" },
  "task.you": { en: "you", zh: "你" },
  "task.working": { en: "working…", zh: "运行中…" },
  "task.activityLabel": { en: "activity", zh: "运行过程" },
  "task.interruptedAppClosed": {
    en: "the app was closed before this task finished.",
    zh: "应用在任务完成前已关闭。",
  },
  "task.searchLabel": { en: "search", zh: "搜索" },
  "task.progressReading": { en: "reading", zh: "读取笔记" },
  "task.progressOcr": { en: "reading images", zh: "识别图片文字" },
  "task.notesLabel": { en: "notes", zh: "笔记" },
  "note.commentsHead": { en: "{n} comments", zh: "共 {n} 条评论" },
  "note.authorBadge": { en: "author", zh: "作者" },
  "note.transcript": { en: "transcript", zh: "语音转写" },
  "note.openExternal": { en: "open on xiaohongshu", zh: "在小红书打开" },
  "task.replyPlaceholder": { en: "ask a follow-up…", zh: "继续追问…" },
  "task.replyConnectHint": {
    en: "connect chrome to send a follow-up",
    zh: "连接 chrome 后可继续追问",
  },
  "task.replySend": { en: "send", zh: "发送" },
  "task.preflightModelConfig": {
    en: "the selected model is not ready. configure its api key or account connection in the model menu, then try again.",
    zh: "当前模型尚未完成配置。请在右上角模型菜单中添加 API Key 或完成账号连接，然后重试。",
  },
  "task.preflightAuth": {
    en: "your socai account is signed out. sign in, then send the task again.",
    zh: "当前 socai 账号尚未登录。请先登录账号，然后重新发送任务。",
  },
  "task.preflightBalance": {
    en: "your socai account does not have enough points. recharge or switch to another configured model, then try again.",
    zh: "当前 socai 账号点数不足。请充值点数或切换到其他已配置的模型，然后重试。",
  },
  "task.preflightAccount": {
    en: "socai could not verify your account and point balance. check the network and account region, then try again.",
    zh: "socai 无法验证当前账号和点数。请检查网络以及账号区域，然后重试。",
  },
  "task.preflightSite": {
    en: "socai could not initialize xiaohongshu support. restart the app and update it if the problem continues.",
    zh: "socai 无法初始化小红书运行环境。请重启应用；问题持续时请更新到最新版本。",
  },
  "task.preflightBrowserConfig": {
    en: "socai could not read the chrome configuration. choose the browser source again in settings, then try again.",
    zh: "socai 无法读取 chrome 配置。请在设置中重新选择浏览器来源，然后重试。",
  },
  "task.preflightBrowser": {
    en: "socai could not prepare chrome. check the browser source in settings, keep chrome open, and reconnect.",
    zh: "socai 无法准备 chrome 浏览器。请检查设置中的浏览器来源，保持 chrome 运行并重新连接。",
  },
  "task.preflightBrowserRemote": {
    en: "socai could not connect to the hosted browser. check your network and socai account, then try again.",
    zh: "socai 无法连接云端浏览器。请检查网络和 socai 账号状态，然后重试。",
  },
  "task.preflightBrowserRemoteQuota": {
    en: "this device has used up today's hosted browser time. try again tomorrow, or switch the browser source to a local chrome in settings.",
    zh: "本设备今日的云端浏览器时长已用完。请明天再试，或在设置中将浏览器来源切换为本地 chrome。",
  },
  "task.preflightXhsLogin": {
    en: "xiaohongshu is signed out in the connected chrome profile. complete sign-in there, then try again.",
    zh: "当前 chrome 配置文件尚未登录小红书。请在已连接的 chrome 中完成登录，然后重试。",
  },
  "task.preflightXhsSession": {
    en: "the hosted xiaohongshu session is unavailable. reconnect the hosted browser and try again; contact support if it continues.",
    zh: "云端小红书会话暂时不可用。请重新连接云端浏览器后重试；问题持续时请联系支持。",
  },
  "task.preflightUnknown": {
    en: "socai could not complete the checks required to start this task. try again and share the error code if it continues.",
    zh: "socai 无法完成任务启动检查。请重试；问题持续时请提供错误码。",
  },
  "task.preflightFailedTitle": {
    en: "task cannot start yet",
    zh: "任务暂时无法开始",
  },
  "task.errorCode": { en: "error code: {code}", zh: "错误码：{code}" },
  "task.apiErrorAuthTitle": { en: "model authentication failed", zh: "模型认证失败" },
  "task.apiErrorAuth": {
    en: "{provider} rejected the current API key. update it in the model menu at the top right, then send the task again.",
    zh: "{provider} 拒绝了当前 API Key。请在右上角模型菜单中更新 API Key，然后重新发送任务。",
  },
  "task.apiErrorAuthOwnKey": {
    en: "{provider} rejected the API key you provided. update it in the model menu at the top right.",
    zh: "{provider} 拒绝了您提供的 API Key。请在右上角模型菜单中更新 API Key。",
  },
  "task.apiErrorBalanceTitle": { en: "model balance is insufficient", zh: "模型余额不足" },
  "task.apiErrorBalance": {
    en: "{provider} reported insufficient balance or quota. recharge the account or switch to another configured model, then try again.",
    zh: "{provider} 返回余额或额度不足。请充值对应账号或切换到其他已配置的模型，然后重试。",
  },
  "task.apiErrorBalanceOwnKey": {
    en: "the account behind the {provider} API key you provided is out of balance or quota.",
    zh: "您提供的 {provider} API Key 对应账号余额或额度不足。",
  },
  "task.apiErrorUsageLimitTitle": {
    en: "model usage limit reached",
    zh: "模型使用额度已用完",
  },
  "task.apiErrorUsageLimit": {
    en: "{provider} reported that this account's usage limit has been reached. wait for the quota cycle to reset, or switch accounts or models.",
    zh: "{provider} 当前账号的使用额度已用完。请等待额度周期恢复，或切换账号或其他已配置的模型。",
  },
  "task.apiErrorUsageLimitOwnKey": {
    en: "the {provider} API key you provided has used up its quota. wait for its quota cycle to reset.",
    zh: "您提供的 {provider} API Key 额度已使用完。请等待该额度周期恢复。",
  },
  "task.apiErrorForbiddenTitle": { en: "model access was denied", zh: "模型访问被拒绝" },
  "task.apiErrorForbidden": {
    en: "{provider} denied access to this model. check the model permission and account region, or switch models.",
    zh: "{provider} 拒绝访问当前模型。请检查模型权限和账号区域，或切换其他模型。",
  },
  "task.apiErrorForbiddenOwnKey": {
    en: "{provider} denied this model to the API key you provided. check the model permission and account region.",
    zh: "{provider} 拒绝您提供的 API Key 访问该模型。请检查模型权限和账号区域。",
  },
  "task.apiErrorModelNotActivatedTitle": {
    en: "selected model is not enabled",
    zh: "所选模型尚未开通",
  },
  "task.apiErrorModelNotActivated": {
    en: "the selected model is not enabled for the current {provider} account. this task has stopped and will not retry automatically. enable the model in the provider console, or switch to an enabled model and send the task again.",
    zh: "当前 {provider} 账号尚未开通所选模型。本次任务已停止，不会自动重试。请先在模型服务商控制台开通该模型，或在右上角模型菜单切换到已开通的模型后重新发送。",
  },
  "task.apiErrorModelNotActivatedOwnKey": {
    en: "the account for your {provider} API key does not have the selected model enabled. this task has stopped and will not retry automatically. enable the model in the provider console, or use an enabled model or API key and send the task again.",
    zh: "您提供的 {provider} API Key 对应账号尚未开通所选模型。本次任务已停止，不会自动重试。请先在模型服务商控制台开通该模型，或更换已开通的模型/API Key 后重新发送。",
  },
  "task.apiErrorRateLimitTitle": { en: "model requests are too frequent", zh: "模型请求过于频繁" },
  "task.apiErrorRateLimit": {
    en: "{provider} is rate limiting requests. wait briefly and send the task again, or switch models.",
    zh: "{provider} 正在限制请求频率。请稍后重新发送任务，或切换其他模型。",
  },
  "task.apiErrorRateLimitOwnKey": {
    en: "{provider} is rate limiting the API key you provided. wait briefly and send the task again.",
    zh: "{provider} 正在限制您提供的 API Key 的请求频率。请稍后重新发送任务。",
  },
  "task.apiErrorUnavailableTitle": { en: "model service is unavailable", zh: "模型服务暂时不可用" },
  "task.apiErrorUnavailable": {
    en: "{provider} is temporarily unavailable. wait briefly and send the task again.",
    zh: "{provider} 服务暂时不可用。请稍后重新发送任务。",
  },
  "task.apiErrorOverloadedTitle": {
    en: "model service is overloaded",
    zh: "模型服务当前过载",
  },
  "task.apiErrorOverloaded": {
    en: "{provider} is currently overloaded. socai completed its bounded automatic retries; wait briefly and resend the task, or switch models.",
    zh: "{provider} 服务当前过载。socai 已完成有限次数的自动重试；请稍后重新发送任务，或切换其他模型。",
  },
  "task.apiErrorTimeoutTitle": { en: "model response timed out", zh: "模型响应超时" },
  "task.apiErrorTimeout": {
    en: "socai timed out while waiting for {provider}. check the network and proxy settings, then try again.",
    zh: "socai 等待 {provider} 响应时超时。请检查网络和代理设置，然后重试。",
  },
  "task.apiErrorNetworkTitle": { en: "model connection failed", zh: "模型连接失败" },
  "task.apiErrorNetwork": {
    en: "socai could not reach {provider}. check the network and proxy settings, then try again.",
    zh: "socai 无法连接 {provider}。请检查网络和代理设置，然后重试。",
  },
  "task.apiErrorGenericTitle": { en: "model request failed", zh: "模型请求失败" },
  "task.apiErrorGeneric": {
    en: "{provider} could not complete the request. try again or switch to another configured model.",
    zh: "{provider} 无法完成当前请求。请重试或切换到其他已配置的模型。",
  },
  "task.apiErrorDismissAria": { en: "dismiss error", zh: "关闭错误提示" },
  "task.apiErrorRequestId": { en: "request {id}", zh: "请求 {id}" },
  "task.apiErrorProvider": { en: "the model provider", zh: "模型服务" },
  "task.apiErrorSignUpHint": {
    en: "socai's built-in model needs no API key — sign in to claim {points} free points and keep going.",
    zh: "socai 内置模型无需 API Key，注册账号即可免费领取 {points} 点额度，继续当前任务。",
  },
  "task.apiErrorSwitchManagedHint": {
    en: "or switch to socai's built-in model in the model menu, which runs on account points without an API key.",
    zh: "也可以在模型菜单中切换到 socai 内置模型，使用账号点数、无需 API Key。",
  },

  "artifact.listAria": { en: "generated files", zh: "生成的文件" },
  "artifact.download": { en: "download", zh: "下载" },
  "artifact.downloading": { en: "downloading…", zh: "下载中…" },
  "artifact.downloadFailed": { en: "retry download", zh: "重试下载" },
  "artifact.downloadAria": { en: "download {name}", zh: "下载 {name}" },
  "artifact.downloadingAria": { en: "downloading {name}", zh: "正在下载 {name}" },
  "artifact.downloadFailedAria": { en: "retry downloading {name}", zh: "重试下载 {name}" },
  "artifact.open": { en: "open", zh: "打开" },
  "artifact.opening": { en: "opening…", zh: "正在打开…" },
  "artifact.openFailed": { en: "retry opening", zh: "重试打开" },
  "artifact.openAria": { en: "show downloaded {name} in folder", zh: "在下载目录中显示 {name}" },
  "artifact.openingAria": { en: "showing {name} in folder", zh: "正在下载目录中显示 {name}" },
  "artifact.openFailedAria": { en: "retry showing {name} in folder", zh: "重试在下载目录中显示 {name}" },
  "artifact.previewAria": { en: "preview {name}", zh: "预览 {name}" },
  "artifact.previewPanelAria": { en: "preview of {name}", zh: "{name} 预览" },
  "artifact.previewClose": { en: "close preview", zh: "关闭预览" },
  "artifact.previewResize": { en: "resize preview", zh: "调整预览宽度" },
  "artifact.previewLoading": { en: "loading preview…", zh: "正在加载预览…" },
  "artifact.previewFailed": { en: "preview unavailable", zh: "无法预览" },
  "artifact.previewPdfUnavailable": {
    en: "PDF preview is unavailable in this WebView.",
    zh: "当前 WebView 无法显示 PDF 预览。",
  },
  "artifact.previewTableLimit": {
    en: "preview limited to {rows} rows and {columns} columns",
    zh: "预览最多显示 {rows} 行、{columns} 列",
  },
  "artifact.previewWorkbookEmpty": {
    en: "this workbook has no visible data",
    zh: "工作簿中没有可显示的数据",
  },
  "artifact.previewWorkbookSheets": { en: "workbook sheets", zh: "工作表" },
  "artifact.previewWorkbookLimit": {
    en: "preview truncated ({shown} of {total} sheets shown); download the file for complete data",
    zh: "预览内容已截断（显示 {shown}/{total} 个工作表）；完整数据请下载文件查看",
  },
  "artifact.previewWorksheetLimit": {
    en: "worksheet preview truncated; download the file for complete data",
    zh: "工作表预览内容已截断；完整数据请下载文件查看",
  },

  "feishu.export": { en: "export to feishu", zh: "导出到飞书" },
  "feishu.dialogAria": { en: "export to feishu", zh: "导出到飞书" },
  "feishu.title": { en: "export to feishu", zh: "导出到飞书" },
  "feishu.loadingAccounts": { en: "loading accounts…", zh: "正在加载飞书账户…" },
  "feishu.accountLoadTimeout": {
    en: "Loading Feishu accounts timed out. Please retry.",
    zh: "读取飞书账户超时，请重试。",
  },
  "feishu.accountIdentityTimeout": {
    en: "Loading Feishu account details timed out.",
    zh: "读取飞书账户信息超时。",
  },
  "feishu.account": { en: "choose account", zh: "选择账户" },
  "feishu.unknownAccount": { en: "connected account", zh: "已连接账户" },
  "feishu.noAccount": { en: "no connected account", zh: "尚未连接账户" },
  "feishu.connectCurrentAccount": {
    en: "reconnect browser account",
    zh: "重新连接浏览器中的账户",
  },
  "feishu.toDocument": { en: "Feishu document", zh: "导出到文档" },
  "feishu.toGroup": { en: "Feishu group", zh: "发送到群" },
  "feishu.creatingApp": {
    en: "approve app creation in the browser…",
    zh: "请在浏览器中确认创建应用…",
  },
  "feishu.authorizing": {
    en: "finish authorization in the browser…",
    zh: "请在浏览器中完成授权…",
  },
  "feishu.exporting": { en: "creating document…", zh: "正在创建飞书文档…" },
  "feishu.loadingGroups": { en: "loading groups…", zh: "正在加载群聊…" },
  "feishu.chatLoadTimeout": {
    en: "Loading Feishu groups timed out. Document export is still available.",
    zh: "读取飞书群聊超时，仍可导出到文档。",
  },
  "feishu.noGroups": { en: "no joined groups found.", zh: "没有找到已加入的群聊。" },
  "feishu.chooseGroup": { en: "choose a group", zh: "选择群聊" },
  "feishu.send": { en: "send", zh: "发送" },
  "feishu.sending": { en: "sending…", zh: "发送中…" },
  "feishu.sent": { en: "sent to group", zh: "已发送到群" },
  "feishu.disconnected": { en: "disconnected", zh: "已断开" },
  "feishu.disconnect": { en: "disconnect", zh: "断开" },
  "feishu.reconnect": { en: "reconnect", zh: "重新连接" },
  "feishu.disconnecting": { en: "disconnecting…", zh: "正在断开账户…" },
  "feishu.disconnectConfirm": {
    en: "Disconnect “{name}”? This removes the local login but does not delete the Feishu app.",
    zh: "断开“{name}”？这会清除本机登录，但不会删除飞书开放平台中的应用。",
  },
  "feishu.retry": { en: "try again", zh: "重试" },
  "feishu.close": { en: "close", zh: "关闭" },

  "task.hero": {
    en: "what should socai research?",
    zh: "想让 socai 研究什么？",
  },
  "task.lede": {
    en: "start a one-shot browser task. socai opens a temporary chrome tab, runs the agent, saves the result, then closes the tab.",
    zh: "启动一次性浏览器任务。socai 会打开临时 chrome 标签页、运行智能体、保存结果，然后关闭标签页。",
  },
  "task.addKeyHint": {
    en: "add an api key in the model menu (top right) to run.",
    zh: "在右上角模型菜单中添加 api key 后即可运行。",
  },
  "task.agentPlaceholder": {
    en: "tell socai what you want researched…",
    zh: "告诉 socai 你想研究什么…",
  },
  "task.today": { en: "today", zh: "今天" },
  "task.yesterday": { en: "yesterday", zh: "昨天" },

  "note.seen": { en: "notes the agent saw", zh: "智能体看过的笔记" },
  "note.openOriginal": { en: "open original ↗", zh: "查看原文 ↗" },
  "note.videoUnavailable": { en: "video unavailable", zh: "视频不可用" },
  "note.noMedia": { en: "no media", zh: "无媒体" },
  "note.likes": { en: "likes", zh: "赞" },
  "note.saves": { en: "saves", zh: "收藏" },
  "note.comments": { en: "comments", zh: "评论" },
} as const satisfies Record<string, Record<Language, string>>;

const taskStatusLabels = {
  queued: { en: "queued", zh: "排队中" },
  running: { en: "running", zh: "运行中" },
  completed: { en: "completed", zh: "已完成" },
  failed: { en: "failed", zh: "失败" },
  cancelled: { en: "cancelled", zh: "已取消" },
  interrupted: { en: "interrupted", zh: "已中断" },
} as const satisfies Record<TaskStatusKey, Record<Language, string>>;

type MessageKey = keyof typeof messages;

let currentLanguage: Language = readInitialLanguage();

export function getLanguage(): Language {
  return currentLanguage;
}

export function isSupportedLanguage(
  language: string | null | undefined,
): language is Language {
  return !!language && supportedLanguages.includes(language as Language);
}

export function setLanguage(language: Language): void {
  currentLanguage = language;
  try {
    window.localStorage.setItem(STORAGE_KEY, language);
  } catch {
    // Ignore storage failures; the active session can still switch languages.
  }
  applyLanguageToDocument();
}

export function applyLanguageToDocument(): void {
  document.documentElement.lang = toHtmlLanguage(currentLanguage);
  document.documentElement.dataset.language = currentLanguage;
}

export function t(
  key: MessageKey,
  params: Record<string, string | number> = {},
): string {
  let message: string = messages[key][currentLanguage];
  for (const [name, value] of Object.entries(params)) {
    message = message.replaceAll(`{${name}}`, `${value}`);
  }
  return message;
}

const taskPreflightMessages = {
  preflight_model_config: "task.preflightModelConfig",
  preflight_auth: "task.preflightAuth",
  preflight_balance: "task.preflightBalance",
  preflight_region_or_account: "task.preflightAccount",
  preflight_site: "task.preflightSite",
  preflight_browser_config: "task.preflightBrowserConfig",
  preflight_browser: "task.preflightBrowser",
  preflight_browser_remote: "task.preflightBrowserRemote",
  preflight_browser_remote_quota: "task.preflightBrowserRemoteQuota",
  preflight_xhs_login: "task.preflightXhsLogin",
  preflight_xhs_session: "task.preflightXhsSession",
} as const satisfies Record<string, MessageKey>;

export function formatTaskCommandError(error: unknown): string {
  const presentation = formatTaskCommandErrorPresentation(error);
  if (!presentation) return String(error);
  return `${presentation.message}\n${presentation.meta}`;
}

export function formatTaskCommandErrorPresentation(
  error: unknown,
): TaskApiErrorPresentation | null {
  const payload = parseTaskCommandError(error);
  if (!payload) return null;
  const messageKey = taskPreflightMessages[payload.code as keyof typeof taskPreflightMessages]
    ?? "task.preflightUnknown";
  const nudge = messageKey === "task.preflightModelConfig" ? managedModelNudge() : "";
  return {
    title: t("task.preflightFailedTitle"),
    message: joinSentences(t(messageKey), nudge),
    meta: t("task.errorCode", { code: payload.code }),
    fingerprint: `${payload.code}:${payload.detail}`,
  };
}

// Whether a socai account is signed in on this device. The api-error copy needs
// it to choose between offering the sign-up bonus and pointing an already
// signed-in user at the built-in model. Pushed in by the auth panel rather than
// read from it, since panels/auth.ts imports this module.
let accountSignedIn = false;

export function setAccountSignedIn(signedIn: boolean): void {
  accountSignedIn = signedIn;
}

/** Where the credential that failed came from. `own_key` is a user-pasted api
 *  key, `codex` a connected chatgpt subscription, `managed` the socai gateway
 *  (points, no key), `unknown` an error whose provider could not be parsed. */
type ApiErrorCredential = "own_key" | "codex" | "managed" | "unknown";

/** Copy that blames the user-supplied api key instead of "the current" one. */
const ownKeyMessages: Partial<Record<MessageKey, MessageKey>> = {
  "task.apiErrorAuth": "task.apiErrorAuthOwnKey",
  "task.apiErrorBalance": "task.apiErrorBalanceOwnKey",
  "task.apiErrorUsageLimit": "task.apiErrorUsageLimitOwnKey",
  "task.apiErrorForbidden": "task.apiErrorForbiddenOwnKey",
  "task.apiErrorModelNotActivated": "task.apiErrorModelNotActivatedOwnKey",
  "task.apiErrorRateLimit": "task.apiErrorRateLimitOwnKey",
};

/** The socai gateway spends account points, so its failures are account
 *  problems rather than api key problems. */
const managedMessages: Partial<Record<MessageKey, MessageKey>> = {
  "task.apiErrorAuth": "task.preflightAuth",
  "task.apiErrorBalance": "task.preflightBalance",
  "task.apiErrorUsageLimit": "task.preflightBalance",
};

export interface TaskApiErrorPresentation {
  title: string;
  message: string;
  meta: string;
  fingerprint: string;
}

export function formatTaskApiError(error: string): TaskApiErrorPresentation {
  const parsed = parseTaskApiError(error);
  const provider = parsed.provider;
  const status = parsed.status;
  const signal = parsed.signal;
  const explicitCode = parsed.code || parsed.type;
  const entitlementCode = `${parsed.code} ${parsed.type}`.toLowerCase();
  const detail = parsed.message.toLowerCase();
  const modelNotActivated = (status === 403 || status === 404)
    && (
      /\bmodel[_-]?not[_-]?(?:activated|open)\b/.test(entitlementCode)
      || /model (?:is )?not (?:activated|enabled|open)|not (?:activated|enabled|opened) (?:the )?model/.test(detail)
      || /模型(?:尚未|未)开通|(?:尚未|未)开通(?:此|该|所选)?模型/.test(detail)
    );
  let fallbackCode: string;
  let titleKey: MessageKey;
  let messageKey: MessageKey;
  if (modelNotActivated) {
    titleKey = "task.apiErrorModelNotActivatedTitle";
    messageKey = "task.apiErrorModelNotActivated";
    fallbackCode = "model_not_activated";
  } else if (status === 401 || /auth|unauthor|invalid.*(?:api|x-api).*key/.test(signal)) {
    titleKey = "task.apiErrorAuthTitle";
    messageKey = "task.apiErrorAuth";
    fallbackCode = "authentication_error";
  } else if (/usage_limit_reached|usage limit (?:has been )?reached/.test(signal)) {
    titleKey = "task.apiErrorUsageLimitTitle";
    messageKey = "task.apiErrorUsageLimit";
    fallbackCode = "usage_limit_reached";
  } else if (
    status === 402
    || /insufficient.*(?:balance|point|credit|quota)|quota.*(?:exhausted|exceeded)|billing.*limit|credit.*balance/.test(signal)
  ) {
    titleKey = "task.apiErrorBalanceTitle";
    messageKey = "task.apiErrorBalance";
    fallbackCode = "insufficient_quota";
  } else if (status === 403 || /forbidden|permission|access denied/.test(signal)) {
    titleKey = "task.apiErrorForbiddenTitle";
    messageKey = "task.apiErrorForbidden";
    fallbackCode = "permission_denied";
  } else if (
    /server_is_overloaded|overloaded_error|currently overloaded|service.*overload/.test(signal)
  ) {
    titleKey = "task.apiErrorOverloadedTitle";
    messageKey = "task.apiErrorOverloaded";
    fallbackCode = "server_is_overloaded";
  } else if (status === 429 || /rate_limit|rate.?limit|too many requests/.test(signal)) {
    titleKey = "task.apiErrorRateLimitTitle";
    messageKey = "task.apiErrorRateLimit";
    fallbackCode = "rate_limit_error";
  } else if (status !== null && status >= 500) {
    titleKey = "task.apiErrorUnavailableTitle";
    messageKey = "task.apiErrorUnavailable";
    fallbackCode = `http_${status}`;
  } else if (/timed out|timeout|os error 10060/.test(signal)) {
    titleKey = "task.apiErrorTimeoutTitle";
    messageKey = "task.apiErrorTimeout";
    fallbackCode = "network_timeout";
  } else if (/network|connect|timeout|timed out|dns/.test(signal)) {
    titleKey = "task.apiErrorNetworkTitle";
    messageKey = "task.apiErrorNetwork";
    fallbackCode = "network_connect_error";
  } else {
    titleKey = "task.apiErrorGenericTitle";
    messageKey = "task.apiErrorGeneric";
    fallbackCode = "api_error";
  }

  const credential = credentialSource(parsed.providerSlug);
  const errorCode = explicitCode || fallbackCode;
  const meta = [
    t("task.errorCode", { code: errorCode }),
    status !== null ? `HTTP ${status}` : "",
    parsed.requestId ? t("task.apiErrorRequestId", { id: parsed.requestId }) : "",
  ].filter(Boolean).join(" · ");
  return {
    title: t(titleKey),
    message: taskApiErrorMessage(messageKey, credential, provider),
    meta,
    fingerprint: parsed.requestId || `${provider}:${parsed.statusText}:${errorCode}:${parsed.detail}`,
  };
}

/** The category copy for `credential`, plus — when the user's own credential is
 *  what ran out — the nudge toward the built-in model they can use instead. */
function taskApiErrorMessage(
  messageKey: MessageKey,
  credential: ApiErrorCredential,
  provider: string,
): string {
  const resolved = credential === "own_key"
    ? ownKeyMessages[messageKey] ?? messageKey
    : credential === "managed"
    ? managedMessages[messageKey] ?? messageKey
    : messageKey;
  const nudge = (credential === "own_key" || credential === "codex")
      && ownKeyMessages[messageKey] !== undefined
    ? managedModelNudge()
    : "";
  return joinSentences(t(resolved, { provider }), nudge);
}

/** Sentence separator for stitched copy: chinese needs none after "。". */
function joinSentences(...parts: string[]): string {
  return parts.filter(Boolean).join(currentLanguage === "zh" ? "" : " ");
}

/** Offers socai's built-in model as the way out of a credential the user owns:
 *  the sign-up bonus when there is no account yet, a switch hint once there is. */
function managedModelNudge(): string {
  return t(accountSignedIn ? "task.apiErrorSwitchManagedHint" : "task.apiErrorSignUpHint", {
    points: SIGNUP_BONUS_POINTS,
  });
}

function credentialSource(slug: string): ApiErrorCredential {
  if (!slug) return "unknown";
  if (slug === "socai") return "managed";
  if (slug === "openai-codex") return "codex";
  return "own_key";
}

export function isTaskApiError(error: string): boolean {
  return /\bAPI error\b|(?:Responses|Chat Completions|Messages?) request failed|error sending request for url|stream ended without response\.completed|model output was truncated|usage_limit_reached|server_is_overloaded/i
    .test(error);
}

interface ParsedTaskApiError {
  provider: string;
  /** Normalized provider id ("openai-codex", "socai", …), "" when unknown. */
  providerSlug: string;
  status: number | null;
  statusText: string;
  code: string;
  type: string;
  message: string;
  detail: string;
  requestId: string;
  signal: string;
}

function parseTaskApiError(error: string): ParsedTaskApiError {
  const raw = error.replace(/^API error:\s*/i, "").trim();
  const segments = raw.split(/\s+\|\s+/);
  const header = segments[0] ?? "";
  const providerValue = header.match(/^(.+?)\s+API error$/i)?.[1]
    ?? header.match(/^(.+?)\s+(?:Responses|Chat Completions|Messages?) request failed(?::|$)/i)?.[1]
    ?? providerFromRequestUrl(raw);
  const fields = new Map<string, string>();
  for (const segment of segments.slice(1)) {
    const separator = segment.indexOf("=");
    if (separator <= 0) continue;
    fields.set(segment.slice(0, separator).trim().toLowerCase(), segment.slice(separator + 1).trim());
  }
  const embedded = embeddedErrorFields(raw);
  const statusText = fields.get("status") || embedded.status || "";
  const parsedStatus = Number.parseInt(statusText, 10);
  const status = Number.isFinite(parsedStatus) ? parsedStatus : null;
  const code = fields.get("code") || embedded.code || "";
  const type = fields.get("type") || embedded.type || "";
  const message = fields.get("message") || embedded.message || "";
  const detail = message || raw;
  const requestId = fields.get("request_id") || embedded.request_id || "";
  return {
    provider: providerLabel(providerValue),
    providerSlug: normalizeProviderSlug(providerValue),
    status,
    statusText,
    code,
    type,
    message,
    detail,
    requestId,
    signal: `${code} ${type} ${detail} ${raw}`.toLowerCase(),
  };
}

function embeddedErrorFields(raw: string): Record<string, string> {
  const jsonStart = raw.indexOf("{");
  if (jsonStart >= 0) {
    const root = parseJsonObject(raw.slice(jsonStart));
    if (root) {
      const response = objectValue(root.response);
      const detail = objectValue(response?.error) ?? objectValue(root.error);
      if (detail) {
        return {
          code: stringValue(detail.code),
          type: stringValue(detail.type),
          message: stringValue(detail.message),
          status: stringValue(response?.status ?? root.status),
          request_id: stringValue(response?.request_id ?? root.request_id),
        };
      }
    }
  }
  return {
    code: embeddedJsonString(raw, "code"),
    type: embeddedJsonString(raw, "type"),
    message: embeddedJsonString(raw, "message"),
    request_id: embeddedJsonString(raw, "request_id"),
  };
}

function embeddedJsonString(raw: string, field: string): string {
  const pattern = new RegExp(`"${field}"\\s*:\\s*"((?:\\\\.|[^"\\\\])*)"`, "i");
  const match = raw.match(pattern);
  if (!match) return "";
  try {
    return JSON.parse(`"${match[1]}"`) as string;
  } catch {
    return match[1];
  }
}

function parseJsonObject(value: string): Record<string, unknown> | null {
  try {
    return objectValue(JSON.parse(value));
  } catch {
    return null;
  }
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function stringValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number") return `${value}`;
  return "";
}

function providerFromRequestUrl(raw: string): string {
  const hostname = raw.match(/https?:\/\/([^/)\s]+)/i)?.[1]?.toLowerCase() ?? "";
  if (hostname === "chatgpt.com") return "openai-codex";
  if (hostname.includes("openai.com")) return "openai";
  if (hostname.includes("anthropic.com")) return "anthropic";
  if (hostname.includes("dashscope")) return "qwen";
  if (hostname.includes("deepseek")) return "deepseek";
  if (hostname.includes("volces.com")) return "doubao";
  return "";
}

function normalizeProviderSlug(provider: string): string {
  const normalized = provider.trim().toLowerCase().replace(/\s+/g, "-");
  return normalized === "openai-codex" || normalized === "codex" ? "openai-codex" : normalized;
}

function providerLabel(provider: string): string {
  const normalized = provider.trim().toLowerCase();
  if (normalized === "openai-codex" || normalized === "openai codex") {
    return "OpenAI Codex";
  }
  if (normalized === "openai") return "OpenAI";
  if (normalized === "anthropic") return "Anthropic";
  if (normalized === "qwen") return "Qwen";
  if (normalized === "deepseek") return "DeepSeek";
  if (normalized === "doubao") return "Doubao";
  return provider
    ? `${provider.charAt(0).toUpperCase()}${provider.slice(1)}`
    : t("task.apiErrorProvider");
}

function parseTaskCommandError(error: unknown): { code: string; detail: string } | null {
  if (typeof error === "object" && error !== null) {
    const payload = error as Record<string, unknown>;
    if (typeof payload.code === "string" && payload.code.startsWith("preflight_")) {
      return {
        code: payload.code,
        detail: typeof payload.detail === "string" ? payload.detail : "",
      };
    }
  }

  const raw = error instanceof Error ? error.message : String(error);
  try {
    const payload = JSON.parse(raw) as Record<string, unknown>;
    if (typeof payload.code === "string" && payload.code.startsWith("preflight_")) {
      return {
        code: payload.code,
        detail: typeof payload.detail === "string" ? payload.detail : "",
      };
    }
  } catch {
    const legacy = raw.match(/^(preflight_[a-z_]+)(?::\s*(.*))?$/s);
    if (legacy) return { code: legacy[1], detail: legacy[2] ?? "" };
  }
  return null;
}

export function getLocale(): string {
  return toHtmlLanguage(currentLanguage);
}

// Timezone preference. "system" (or empty) clears the override and follows the
// local zone; any other value is treated as an IANA id.
export function getTimezone(): string {
  return activeTimezone ?? "system";
}

export function setTimezone(timezone: string): void {
  activeTimezone = timezone && timezone !== "system" ? timezone : undefined;
  try {
    if (activeTimezone) {
      window.localStorage.setItem(TIMEZONE_STORAGE_KEY, activeTimezone);
    } else {
      window.localStorage.removeItem(TIMEZONE_STORAGE_KEY);
    }
  } catch {
    // Ignore storage failures; the active session still uses the chosen zone.
  }
}

export function taskStatusLabel(status: TaskStatusKey): string {
  return taskStatusLabels[status][currentLanguage];
}

export function formatTaskInterruptionMessage(message: string): string {
  const appClosed = "app was closed before this task finished";
  const normalized = message.trim().toLowerCase();
  if (normalized === appClosed || normalized === `[task interrupted: ${appClosed}]`) {
    return t("task.interruptedAppClosed");
  }
  return message;
}

export function formatTabs(count: number): string {
  if (currentLanguage === "zh") return `${count} 个标签页`;
  return `${count} tab${count === 1 ? "" : "s"}`;
}

export function formatTaskCount(count: number): string {
  if (currentLanguage === "zh") return `${count} 个任务`;
  return `${count} task${count === 1 ? "" : "s"}`;
}

export function formatStepCount(count: number): string {
  if (currentLanguage === "zh") return `${count} 步`;
  return `${count} step${count === 1 ? "" : "s"}`;
}

function formatTokenCount(value: number): string {
  const compact = (divisor: number, suffix: string) => {
    const scaled = value / divisor;
    const digits = scaled >= 100 ? 0 : 1;
    return `${Number(scaled.toFixed(digits))}${suffix}`;
  };
  if (value > 1_000_000) return compact(1_000_000, "M");
  if (value > 1_000) return compact(1_000, "K");
  return value.toLocaleString(getLocale());
}

export function formatTokenUsage(
  inputTokens: number,
  outputTokens: number,
  cachedInputTokens: number | null = null,
  cacheCreationInputTokens = 0,
  estimatedCost: number | null = null,
  costCurrency: string | null = null,
): string {
  const locale = getLocale();
  const number = formatTokenCount;
  const parts = currentLanguage === "zh"
    ? [`输入 ${number(inputTokens)} / 输出 ${number(outputTokens)} tokens`]
    : [`in ${number(inputTokens)} / out ${number(outputTokens)} tokens`];
  if (cachedInputTokens !== null) {
    const ratio = inputTokens > 0 ? ` (${(cachedInputTokens / inputTokens * 100).toFixed(1)}%)` : "";
    parts.push(currentLanguage === "zh"
      ? `缓存命中 ${number(cachedInputTokens)}${ratio}`
      : `cache hit ${number(cachedInputTokens)}${ratio}`);
  }
  if (cacheCreationInputTokens > 0) {
    parts.push(currentLanguage === "zh"
      ? `缓存写入 ${number(cacheCreationInputTokens)}`
      : `cache write ${number(cacheCreationInputTokens)}`);
  }
  if (estimatedCost !== null && costCurrency) {
    const cents = Math.trunc(estimatedCost * 100) / 100;
    let amount: string;
    try {
      amount = new Intl.NumberFormat(locale, {
        style: "currency",
        currency: costCurrency,
        minimumFractionDigits: 2,
        maximumFractionDigits: 2,
      }).format(cents);
    } catch {
      amount = `${cents.toFixed(2)} ${costCurrency}`;
    }
    parts.push(currentLanguage === "zh" ? `估算 ${amount}` : `est. ${amount}`);
  }
  return parts.join(" · ");
}

// Task timestamps read as "today 14:30" / "yesterday 09:15" for the last two
// calendar days, falling back to a localized date ("Jun 21 09:15", with the
// year appended only when it differs from now) for anything older.
export function formatTaskTimestamp(ms: number): string {
  if (!ms) return "";
  const date = new Date(ms);
  const time = date.toLocaleTimeString(getLocale(), {
    hour: "2-digit",
    minute: "2-digit",
    timeZone: activeTimezone,
  });
  return `${relativeDayLabel(date)} ${time}`;
}

// The today/yesterday bucket is computed in the system-local zone; the timezone
// preference only re-anchors the clock time and the fallback date string. The
// two can disagree within a few hours of midnight across distant zones, which
// is an acceptable tradeoff for a display-only preference.
function relativeDayLabel(date: Date): string {
  const now = new Date();
  const startOfToday = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate(),
  ).getTime();
  const startOfDate = new Date(
    date.getFullYear(),
    date.getMonth(),
    date.getDate(),
  ).getTime();
  const dayDiff = Math.round((startOfToday - startOfDate) / 86_400_000);
  if (dayDiff === 0) return t("task.today");
  if (dayDiff === 1) return t("task.yesterday");
  const sameYear = date.getFullYear() === now.getFullYear();
  return date.toLocaleDateString(getLocale(), {
    month: "short",
    day: "numeric",
    timeZone: activeTimezone,
    ...(sameYear ? {} : { year: "numeric" }),
  });
}

function readInitialLanguage(): Language {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (isSupportedLanguage(stored)) return stored;
  } catch {
    // Ignore storage errors and fall back to the default language.
  }

  return DEFAULT_LANGUAGE;
}

function readInitialTimezone(): string | undefined {
  try {
    const stored = window.localStorage.getItem(TIMEZONE_STORAGE_KEY);
    if (stored && stored !== "system") return stored;
  } catch {
    // Ignore storage errors and follow the system local zone.
  }
  return undefined;
}

function toHtmlLanguage(language: Language): string {
  return language === "zh" ? "zh-CN" : "en";
}
