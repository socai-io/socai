import {isPermissionGranted, requestPermission, sendNotification} from "@tauri-apps/plugin-notification";
import type {AgentTaskEventPayload} from "../main";
import {getLanguage} from "./i18n";

const preference = "socai-desktop-completion-notifications";
const openedAt = Date.now();
const claimed = new Set<string>();
let sessionEnabled: boolean | undefined;

export function completionNotificationsEnabled(): boolean {
  if (sessionEnabled !== undefined) return sessionEnabled;
  try { return localStorage.getItem(preference) !== "false"; }
  catch { return true; }
}

export function setCompletionNotificationsEnabled(enabled: boolean): void {
  sessionEnabled = enabled;
  try { localStorage.setItem(preference, String(enabled)); } catch { /* Keep this session's choice. */ }
  if (enabled) requestCompletionNotificationPermission();
}

export function requestCompletionNotificationPermission(): void {
  if (!completionNotificationsEnabled()) return;
  // Called from explicit Send or settings actions, never during app startup.
  void isPermissionGranted().then(async granted => { if (!granted) await requestPermission(); }).catch(error => {
    console.warn("notification permission unavailable:", error);
  });
}

export async function notifyTaskCompletion(event: AgentTaskEventPayload): Promise<void> {
  // The final registry snapshot is authoritative; raw agent `done` may precede
  // cancellation, settlement or persistence failures.
  if (event.kind !== "completed" || event.snapshot?.status !== "completed") return;
  const runId = event.snapshot.run_id;
  const finishedAt = event.snapshot.finished_at;
  if (!event.task_id || !runId || finishedAt == null || !Number.isFinite(finishedAt) || finishedAt < openedAt) return;
  const key = `${event.task_id}:${runId}`;
  if (claimed.has(key)) return;
  claimed.add(key);
  if (claimed.size > 1000) claimed.delete(claimed.values().next().value!);
  if (!completionNotificationsEnabled() || (document.visibilityState === "visible" && document.hasFocus())) return;
  try {
    if (!await isPermissionGranted() || !completionNotificationsEnabled()) return;
    sendNotification({title: "socai", body: getLanguage() === "zh"
      ? "任务运行已结束，打开 Socai 查看结果" : "Run finished. Open Socai to view the result."});
  } catch (error) {
    // Notification failures must not change a completed run or retry the task.
    console.warn("completion notification failed:", error);
  }
}
