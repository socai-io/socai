export type Language = "en" | "zh";
export type TaskStatusKey = "queued" | "running" | "completed" | "failed" | "cancelled" | "interrupted";

const DEFAULT_LANGUAGE: Language = "zh";
const STORAGE_KEY = "socai-language";
const supportedLanguages: Language[] = ["zh", "en"];

const messages = {
  "language.switcherAria": { en: "language", zh: "语言" },

  "common.loading": { en: "loading…", zh: "加载中…" },
  "common.or": { en: "or,", zh: "或" },
  "common.save": { en: "save", zh: "保存" },
  "common.saving": { en: "saving…", zh: "保存中…" },

  "chrome.label": { en: "chrome", zh: "chrome" },
  "chrome.connectAria": { en: "connect chrome", zh: "连接 chrome" },
  "chrome.statusToggleAria": { en: "show chrome connection status", zh: "显示 chrome 连接状态" },
  "chrome.dialogAria": { en: "chrome connection status", zh: "chrome 连接状态" },
  "chrome.requiredAria": { en: "chrome required", zh: "需要 chrome" },
  "chrome.disconnected": { en: "disconnected", zh: "未连接" },
  "chrome.connecting": { en: "connecting", zh: "连接中" },
  "chrome.connected": { en: "connected", zh: "已连接" },
  "chrome.tabs": { en: "tabs", zh: "标签页" },
  "chrome.browser": { en: "browser", zh: "浏览器" },
  "chrome.endpoint": { en: "endpoint", zh: "端点" },
  "chrome.source": { en: "source", zh: "来源" },
  "chrome.sourceManaged": { en: "isolated profile", zh: "独立资料目录" },
  "chrome.sourceExisting": { en: "existing browser", zh: "现有浏览器" },
  "chrome.profile": { en: "profile", zh: "资料目录" },
  "chrome.disconnect": { en: "disconnect", zh: "断开连接" },
  "chrome.lookingForChrome": { en: "looking for chrome…", zh: "正在寻找 chrome…" },
  "chrome.connectToStart": { en: "connect chrome to start", zh: "连接 chrome 后开始" },
  "chrome.connectCta": { en: "connect chrome →", zh: "连接 chrome →" },
  "chrome.connectingCta": { en: "connecting…", zh: "连接中…" },
  "chrome.remoteDebuggingHelp": {
    en: "how do i enable remote debugging? ↗",
    zh: "如何启用远程调试？↗",
  },

  "update.available": { en: "update available", zh: "有可用更新" },
  "update.toggleAria": { en: "show update status", zh: "显示更新状态" },
  "update.dialogAria": { en: "app update", zh: "应用更新" },
  "update.title": { en: "update", zh: "更新" },
  "update.newVersion": { en: "new version", zh: "新版本" },
  "update.upgradeAndRestart": { en: "upgrade & restart", zh: "升级并重启" },
  "update.downloading": { en: "downloading…", zh: "下载中…" },
  "update.readyTitle": { en: "update ready", zh: "更新就绪" },
  "update.readyHint": { en: "restart to finish the update.", zh: "重启以完成更新。" },
  "update.restartToFinish": { en: "restart to finish", zh: "待重启" },
  "update.restartNow": { en: "restart now", zh: "立即重启" },
  "update.failed": { en: "update failed", zh: "更新失败" },
  "update.retry": { en: "try again", zh: "重试" },
  "update.viewOnGithub": { en: "view on github ↗", zh: "在 github 查看 ↗" },

  "agent.label": { en: "model", zh: "模型" },
  "agent.configurationAria": { en: "agent configuration", zh: "智能体设置" },
  "agent.selectModelAria": { en: "select agent model", zh: "选择智能体模型" },
  "agent.selectProviderAria": { en: "select agent provider", zh: "选择智能体服务商" },
  "agent.provider": { en: "provider", zh: "服务商" },
  "agent.apiKey": { en: "api key", zh: "api key" },
  "agent.credentialConfigured": { en: "{provider} credential configured.", zh: "{provider} 凭证已配置。" },
  "agent.modelVersion": { en: "model version", zh: "模型版本" },
  "agent.loading": { en: "loading", zh: "加载中" },
  "agent.keyNeeded": { en: "api key needed", zh: "需要 api key" },
  "agent.defaultModel": { en: "default", zh: "默认" },
  "agent.needsCredential": { en: "{model} needs an api key.", zh: "{model} 需要 api key。" },
  "agent.connectChatgpt": { en: "connect chatgpt subscription", zh: "连接 chatgpt 订阅" },
  "agent.opening": { en: "opening…", zh: "打开中…" },
  "agent.pasteApiKey": { en: "paste api key", zh: "粘贴 api key" },
  "agent.codexLoginMissing": {
    en: "codex login not detected yet. return to socai after login completes.",
    zh: "还没有检测到 codex 登录。登录完成后返回 socai。",
  },

  "task.pagesAria": { en: "task pages", zh: "任务页面" },
  "task.new": { en: "new task", zh: "新任务" },
  "task.history": { en: "history", zh: "历史" },
  "task.historyTitle": { en: "task history", zh: "任务历史" },
  "task.historyDescription": {
    en: "review completed, failed, interrupted, and running tasks.",
    zh: "查看已完成、失败、中断和运行中的任务。",
  },
  "task.historyAria": { en: "task history", zh: "任务历史" },
  "task.selected": { en: "selected task", zh: "已选任务" },
  "task.selectedAria": { en: "selected task", zh: "已选任务" },
  "task.noTasks": { en: "no tasks yet.", zh: "暂无任务。" },
  "task.cancel": { en: "cancel", zh: "取消" },
  "task.finalAnswer": { en: "final answer", zh: "最终答案" },
  "task.waitingForEvents": { en: "waiting for events…", zh: "等待事件…" },
  "task.noTimeline": { en: "no event timeline available.", zh: "暂无事件时间线。" },
  "task.emptyDetail": { en: "start a task or choose one from history.", zh: "启动一个任务或从历史中选择。" },
  "task.run": { en: "run", zh: "运行" },

  "task.hero": { en: "what should socai research?", zh: "想让 socai 研究什么？" },
  "task.lede": {
    en: "start a one-shot browser task. socai opens a temporary chrome tab, runs the agent, saves the result, then closes the tab.",
    zh: "启动一次性浏览器任务。socai 会打开临时 chrome 标签页、运行智能体、保存结果，然后关闭标签页。",
  },
  "task.modeAria": { en: "task mode", zh: "任务模式" },
  "task.modeAgent": { en: "agent tasks", zh: "智能体任务" },
  "task.modeTools": { en: "tool tests", zh: "工具测试" },
  "task.starting": { en: "starting…", zh: "启动中…" },
  "task.runTest": { en: "run test", zh: "运行测试" },
  "task.loadingModels": { en: "loading agent models…", zh: "正在加载智能体模型…" },
  "task.addKeyHint": { en: "add an api key in the model menu (top right) to run.", zh: "在右上角模型菜单中添加 api key 后即可运行。" },
  "task.agentPlaceholder": {
    en: "tell socai what you want researched…\neach task opens its own temporary chrome tab.",
    zh: "告诉 socai 你想研究什么…\n每个任务都会打开自己的临时 chrome 标签页。",
  },
  "task.recent": { en: "recent", zh: "最近" },
  "task.viewHistory": { en: "view history", zh: "查看历史" },
  "task.noRecent": { en: "no recent tasks yet.", zh: "暂无最近任务。" },

  "tool.pickerAria": { en: "tool", zh: "工具" },
  "tool.searchNotes": { en: "search notes", zh: "搜索笔记" },
  "tool.topicScan": { en: "topic scan", zh: "话题扫描" },
  "tool.extractNote": { en: "extract note", zh: "提取笔记" },
  "tool.hintSearchNotes": {
    en: "test search_notes on a fresh temporary xiaohongshu tab.",
    zh: "在新的临时 xiaohongshu 标签页中测试 search_notes。",
  },
  "tool.hintTopicScan": {
    en: "test topic_scan on a fresh temporary xiaohongshu tab.",
    zh: "在新的临时 xiaohongshu 标签页中测试 topic_scan。",
  },
  "tool.hintExtractNote": {
    en: "paste a note id or url; socai opens a fresh temporary page and extracts it.",
    zh: "粘贴笔记 id 或 url；socai 会打开新的临时页面并提取内容。",
  },
  "tool.placeholderSearch": { en: "search query…", zh: "搜索关键词…" },
  "tool.placeholderTopic": { en: "topic to scan…", zh: "要扫描的话题…" },
  "tool.placeholderNote": { en: "note id or url…", zh: "笔记 id 或 url…" },
  "tool.running": { en: "running", zh: "运行中" },
  "tool.noResult": { en: "no tool test result yet.", zh: "暂无工具测试结果。" },
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

export function isSupportedLanguage(language: string | null | undefined): language is Language {
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

export function t(key: MessageKey, params: Record<string, string | number> = {}): string {
  let message: string = messages[key][currentLanguage];
  for (const [name, value] of Object.entries(params)) {
    message = message.replaceAll(`{${name}}`, `${value}`);
  }
  return message;
}

export function getLocale(): string {
  return toHtmlLanguage(currentLanguage);
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

export function formatRunningTaskCount(count: number): string {
  if (currentLanguage === "zh") return `${count} 个任务运行中`;
  return `${count} task${count === 1 ? "" : "s"} running`;
}

export function formatTurns(count: number): string {
  if (currentLanguage === "zh") return `${count} 轮`;
  return `${count} turn${count === 1 ? "" : "s"}`;
}

export function formatTokenUsage(inputTokens: number, outputTokens: number): string {
  if (currentLanguage === "zh") return `输入 ${inputTokens} / 输出 ${outputTokens} tokens`;
  return `in ${inputTokens} / out ${outputTokens} tokens`;
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

function toHtmlLanguage(language: Language): string {
  return language === "zh" ? "zh-CN" : "en";
}
