//! Embedded rich-note UI (SocaiV2 design, ported from the Claude Design handoff).
//!
//! A note is a Xiaohongshu post the agent saw/cited. Notes live in a per-run
//! registry (note_id -> NoteData); the timeline embeds a rich card per note it
//! surfaced, the answer cites notes with `note:<id>` links upgraded into pills,
//! and any reference opens one lightbox viewer. Media is served from disk via
//! the Tauri asset protocol (convertFileSrc) — images/video play locally.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { esc } from "../lib/html";
import { renderMarkdown } from "../lib/markdown";
import type { NoteData, NoteMedia } from "../main";

// ── per-run registry (the selected task's notes) ────────────────────
let REGISTRY: Record<string, NoteData> = {};
let RUN_DIR = "";

/** Point the note UI at the selected task's archive. Call before rendering. */
export function setNoteRegistry(notes: NoteData[] | undefined, runDir: string | null | undefined): void {
  REGISTRY = {};
  for (const note of notes ?? []) {
    if (note && typeof note.note_id === "string" && note.note_id) REGISTRY[note.note_id] = note;
  }
  RUN_DIR = runDir ?? "";
}
function resolveNote(ref: string): NoteData | null {
  return REGISTRY[ref] ?? null;
}

// ── helpers ─────────────────────────────────────────────────────────
function coverOf(note: NoteData): NoteMedia {
  return (note.media && note.media[0]) || { kind: "image", ratio: "3:4" };
}
function fmtStat(n: number | undefined | null): string {
  if (n == null) return "—";
  if (n >= 1000) {
    const k = n / 1000;
    return (k >= 100 ? Math.round(k) : Math.round(k * 10) / 10) + "k";
  }
  return String(n);
}
function fmtDate(ms: number | undefined): string {
  if (!ms) return "";
  try {
    return new Date(ms).toLocaleDateString("en-US", { month: "short", day: "numeric" });
  } catch {
    return "";
  }
}
function authorInitial(note: NoteData): string {
  const n = note.author && note.author.name;
  return n ? Array.from(n)[0] : "·";
}
// Resolve a note media path (absolute, or media_dir-relative to run_dir) to a
// webview-loadable asset URL. Empty when the file isn't available.
function assetUrl(note: NoteData, path: string | undefined): string {
  if (!path) return "";
  if (/^(asset|https?):/.test(path)) return path;
  if (path.startsWith("/")) return convertFileSrc(path);
  const base = `${RUN_DIR.replace(/\/$/, "")}/${(note.media_dir || "").replace(/\/$/, "")}`;
  return convertFileSrc(`${base.replace(/\/$/, "")}/${path}`);
}

// ── tiny line icons (currentColor, 24-box) ──────────────────────────
const svg = (inner: string, sw = 1.7, filled = false): string =>
  `<svg viewBox="0 0 24 24" fill="${filled ? "currentColor" : "none"}" stroke="${filled ? "none" : "currentColor"}" stroke-width="${sw}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${inner}</svg>`;
const IC = {
  heart: () => svg(`<path d="M12 20s-6.5-4.1-6.5-9A3.5 3.5 0 0 1 12 7a3.5 3.5 0 0 1 6.5 4c0 4.9-6.5 9-6.5 9z" />`),
  bookmark: () => svg(`<path d="M6.5 4.5h11v15l-5.5-3.6-5.5 3.6z" />`),
  comment: () => svg(`<path d="M5 5.5h14v9H9.5L5 18z" />`),
  share: () => svg(`<path d="M12 3.5v11" /><path d="M8 7l4-3.5L16 7" /><path d="M5 12.5V20h14v-7.5" />`),
  play: () => svg(`<path d="M8 5.2v13.6L19 12z" />`, 1.7, true),
  stack: () => svg(`<rect x="8" y="3.5" width="12.5" height="12.5" rx="1.6" /><path d="M15.5 19.5H4.5a1 1 0 0 1-1-1V7.5" />`, 1.8),
  external: () => svg(`<path d="M14 4.5h5.5V10" /><path d="M19.5 4.5 11 13" /><path d="M18 14.5v4a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1h4" />`),
  chevL: () => svg(`<path d="M14.5 6 9 12l5.5 6" />`, 1.9),
  chevR: () => svg(`<path d="M9.5 6 15 12l-5.5 6" />`, 1.9),
  close: () => svg(`<path d="M6 6l12 12M18 6 6 18" />`, 1.8),
};
function kindIcon(note: NoteData): string {
  const cover = coverOf(note);
  if (cover.kind === "video") return IC.play();
  if ((note.media || []).length > 1) return IC.stack();
  return "";
}

// ── media frame ─────────────────────────────────────────────────────
// Cards/thumbs/dots: universal 3:4 frame — images fill (cover), a 9:16 video
// is pillarboxed inside. `gallery` is full-bleed: the gallery frame adopts the
// media's own ratio (see frameRatio), so a real <video controls> spans the
// frame's full width and the native timeline scrubber has room to render.
type MediaVariant = "cover" | "thumb" | "dot" | "gallery";
function mediaFrame(note: NoteData, m: NoteMedia, variant: MediaVariant, count = 0): string {
  const decorative = variant === "thumb" || variant === "dot";
  if (m.kind === "video") {
    const posterUrl = assetUrl(note, m.poster);
    const posterImg = posterUrl ? `<img class="note-media__img" src="${esc(posterUrl)}" alt="" loading="lazy" />` : "";
    if (variant === "gallery") {
      const videoUrl = assetUrl(note, m.src);
      const inner = videoUrl
        ? `<video class="note-media__img" controls preload="metadata"${posterUrl ? ` poster="${esc(posterUrl)}"` : ""} src="${esc(videoUrl)}"></video>`
        : `${posterImg}<span class="note-media__play">${IC.play()}</span>`;
      return `<span class="note-media" data-kind="video">${inner}</span>`;
    }
    return `<span class="note-media note-media--pillarbox" data-kind="video"><span class="note-media__inner">${posterImg}${
      decorative ? "" : `<span class="note-media__play">${IC.play()}</span>`
    }${m.dur && !decorative ? `<span class="note-media__dur">${esc(m.dur)}</span>` : ""}</span></span>`;
  }
  const url = assetUrl(note, m.src);
  // gallery slides are pre-rendered hidden — lazy would defer them to first
  // reveal and flash; everything else stays lazy.
  const img = url
    ? `<img class="note-media__img" src="${esc(url)}" alt=""${variant === "gallery" ? "" : ` loading="lazy"`} />`
    : `<span class="note-media--placeholder" style="position:absolute;inset:0"></span>`;
  const badge = count > 1 && !decorative ? `<span class="note-media__count">${IC.stack()}${count}</span>` : "";
  return `<span class="note-media" data-kind="image">${img}${badge}</span>`;
}

function statsRow(note: NoteData): string {
  const s = note.stats || {};
  const items: Array<[string, string, number | undefined]> = [
    ["likes", IC.heart(), s.likes],
    ["collects", IC.bookmark(), s.collects],
    ["comments", IC.comment(), s.comments],
    ["shares", IC.share(), s.shares],
  ];
  return `<span class="note-stats">${items
    .map(([k, icon, v]) => `<span class="note-stat" title="${k}">${icon}${fmtStat(v)}</span>`)
    .join("")}</span>`;
}
function avatar(note: NoteData, cls = ""): string {
  return `<span class="note-author__avatar ${cls}" aria-hidden="true">${esc(authorInitial(note))}</span>`;
}

// ── rich card (the timeline embed unit + answer sources) ────────────
// The carousel corner belongs to the `1/2` idx chip alone — the ▣-count badge
// (same corner, same pill) is for static covers only; stacking both reads as
// a broken chip. Rich covers carousel, compact covers stay static (design).
function coverInner(note: NoteData, idx: number): string {
  const media = note.media && note.media.length ? note.media : [coverOf(note)];
  const i = Math.max(0, Math.min(idx, media.length - 1));
  const multi = media.length > 1;
  const nav = multi
    ? `<button type="button" class="note-card__nav note-card__nav--prev" data-note-nav="-1" aria-label="previous image">${IC.chevL()}</button>` +
      `<button type="button" class="note-card__nav note-card__nav--next" data-note-nav="1" aria-label="next image">${IC.chevR()}</button>` +
      `<span class="note-card__idx">${i + 1}/${media.length}</span>`
    : "";
  return mediaFrame(note, media[i], "cover") + nav;
}
function renderCard(note: NoteData, density: "rich" | "compact" = "rich"): string {
  const title = esc(note.title || "");
  const name = esc((note.author && note.author.name) || "");
  const cover =
    density === "rich"
      ? `<div class="note-card__cover" data-note-cover="${esc(note.note_id)}" data-idx="0">${coverInner(note, 0)}</div>`
      : `<div class="note-card__cover">${mediaFrame(note, coverOf(note), "cover", (note.media || []).length)}</div>`;
  return `
    <div class="note-card" data-density="${density}" data-note-open="${esc(note.note_id)}" role="button" tabindex="0" title="${title}">
      ${cover}
      <div class="note-card__body">
        <div class="note-card__title">${title}</div>
        ${statsRow(note)}
        <div class="note-card__meta">
          <span class="note-author">${avatar(note)}<span class="note-author__name">${name}</span></span>
          <span class="note-card__date">${esc(fmtDate(note.posted_at))}</span>
          ${
            note.url
              ? `<span class="note-card__link" data-note-external="${esc(note.url)}" title="open on xiaohongshu" role="button" tabindex="0" aria-label="open on xiaohongshu">${IC.external()}</span>`
              : ""
          }
        </div>
      </div>
    </div>`;
}

/** The notes a tool_result surfaced, as rich cards beneath its row. */
export function renderTimelineEmbed(refs: string[], density: "rich" | "compact" = "rich"): string {
  const notes = refs.map(resolveNote).filter((n): n is NoteData => !!n);
  if (notes.length === 0) return "";
  return `<div class="event-embed">${notes.map((n) => renderCard(n, density)).join("")}</div>`;
}

// ── answer citations (pills + hover preview) ────────────────────────
function pillHTML(id: string, label: string, note: NoteData): string {
  const cover = coverOf(note);
  return (
    `<span class="note-cite-wrap" data-note-cite="${esc(id)}">` +
    `<span class="note-cite" data-note-open="${esc(id)}" role="button" tabindex="0">` +
    `<span class="note-cite__dot">${mediaFrame(note, cover, "dot")}</span>${esc(label)}` +
    `<span class="note-cite__kind">${kindIcon(note)}</span></span></span>`
  );
}
function citePreviewHTML(note: NoteData): string {
  const cover = coverOf(note);
  const name = esc((note.author && note.author.name) || "");
  return (
    `<span class="note-cite-pop__inner">` +
    `<span class="note-cite-pop__media">${mediaFrame(note, cover, "dot")}</span>` +
    `<span class="note-cite-pop__body">` +
    `<span class="note-cite-pop__title">${esc(note.title || "")}</span>` +
    `<span class="note-cite-pop__author">${avatar(note)}${name}</span>${statsRow(note)}</span></span>`
  );
}

/** Render the markdown answer, upgrading `note:<id>` links into citation pills.
 *  The `note:` scheme is rewritten to a `#note:` fragment before markdown so it
 *  survives DOMPurify untouched (a raw custom scheme gets sanitized away). */
export function renderNoteAnswer(src: string): string {
  const rewritten = String(src || "")
    .trim()
    .replace(/\]\(note:([^)\s]+)\)/g, "](#note:$1)");
  const html = renderMarkdown(rewritten);
  const doc = new DOMParser().parseFromString(`<div id="r">${html}</div>`, "text/html");
  const root = doc.getElementById("r");
  if (!root) return html;
  root.querySelectorAll('a[href^="#note:"], a[href^="note:"]').forEach((a) => {
    const id = (a.getAttribute("href") || "").replace(/^#?note:/, "");
    const label = a.textContent || id;
    const note = resolveNote(id);
    const holder = doc.createElement("span");
    holder.innerHTML = note ? pillHTML(id, label, note) : esc(label);
    a.replaceWith(...Array.from(holder.childNodes));
  });
  return root.innerHTML;
}

// ── viewer (lightbox) ───────────────────────────────────────────────
// All slides render up front; navigation only toggles `hidden`. Keeping the
// DOM alive preserves decoded images and video state, so switching never
// flashes the way an innerHTML re-render does.
// Each frame hugs its own media's ratio (core stamps "9:16" on videos, "3:4"
// on images) rather than the cards' universal 3:4 — no gray pillarbox bars,
// and a video's native controls get the full frame width.
function frameRatio(m: NoteMedia): string {
  const parsed = /^(\d+)\s*:\s*(\d+)$/.exec(m.ratio || "");
  if (parsed) return `${parsed[1]} / ${parsed[2]}`;
  return m.kind === "video" ? "9 / 16" : "3 / 4";
}
function galleryStage(note: NoteData, idx: number): string {
  const media = note.media && note.media.length ? note.media : [coverOf(note)];
  const i = Math.max(0, Math.min(idx, media.length - 1));
  const multi = media.length > 1;
  const frames = media
    .map((m, j) => `<div class="note-gallery__frame" style="aspect-ratio: ${frameRatio(m)}"${j === i ? "" : " hidden"}>${mediaFrame(note, m, "gallery")}</div>`)
    .join("");
  const nav = multi
    ? `<button type="button" class="note-gallery__nav note-gallery__nav--prev" data-gallery-nav="-1"${i === 0 ? " disabled" : ""} aria-label="previous">${IC.chevL()}</button>` +
      `<button type="button" class="note-gallery__nav note-gallery__nav--next" data-gallery-nav="1"${i === media.length - 1 ? " disabled" : ""} aria-label="next">${IC.chevR()}</button>`
    : "";
  return frames + nav;
}
function galleryThumbs(note: NoteData, idx: number): string {
  const media = note.media || [];
  if (media.length <= 1) return "";
  return `<div class="note-gallery__thumbs">${media
    .map(
      (m, i) =>
        `<button type="button" class="note-gallery__thumb${i === idx ? " is-active" : ""}" data-gallery-thumb="${i}" aria-label="media ${i + 1}">${mediaFrame(note, m, "thumb")}</button>`,
    )
    .join("")}</div>`;
}
function metaPanel(note: NoteData): string {
  const s = note.stats || {};
  const stats: Array<[string, string, number | undefined]> = [
    ["likes", IC.heart(), s.likes],
    ["collects", IC.bookmark(), s.collects],
    ["comments", IC.comment(), s.comments],
    ["shares", IC.share(), s.shares],
  ];
  const statGrid = stats
    .map(
      ([k, icon, v]) =>
        `<div class="note-meta__stat"><span class="note-meta__stat-k">${icon}${k}</span><span class="note-meta__stat-v">${fmtStat(v)}</span></div>`,
    )
    .join("");
  return `
    <div class="note-meta">
      <h3 class="note-meta__title">${esc(note.title || "")}</h3>
      ${note.excerpt ? `<p class="note-meta__excerpt">${esc(note.excerpt)}</p>` : ""}
      <div class="note-meta__statgrid">${statGrid}</div>
      <div class="note-meta__rows">
        <div class="note-meta__row"><span class="note-meta__row-k">posted</span><span class="note-meta__row-v">${esc(fmtDate(note.posted_at))}</span></div>
        <div class="note-meta__row"><span class="note-meta__row-k">note_id</span><span class="note-meta__row-v" title="${esc(note.note_id)}">${esc(note.note_id)}</span></div>
      </div>
      ${
        note.url
          ? `<span class="btn-ghost btn-compact note-meta__link" data-note-external="${esc(note.url)}" role="button" tabindex="0">${IC.external()}open on xiaohongshu</span>`
          : ""
      }
      <span class="note-meta__saved"><i></i>${(note.media || []).length} media · saved locally</span>
    </div>`;
}
function viewerHTML(note: NoteData): string {
  const name = esc((note.author && note.author.name) || "");
  const handle = esc((note.author && note.author.handle) || note.note_id);
  return `
    <div class="note-viewer-backdrop" data-note-close>
      <div class="note-viewer-panel" role="dialog" aria-label="${esc(note.title || "note")}" data-note-stop>
        <div class="note-viewer-head">
          <span class="note-viewer-head__author">
            ${avatar(note, "note-viewer-head__avatar")}
            <span class="note-viewer-head__meta">
              <span class="note-viewer-head__name">${name}</span>
              <span class="note-viewer-head__handle">${handle}</span>
            </span>
          </span>
          <button type="button" class="note-viewer-close" data-note-close aria-label="close">${IC.close()}</button>
        </div>
        <div class="note-viewer-body">
          <div class="note-gallery" data-gallery data-idx="0" data-note="${esc(note.note_id)}">
            <div class="note-gallery__stage">${galleryStage(note, 0)}</div>
            ${galleryThumbs(note, 0)}
          </div>
          ${metaPanel(note)}
        </div>
      </div>
    </div>`;
}
function galleryGo(gallery: HTMLElement, idx: number): void {
  const note = resolveNote(gallery.getAttribute("data-note") || "");
  if (!note) return;
  const n = (note.media || []).length || 1;
  const next = Math.max(0, Math.min(n - 1, idx));
  gallery.setAttribute("data-idx", String(next));
  gallery.querySelectorAll<HTMLElement>(".note-gallery__frame").forEach((frame, i) => {
    frame.hidden = i !== next;
    if (i !== next) frame.querySelector("video")?.pause();
  });
  gallery.querySelectorAll<HTMLButtonElement>("[data-gallery-nav]").forEach((btn) => {
    const dir = parseInt(btn.getAttribute("data-gallery-nav") || "0", 10);
    btn.disabled = dir < 0 ? next === 0 : next === n - 1;
  });
  gallery.querySelectorAll(".note-gallery__thumb").forEach((th, i) =>
    th.classList.toggle("is-active", i === next),
  );
}
function closeViewer(): void {
  document.querySelector(".note-viewer-backdrop")?.parentElement?.remove();
}
function openViewer(id: string): void {
  const note = resolveNote(id);
  if (!note) return;
  closeViewer();
  const host = document.createElement("div");
  host.className = "note-viewer note-viewer--lightbox";
  host.innerHTML = viewerHTML(note);
  document.body.appendChild(host);
}

// ── interactions (one delegated listener set, attached once) ────────
let bound = false;
/** Wire note interactions globally (idempotent): open viewer, card carousel,
 *  gallery nav/thumbs, citation hover preview, external links. */
export function bindNoteInteractions(): void {
  if (bound) return;
  bound = true;

  document.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;

    // external link (Tauri webview won't honour target=_blank)
    const ext = target.closest<HTMLElement>("[data-note-external]");
    if (ext) {
      e.preventDefault();
      e.stopPropagation();
      const url = ext.getAttribute("data-note-external");
      if (url) invoke("open_external", { url }).catch((err) => console.error("open_external failed:", err));
      return;
    }
    // card carousel nav — must not open the viewer
    const nav = target.closest<HTMLElement>("[data-note-nav]");
    if (nav) {
      e.preventDefault();
      e.stopPropagation();
      const cover = nav.closest<HTMLElement>("[data-note-cover]");
      const note = cover && resolveNote(cover.getAttribute("data-note-cover") || "");
      if (cover && note) {
        const n = (note.media || []).length || 1;
        const cur = parseInt(cover.getAttribute("data-idx") || "0", 10) || 0;
        const nextIdx = (cur + parseInt(nav.getAttribute("data-note-nav") || "0", 10) + n) % n;
        cover.setAttribute("data-idx", String(nextIdx));
        cover.innerHTML = coverInner(note, nextIdx);
      }
      return;
    }
    // viewer gallery nav
    const gnav = target.closest<HTMLElement>("[data-gallery-nav]");
    if (gnav) {
      const gallery = gnav.closest<HTMLElement>("[data-gallery]");
      if (gallery) galleryGo(gallery, (parseInt(gallery.getAttribute("data-idx") || "0", 10) || 0) + parseInt(gnav.getAttribute("data-gallery-nav") || "0", 10));
      return;
    }
    const thumb = target.closest<HTMLElement>("[data-gallery-thumb]");
    if (thumb) {
      const gallery = thumb.closest<HTMLElement>("[data-gallery]");
      if (gallery) galleryGo(gallery, parseInt(thumb.getAttribute("data-gallery-thumb") || "0", 10));
      return;
    }
    // close: the X button, or a click on the backdrop itself (outside the panel)
    if (target.closest(".note-viewer-close")) {
      closeViewer();
      return;
    }
    if (target.closest("[data-note-close]") && !target.closest("[data-note-stop]")) {
      closeViewer();
      return;
    }
    // open the viewer from any reference
    const open = target.closest<HTMLElement>("[data-note-open]");
    if (open) {
      e.preventDefault();
      openViewer(open.getAttribute("data-note-open") || "");
    }
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeViewer();
      return;
    }
    // Activate role="button" note controls from the keyboard: Enter/Space
    // synthesize a click so the delegated click handler above does the rest.
    if (e.key !== "Enter" && e.key !== " ") return;
    const target = e.target as HTMLElement;
    if (target.closest("input, textarea, select, [contenteditable]")) return;
    const actionable = target.closest<HTMLElement>(
      "[data-note-open], [data-note-external], [data-note-nav], [data-gallery-nav], [data-gallery-thumb], [data-note-close], .note-viewer-close",
    );
    if (!actionable) return;
    e.preventDefault();
    actionable.click();
  });

  // citation hover preview (position:fixed popover so it escapes overflow)
  document.addEventListener(
    "mouseover",
    (e) => {
      const wrap = (e.target as HTMLElement).closest<HTMLElement>(".note-cite-wrap[data-note-cite]");
      if (!wrap || wrap.querySelector(".note-cite-pop")) return;
      const note = resolveNote(wrap.getAttribute("data-note-cite") || "");
      if (!note) return;
      const r = wrap.getBoundingClientRect();
      const W = 250,
        H = 128,
        gap = 8;
      const above = r.top > H + gap;
      const left = Math.max(W / 2 + 8, Math.min(window.innerWidth - W / 2 - 8, r.left + r.width / 2));
      const top = above ? r.top - gap : r.bottom + gap;
      const pop = document.createElement("span");
      pop.className = "note-cite-pop";
      pop.style.left = `${left}px`;
      pop.style.top = `${top}px`;
      pop.style.transform = above ? "translate(-50%, -100%)" : "translate(-50%, 0)";
      pop.innerHTML = citePreviewHTML(note);
      wrap.appendChild(pop);
    },
    true,
  );
  document.addEventListener(
    "mouseout",
    (e) => {
      const wrap = (e.target as HTMLElement).closest<HTMLElement>(".note-cite-wrap");
      if (!wrap) return;
      // mouseout bubbles for moves between the pill's children (including onto
      // the popover itself) — only remove once the pointer truly leaves the wrap.
      const to = e.relatedTarget as Node | null;
      if (to && wrap.contains(to)) return;
      wrap.querySelector(".note-cite-pop")?.remove();
    },
    true,
  );
}
