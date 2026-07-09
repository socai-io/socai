export type Language = "en" | "zh";
export type TaskStatusKey =
  "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted";

const DEFAULT_LANGUAGE: Language = "zh";
const STORAGE_KEY = "socai-language";
const TIMEZONE_STORAGE_KEY = "socai-timezone";
const supportedLanguages: Language[] = ["zh", "en"];

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
  "chrome.connectCta": { en: "connect chrome →", zh: "连接 chrome →" },
  "chrome.connectingCta": { en: "connecting…", zh: "连接中…" },
  "chrome.remoteDebuggingHelp": {
    en: "how do i enable remote debugging? ↗",
    zh: "如何启用远程调试？↗",
  },

  "status.capsuleAria": {
    en: "chrome and agent status",
    zh: "chrome 与智能体状态",
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
  "settings.pro": { en: "pro", zh: "pro" },
  "settings.inviteCode": { en: "invite code", zh: "邀请码" },
  "settings.activate": { en: "activate", zh: "激活" },
  "settings.proActivated": { en: "activated", zh: "已激活" },
  "settings.proNotActivated": { en: "not activated", zh: "未激活" },
  "settings.proHint": {
    en: "required for video transcripts and future pro features.",
    zh: "视频帖子提取音频，等其他 pro 功能。",
  },
  "settings.browse": { en: "browse…", zh: "浏览…" },
  "settings.chrome": { en: "chrome", zh: "chrome" },
  "settings.source": { en: "source", zh: "来源" },
  "settings.sourceManaged": { en: "isolated profile", zh: "独立配置文件" },
  "settings.sourceExisting": { en: "existing browser", zh: "现有浏览器" },
  "settings.profileDir": { en: "profile directory", zh: "资料目录" },
  "settings.profileHint": {
    en: "socai launches a throwaway chrome with this profile.",
    zh: "socai 会用此资料目录启动临时 chrome。",
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
  "settings.loadFailed": {
    en: "could not load settings.",
    zh: "无法加载设置。",
  },
  "settings.autosaveHint": {
    en: "changes are saved automatically.",
    zh: "更改会自动保存。",
  },

  "agent.label": { en: "model", zh: "模型" },
  "agent.configurationAria": { en: "agent configuration", zh: "智能体设置" },
  "agent.selectModelAria": { en: "select agent model", zh: "选择智能体模型" },
  "agent.selectProviderAria": {
    en: "select agent provider",
    zh: "选择智能体服务商",
  },
  "agent.provider": { en: "provider", zh: "服务商" },
  "agent.apiKey": { en: "api key", zh: "api key" },
  "agent.credentialConfigured": {
    en: "{provider} api key configured.",
    zh: "{provider} api key 已配置。",
  },
  "agent.chatgptConnected": {
    en: "chatgpt subscription connected.",
    zh: "chatgpt 订阅已连接。",
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
  "task.searchLabel": { en: "search", zh: "搜索" },
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

export function formatTokenUsage(
  inputTokens: number,
  outputTokens: number,
): string {
  if (currentLanguage === "zh")
    return `输入 ${inputTokens} / 输出 ${outputTokens} tokens`;
  return `in ${inputTokens} / out ${outputTokens} tokens`;
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
