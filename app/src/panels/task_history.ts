//! Sidebar (task history list) + the selected-task detail pane.
//!
//! The detail body uses the "split" layout: timeline ‖ final answer, each
//! filling the pane. When only one panel has content — a running task with no
//! answer yet, or a failed task that produced only an error — it falls back to a
//! single stacked panel. (Narrow windows fold split into a stack via CSS.)
//!
//! Below the body, a full-width filmstrip shows the notes the agent saw.

import { convertFileSrc } from "@tauri-apps/api/core";
import type { AgentTaskEventPayload, NoteMediaAsset, NoteRecord } from "../main";
import { esc } from "../lib/html";
import { renderMarkdown } from "../lib/markdown";
import { formatTaskCount, formatTaskTimestamp, formatTokenUsage, formatTurns, taskStatusLabel, t } from "../lib/i18n";
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

function renderTaskRows(props: SidebarProps): string {
  if (props.tasks.length === 0) {
    return `<p class="t-small placeholder task-list-empty">${esc(t("task.noTasks"))}</p>`;
  }
  return [...props.tasks]
    .sort((a, b) => b.created_at - a.created_at)
    .map((task) => {
      const active = !props.composing && task.task_id === props.selectedTaskId ? "task-row-active" : "";
      return `
        <button type="button" class="task-row ${active}" data-task-id="${esc(task.task_id)}">
          <span class="task-row-glyph task-row-glyph-${esc(task.status)}" aria-hidden="true">${taskStatusGlyph(task.status)}</span>
          <span class="task-row-main">
            <span class="task-row-title">${esc(task.task)}</span>
            <span class="task-row-meta">${esc(taskStatusLabel(task.status))} · ${esc(formatTaskTimestamp(task.created_at))}</span>
          </span>
        </button>
      `;
    })
    .join("");
}

export function renderTaskDetail(task: AgentTaskView | undefined): string {
  if (!task) return renderEmptyDetail();

  const running = task.status === "running" || task.status === "queued";
  const hasTimeline = task.events.length > 0 || running;
  const hasResult = !!task.final_text || !!task.error;
  const bothPanels = hasTimeline && hasResult;

  let body: string;
  if (bothPanels) {
    body = `
      <div class="detail-split">
        <div class="detail-col">${renderTimelinePanel(task)}</div>
        <div class="detail-col">${renderResultPanel(task)}</div>
      </div>
    `;
  } else {
    body = `
      <div class="detail-body detail-body--stacked">
        ${hasTimeline ? renderTimelinePanel(task) : ""}
        ${hasResult ? renderResultPanel(task) : ""}
        ${!hasTimeline && !hasResult ? `<p class="t-small placeholder">${esc(t("task.noTimeline"))}</p>` : ""}
      </div>
    `;
  }

  return `${renderDetailHead(task, running)}${body}${renderNotesSection(task)}`;
}

function renderDetailHead(task: AgentTaskView, running: boolean): string {
  const time = formatTaskTimestamp(task.started_at ?? task.created_at);
  const duration = formatDuration(task);
  const tokens = task.input_tokens !== null && task.output_tokens !== null
    ? formatTokenUsage(task.input_tokens, task.output_tokens)
    : "";
  const dotClass = running ? "badge-dot-ink badge-dot-pulse" : "badge-dot-hollow";
  const items = [
    `<span class="task-meta-item task-meta-status"><i class="badge-dot ${dotClass}" aria-hidden="true"></i>${esc(taskStatusLabel(task.status))}</span>`,
    time ? `<span class="task-meta-item">${esc(time)}</span>` : "",
    duration ? `<span class="task-meta-item">${esc(duration)}</span>` : "",
    task.model ? `<span class="task-meta-item t-mono">${esc(task.model)}</span>` : "",
    task.turns !== null && task.turns !== undefined ? `<span class="task-meta-item">${esc(formatTurns(task.turns))}</span>` : "",
    tokens ? `<span class="task-meta-item">${esc(tokens)}</span>` : "",
  ].join("");
  return `
    <div class="task-detail-head">
      <div class="task-detail-headinfo">
        <h2 class="t-h3 task-detail-title">${esc(task.task)}</h2>
        <div class="task-detail-meta">${items}</div>
      </div>
      ${running ? `
        <div class="task-detail-actions">
          <button type="button" class="btn-ghost btn-compact" data-cancel-task="${esc(task.task_id)}">${esc(t("task.cancel"))}</button>
        </div>` : ""}
    </div>
  `;
}

function renderTimelinePanel(task: AgentTaskView): string {
  const hasEvents = task.events.length > 0;
  const rows = hasEvents
    ? task.events.map(renderAgentEvent).join("")
    : `<p class="t-small placeholder" data-events-placeholder>${esc(t("task.waitingForEvents"))}</p>`;
  return `
    <div class="result-block detail-panel">
      <p class="t-eyebrow result-label detail-panel-label">${esc(t("task.timeline"))}</p>
      <div class="event-stream" data-agent-events="${esc(task.task_id)}">${rows}</div>
    </div>
  `;
}

function renderResultPanel(task: AgentTaskView): string {
  if (task.final_text) {
    return `
      <div class="agent-outcome detail-panel">
        <p class="t-eyebrow result-label detail-panel-label">${esc(t("task.finalAnswer"))}</p>
        <div class="result-pre result-md">${renderMarkdown(task.final_text.trim())}</div>
      </div>
    `;
  }
  return `
    <div class="agent-outcome detail-panel">
      <p class="t-eyebrow result-label detail-panel-label">${esc(t("task.errorLabel"))}</p>
      <pre class="result-pre result-error">${esc(task.error ?? "")}</pre>
    </div>
  `;
}

function renderEmptyDetail(): string {
  return `
    <div class="task-empty-detail">
      <p class="t-eyebrow result-label">${esc(t("task.selected"))}</p>
      <p class="t-small placeholder">${esc(t("task.emptyDetail"))}</p>
    </div>
  `;
}

// ── Embedded note cards ───────────────────────────────────────────────────
// The notes the agent saw this run, rendered from the local archive
// (`agent_task_notes`). Media is served from disk via the Tauri asset protocol
// (convertFileSrc), so images and video play locally without re-fetching.

function renderNotesSection(task: AgentTaskView): string {
  const notes = task.notes ?? [];
  if (notes.length === 0) return "";
  return `
    <section class="note-rail-wrap" aria-label="${esc(t("note.seen"))}">
      <p class="t-eyebrow result-label">${esc(t("note.seen"))} · ${notes.length}</p>
      <div class="note-rail">
        ${notes.map(renderNoteCard).join("")}
      </div>
    </section>
  `;
}

function renderNoteCard(note: NoteRecord): string {
  const title = (note.title ?? "").trim();
  const author = (note.author ?? "").trim();
  const content = (note.content ?? "").trim();
  const stats = noteStatsLine(note);
  const url = (note.url ?? "").trim();
  const tags = (note.hashtags ?? []).filter((tag) => tag.trim()).slice(0, 4);
  return `
    <article class="note-card">
      ${renderNoteMedia(note)}
      <div class="note-card-body">
        ${title ? `<p class="note-card-title">${esc(title)}</p>` : ""}
        ${author ? `<p class="note-card-author t-small subtle">${esc(author)}</p>` : ""}
        ${stats ? `<p class="note-card-stats t-small subtle">${esc(stats)}</p>` : ""}
        ${content ? `<p class="note-card-content t-small">${esc(content)}</p>` : ""}
        ${tags.length ? `<div class="note-card-tags">${tags.map((tag) => `<span class="note-card-tag">${esc(formatTag(tag))}</span>`).join("")}</div>` : ""}
        ${url ? `<button type="button" class="note-card-open" data-open-note-url="${esc(url)}">${esc(t("note.openOriginal"))}</button>` : ""}
      </div>
    </article>
  `;
}

function renderNoteMedia(note: NoteRecord): string {
  const media = note.media ?? [];
  if (note.type === "video") {
    const video = pickDownloadedAsset(media, "video");
    const poster = pickDownloadedAsset(media, "video_poster");
    const posterSrc = poster?.local_path ? convertFileSrc(poster.local_path) : "";
    if (video?.local_path) {
      return `
        <div class="note-card-media note-card-media-video">
          <video class="note-card-asset" controls preload="metadata"${posterSrc ? ` poster="${esc(posterSrc)}"` : ""} src="${esc(convertFileSrc(video.local_path))}"></video>
        </div>`;
    }
    // Video not downloadable (e.g. a blob: URL): show the poster if we have it,
    // flagged, rather than a broken player.
    if (posterSrc) {
      return `
        <div class="note-card-media note-card-media-video">
          <img class="note-card-asset" src="${esc(posterSrc)}" alt="" loading="lazy" />
          <span class="note-card-media-flag t-small">${esc(t("note.videoUnavailable"))}</span>
        </div>`;
    }
    return renderNoteMediaEmpty();
  }
  // Image note: first downloaded image as the cover; badge the carousel length.
  const images = media.filter(
    (asset) => asset.role === "image" && asset.download_status === "downloaded" && asset.local_path,
  );
  const cover = images[0];
  if (cover?.local_path) {
    const count = images.length > 1 ? `<span class="note-card-media-count t-small">1/${images.length}</span>` : "";
    return `
      <div class="note-card-media">
        <img class="note-card-asset" src="${esc(convertFileSrc(cover.local_path))}" alt="" loading="lazy" />
        ${count}
      </div>`;
  }
  return renderNoteMediaEmpty();
}

function renderNoteMediaEmpty(): string {
  return `<div class="note-card-media note-card-media-empty"><span class="t-small placeholder">${esc(t("note.noMedia"))}</span></div>`;
}

function pickDownloadedAsset(media: NoteMediaAsset[], role: string): NoteMediaAsset | undefined {
  return media.find(
    (asset) => asset.role === role && asset.download_status === "downloaded" && !!asset.local_path,
  );
}

function noteStatsLine(note: NoteRecord): string {
  const parts: string[] = [];
  const likes = (note.likes ?? "").trim();
  const favorites = (note.favorites ?? "").trim();
  const comments = (note.comments_count ?? "").trim();
  if (likes) parts.push(`${likes} ${t("note.likes")}`);
  if (favorites) parts.push(`${favorites} ${t("note.saves")}`);
  if (comments) parts.push(`${comments} ${t("note.comments")}`);
  return parts.join(" · ");
}

function formatTag(tag: string): string {
  const trimmed = tag.trim();
  return trimmed.startsWith("#") ? trimmed : `#${trimmed}`;
}

export function renderAgentEvent(ev: AgentTaskEventPayload): string {
  const glyph = eventGlyph(ev.kind);
  return `<div class="event event-${ev.kind}"><span class="event-glyph">${glyph}</span><span class="event-text">${esc(ev.text)}</span></div>`;
}

// Elapsed run time: started→finished, or started→now while still running.
// A terminal task without a finished_at has no meaningful end, so we show no
// duration rather than a figure that keeps ticking up against the wall clock.
function formatDuration(task: AgentTaskView): string | null {
  if (!task.started_at) return null;
  const running = task.status === "running" || task.status === "queued";
  const end = task.finished_at ?? (running ? Date.now() : null);
  if (end === null) return null;
  const seconds = Math.max(0, Math.round((end - task.started_at) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

function eventGlyph(kind: AgentTaskEventPayload["kind"]): string {
  switch (kind) {
    case "queued": return "○";
    case "running": return "●";
    case "started": return "▸";
    case "tab": return "□";
    case "turn": return "──";
    case "assistant": return " ";
    case "reasoning": return "·";
    case "tool_call": return "→";
    case "tool_result": return "←";
    case "tool_error": return "✗";
    case "api_error": return "✗";
    case "done": return "✓";
    case "completed": return "✓";
    case "failed": return "✗";
    case "cancelled": return "−";
    case "interrupted": return "!";
  }
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
