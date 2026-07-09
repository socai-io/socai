//! Sidebar (task history list) + the universal delete-confirm dialog.
//! The selected task itself renders as an inline conversation — see
//! `conversation.ts`.

import { esc } from "../lib/html";
import { formatTaskCount, formatTaskTimestamp, taskStatusLabel, t } from "../lib/i18n";
import type { AgentTaskView } from "./tasks";

export interface SidebarProps {
  tasks: AgentTaskView[];
  selectedTaskId: string | null;
  /** True while the compose view is showing — no row should read as selected. */
  composing: boolean;
}

export function renderSidebar(props: SidebarProps): string {
  return `
    <aside class="sidebar" aria-label="${esc(t("task.historyAria"))}">
      <div class="sidebar-head">
        <button id="sidebar-new" type="button" class="sidebar-new">
          <span class="sidebar-new-glyph" aria-hidden="true">+</span>${esc(t("task.new"))}
        </button>
      </div>
      <div class="sidebar-list-head">
        <p class="t-eyebrow result-label">${esc(t("task.history"))}</p>
        <span class="t-small subtle">${esc(formatTaskCount(props.tasks.length))}</span>
      </div>
      <div class="sidebar-list">
        ${renderTaskRows(props)}
      </div>
    </aside>
  `;
}

// A history row is a non-interactive container holding two sibling controls:
// a real <button> covering the glyph+title+meta (click/Enter/Space opens the
// task) and, for finished tasks, a quiet × button that surfaces on hover/focus
// (running/queued must be cancelled first — cancel lives in the conversation
// head). Two siblings, never nested: an interactive control inside a
// role="button" is invalid ARIA and would fold the ×'s label into the row's
// name. Every delete affordance routes through the universal centered confirm
// dialog — nothing is destroyed until confirmed.
function renderTaskRows(props: SidebarProps): string {
  if (props.tasks.length === 0) {
    return `<p class="t-small placeholder task-list-empty">${esc(t("task.noTasks"))}</p>`;
  }
  return [...props.tasks]
    .sort((a, b) => b.created_at - a.created_at)
    .map((task) => {
      const active = !props.composing && task.task_id === props.selectedTaskId ? "task-row-active" : "";
      const running = task.status === "running" || task.status === "queued";
      return `
        <div class="task-row ${active}">
          <button type="button" class="task-row-open" data-task-id="${esc(task.task_id)}">
            <span class="task-row-glyph task-row-glyph-${esc(task.status)}" aria-hidden="true">${taskStatusGlyph(task.status)}</span>
            <span class="task-row-main">
              <span class="task-row-title">${esc(task.task)}</span>
              <span class="task-row-meta">${esc(taskStatusLabel(task.status))} · ${esc(formatTaskTimestamp(task.created_at))}</span>
            </span>
          </button>
          ${running ? "" : `
          <button
            type="button"
            class="task-row-delete"
            data-delete-task="${esc(task.task_id)}"
            aria-label="${esc(t("task.deleteAria"))} · ${esc(task.task)}"
            title="${esc(t("task.deleteAria"))}"
          >×</button>`}
        </div>
      `;
    })
    .join("");
}

// Universal delete confirmation — centered alertdialog on a dimmed scrim.
// Esc, scrim-click, or keep dismisses; delete commits. Warns that the task
// and ALL its artifacts are removed permanently. Bound in tasks.ts.
export function renderConfirmDeleteDialog(task: AgentTaskView): string {
  return `
    <div class="modal-scrim" data-delete-dismiss>
      <div class="confirm-dialog" role="alertdialog" aria-modal="true" aria-label="${esc(t("task.deleteAria"))}">
        <p class="confirm-dialog-title">${esc(t("task.deleteQuestion"))}</p>
        <p class="t-small confirm-dialog-task">“${esc(task.task)}”</p>
        <p class="t-small subtle">${esc(t("task.deleteWarn"))}</p>
        <div class="confirm-dialog-actions">
          <button id="confirm-delete-keep" type="button" class="btn-ghost btn-compact">${esc(t("task.deleteKeep"))}</button>
          <button id="confirm-delete-commit" type="button" class="btn-primary btn-compact">${esc(t("task.delete"))}</button>
        </div>
      </div>
    </div>
  `;
}

function taskStatusGlyph(status: AgentTaskView["status"]): string {
  switch (status) {
    case "queued": return "○";
    case "running": return "●";
    case "completed": return "✓";
    case "failed": return "×";
    case "cancelled": return "−";
    case "interrupted": return "!";
  }
}
