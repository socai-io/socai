//! Embedded rich-note UI (SocaiV2 design, ported from the Claude Design handoff).
//!
//! A note is a Xiaohongshu post the agent saw/cited. Notes live in a per-run
//! registry (note_id -> NoteData); the conversation embeds a rich card per
//! note each search surfaced, the answer cites notes with `note:<id>` links
//! upgraded into pills, and any reference opens one lightbox viewer. Media is served from disk via
//! the Tauri asset protocol (convertFileSrc) — images/video play locally.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { esc } from "../lib/html";
import { getLocale, t } from "../lib/i18n";
import { renderMarkdown } from "../lib/markdown";
import type { NoteComment, NoteData, NoteMedia } from "../main";

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
    const date = new Date(ms);
    const sameYear = date.getFullYear() === new Date().getFullYear();
    return date.toLocaleDateString(getLocale(), {
      month: "short",
      day: "numeric",
      ...(sameYear ? {} : { year: "numeric" }),
    });
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
  // Absolute: unix, or a Windows drive path (the backend absolutizes media
  // paths when aggregating notes across a conversation's run dirs).
  if (path.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(path)) return convertFileSrc(path);
  const base = `${RUN_DIR.replace(/\/$/, "")}/${(note.media_dir || "").replace(/\/$/, "")}`;
  return convertFileSrc(`${base.replace(/\/$/, "")}/${path}`);
}

// ── tiny line icons (currentColor, 24-box) ──────────────────────────
const svg = (inner: string, sw = 1.7, filled = false): string =>
  `<svg viewBox="0 0 24 24" fill="${filled ? "currentColor" : "none"}" stroke="${filled ? "none" : "currentColor"}" stroke-width="${sw}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${inner}</svg>`;
const IC = {
  heart: () => svg(`<path d="M12 20s-6.5-4.1-6.5-9A3.5 3.5 0 0 1 12 7a3.5 3.5 0 0 1 6.5 4c0 4.9-6.5 9-6.5 9z" />`),
  // XHS uses a five-pointed star for collects and a round speech bubble for
  // comments — mirror both so the counts read instantly to XHS users.
  star: () => svg(`<path d="M12 2.76 14.84 8.52 21.2 9.45 16.6 13.93 17.69 20.26 12 17.27 6.31 20.26 7.4 13.93 2.8 9.45 9.16 8.52z" />`),
  comment: () =>
    svg(`<path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z" />`),
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
    const loading = !m.src && m.status === "loading";
    const loadingIndicator = loading
      ? `<span class="note-media__loading" aria-hidden="true"></span>`
      : "";
    const failedIndicator = !m.src && m.status === "failed"
      ? `<span class="note-media__failed" aria-hidden="true">!</span>`
      : "";
    const statusIndicator = loadingIndicator || failedIndicator;
    if (variant === "gallery") {
      const videoUrl = assetUrl(note, m.src);
      const inner = videoUrl
        ? `<video class="note-media__img" controls preload="metadata"${posterUrl ? ` poster="${esc(posterUrl)}"` : ""} src="${esc(videoUrl)}"></video>`
        : `${posterImg}${statusIndicator}`;
      return `<span class="note-media" data-kind="video">${inner}</span>`;
    }
    // With the file on disk the glyph is a real button and marks the whole
    // frame as a play surface (the click handler treats any click on the
    // frame as play; the button itself is the keyboard-focusable control).
    // Without a file the glyph stays decorative and the click bubbles to the
    // card (viewer opens).
    const videoUrl = decorative ? "" : assetUrl(note, m.src);
    const play = decorative
      ? ""
      : videoUrl
        ? `<button type="button" class="note-media__play" data-note-play="${esc(videoUrl)}" aria-label="play video">${IC.play()}</button>`
        : statusIndicator;
    return `<span class="note-media note-media--pillarbox" data-kind="video"><span class="note-media__inner">${posterImg}${play}${m.dur && !decorative ? `<span class="note-media__dur">${esc(m.dur)}</span>` : ""}</span></span>`;
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

function fmtClock(s: number): string {
  if (!isFinite(s) || s < 0) return "0:00";
  const total = Math.floor(s);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

// Start inline playback in a card cover: overlay a <video> on the poster
// (the play button's data-note-play carries the resolved asset url). The
// poster stays in-flow underneath — it is what sizes the pillarbox, so
// removing it would reflow the card mid-click — and it backs the letterbox
// bars contain-fit leaves around whatever ratio the file really is.
// The controls are ours, not the native ones: at card width WebKit's adaptive
// controls drop the scrubber and time readout, so the card gets a slim
// track + clock bar, click-on-video toggles play/pause, and a centered glyph
// marks the paused state. The viewer gallery keeps native controls.
function playInline(btn: HTMLElement): void {
  const frame = btn.closest<HTMLElement>(".note-media__inner") ?? btn.closest<HTMLElement>(".note-media");
  const src = btn.getAttribute("data-note-play");
  if (!frame || !src) return;
  // video + glyph live in the pillarbox inner; the bar spans the whole cover
  const barHost = btn.closest<HTMLElement>(".note-media") ?? frame;
  const poster = frame.querySelector<HTMLImageElement>("img.note-media__img")?.getAttribute("src") || "";
  const durText = frame.querySelector(".note-media__dur")?.textContent || "0:00";

  const video = document.createElement("video");
  video.className = "note-media__video";
  video.playsInline = true;
  // no native controls, so the element itself is the keyboard control:
  // focusable, Enter/Space toggling (keeps keyboard users in the loop after
  // the play button — their focus anchor — is removed below)
  video.tabIndex = 0;
  video.setAttribute("aria-label", "video player");
  if (poster) video.poster = poster;
  video.src = src;

  // paused indicator — same glyph as the poster state, but inert (clicks fall
  // through to the video's toggle); visible until playback actually starts,
  // so a refused play() keeps a play affordance on screen.
  const glyph = document.createElement("span");
  glyph.className = "note-media__play note-media__play--hud";
  glyph.innerHTML = IC.play();

  const bar = document.createElement("span");
  bar.className = "note-media__vbar";
  bar.innerHTML =
    `<span class="note-media__vtrack"><span class="note-media__vfill"></span></span>` +
    `<span class="note-media__vtime">0:00 / ${esc(durText)}</span>`;
  const track = bar.querySelector<HTMLElement>(".note-media__vtrack")!;
  const fill = bar.querySelector<HTMLElement>(".note-media__vfill")!;
  const clock = bar.querySelector<HTMLElement>(".note-media__vtime")!;

  const sync = () => {
    const d = video.duration;
    // clamp: currentTime can drift a hair past duration at end-of-media
    if (isFinite(d) && d > 0) fill.style.width = `${Math.min(100, Math.max(0, (video.currentTime / d) * 100))}%`;
    clock.textContent = `${fmtClock(video.currentTime)} / ${isFinite(d) && d > 0 ? fmtClock(d) : durText}`;
  };
  video.addEventListener("timeupdate", sync);
  video.addEventListener("loadedmetadata", sync);
  video.addEventListener("play", () => (glyph.hidden = true));
  video.addEventListener("pause", () => (glyph.hidden = false));
  const toggle = () => {
    if (video.paused) video.play().catch(() => {});
    else video.pause();
  };
  const seekTo = (t: number) => {
    const d = video.duration;
    if (!isFinite(d) || d <= 0) return;
    video.currentTime = Math.max(0, Math.min(d, t));
    sync(); // timeupdate is throttled; reflect the seek immediately
  };
  video.addEventListener("click", toggle);
  // the focused video is the whole keyboard surface, like a native player:
  // Enter/Space toggle, arrows seek ±5s, Home/End jump (the track itself
  // stays pointer-only — no second tab stop per card)
  video.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault(); // Space must toggle, not scroll the timeline
      toggle();
    } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      seekTo(video.currentTime + (e.key === "ArrowRight" ? 5 : -5));
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      seekTo(e.key === "Home" ? 0 : video.duration);
    }
  });
  // bar clicks are seeks, never card opens (the delegated open handler sits
  // on document, so stopping propagation here is enough)
  bar.addEventListener("click", (e) => e.stopPropagation());
  const scrub = (e: PointerEvent) => {
    const r = track.getBoundingClientRect();
    if (r.width <= 0) return;
    seekTo(((e.clientX - r.left) / r.width) * video.duration);
  };
  track.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    scrub(e);
    // capture only extends the drag; a failed capture must not void the seek
    try {
      track.setPointerCapture(e.pointerId);
    } catch {
      /* inactive pointer (lifted mid-gesture) — click-seek already landed */
    }
  });
  track.addEventListener("pointermove", (e) => {
    if (track.hasPointerCapture(e.pointerId)) scrub(e);
  });

  // removing the focused button would strand keyboard focus on <body>;
  // hand it to the video so Enter/Space keep controlling playback
  const hadFocus = document.activeElement === btn;
  btn.remove();
  frame.querySelector(".note-media__dur")?.remove(); // the bar shows the clock
  frame.append(video, glyph);
  barHost.appendChild(bar);
  if (hadFocus) video.focus();
  // Called inside the click gesture, so WKWebView allows playback with sound;
  // if it still refuses, the poster, glyph and toggle remain usable.
  video.play().catch(() => {});
}

// No shares entry: XHS exposes no share count anywhere we read, so the column
// would always render the "—" placeholder.
function statsRow(note: NoteData): string {
  const s = note.stats || {};
  const items: Array<[string, string, number | undefined]> = [
    ["likes", IC.heart(), s.likes],
    ["collects", IC.star(), s.collects],
    ["comments", IC.comment(), s.comments],
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

/** Rich cards for the given refs, unwrapped — the conversation's search-note
 *  groups supply their own scrolling row container. Empty when none resolve. */
export function renderNoteCards(refs: string[], density: "rich" | "compact" = "rich"): string {
  const notes = refs.map(resolveNote).filter((n): n is NoteData => !!n);
  return notes.map((n) => renderCard(n, density)).join("");
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
// The right panel mirrors the XHS note modal: author row on top (clickable
// into the profile when the archive captured author.url), the full body
// (never the truncated excerpt when content is available), a quiet date + IP
// line, the transcript, then the captured top comments — and a slim
// icon-count engage bar pinned at the bottom.
function commentHTML(comment: NoteComment, isReply = false): string {
  const author = esc(comment.author || "·");
  const metaBits = [comment.time, comment.likes ? `${fmtStat(comment.likes)} ♥` : ""].filter(Boolean);
  const replies = (comment.replies || []).map((reply) => commentHTML(reply, true)).join("");
  return `
    <div class="note-comment${isReply ? " note-comment--reply" : ""}">
      <span class="note-author__avatar note-comment__avatar" aria-hidden="true">${esc(Array.from(comment.author || "·")[0])}</span>
      <div class="note-comment__body">
        <span class="note-comment__author">${author}${comment.is_author ? `<span class="note-comment__badge">${esc(t("note.authorBadge"))}</span>` : ""}</span>
        <p class="note-comment__text">${esc(comment.text)}</p>
        ${metaBits.length ? `<span class="note-comment__meta">${esc(metaBits.join(" · "))}</span>` : ""}
        ${replies ? `<div class="note-comment__replies">${replies}</div>` : ""}
      </div>
    </div>`;
}
function viewPanel(note: NoteData): string {
  const name = esc((note.author && note.author.name) || "");
  const authorUrl = (note.author && note.author.url) || "";
  const authorAttrs = authorUrl
    ? ` data-note-external="${esc(authorUrl)}" role="button" tabindex="0" title="${esc(authorUrl)}"`
    : "";
  const body = (typeof note.content === "string" && note.content.trim()) || note.excerpt || "";
  const transcript = typeof note.transcript === "string" ? note.transcript.trim() : "";
  const dateBits = [fmtDate(note.posted_at), note.ip_location || ""].filter(Boolean);
  const comments = note.comments || [];
  const commentTotal = note.stats?.comments ?? (comments.length || undefined);
  return `
    <div class="note-view">
      ${
        name
          ? `<div class="note-view__author${authorUrl ? " is-link" : ""}"${authorAttrs}>
              ${avatar(note, "note-view__avatar")}
              <span class="note-view__author-name">${name}</span>
            </div>`
          : ""
      }
      <div class="note-view__scroll">
        ${note.title ? `<h3 class="note-view__title">${esc(note.title)}</h3>` : ""}
        ${body ? `<p class="note-view__content">${esc(body)}</p>` : ""}
        ${dateBits.length ? `<p class="note-view__date">${esc(dateBits.join(" "))}</p>` : ""}
        ${
          transcript
            ? `<div class="note-view__transcript"><span class="note-view__transcript-label">${esc(t("note.transcript"))}</span><p class="note-view__transcript-text">${esc(transcript)}</p></div>`
            : ""
        }
        ${
          comments.length
            ? `<div class="note-view__comments">
                <span class="note-view__comments-head">${esc(t("note.commentsHead", { n: fmtStat(commentTotal) }))}</span>
                ${comments.map((comment) => commentHTML(comment)).join("")}
              </div>`
            : ""
        }
      </div>
      <div class="note-view__engage">
        ${statsRow(note)}
      </div>
    </div>`;
}
// No X: clicking outside the panel closes (and Esc still works), matching the
// XHS modal. The top-right corner instead opens the note on xiaohongshu —
// only a note with no archived url keeps a close glyph so the corner isn't dead.
function viewerHTML(note: NoteData): string {
  const corner = note.url
    ? `<button type="button" class="note-viewer-corner" data-note-external="${esc(note.url)}" title="${esc(t("note.openExternal"))}" aria-label="${esc(t("note.openExternal"))}">${IC.external()}</button>`
    : `<button type="button" class="note-viewer-corner" data-note-close aria-label="close">${IC.close()}</button>`;
  return `
    <div class="note-viewer-backdrop" data-note-close>
      <div class="note-viewer-panel" role="dialog" aria-label="${esc(note.title || "note")}" data-note-stop>
        ${corner}
        <div class="note-viewer-body">
          <div class="note-gallery" data-gallery data-idx="0" data-note="${esc(note.note_id)}">
            <div class="note-gallery__stage">${galleryStage(note, 0)}</div>
            ${galleryThumbs(note, 0)}
          </div>
          ${viewPanel(note)}
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
  // An inline card video would keep its audio running under the backdrop —
  // the lightbox owns playback while open. (Runs before the viewer is
  // appended, so only card/popover videos match.)
  document.querySelectorAll<HTMLVideoElement>(".note-media video").forEach((v) => v.pause());
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

    // A click inside a <video> is a native-controls interaction (play/pause,
    // scrub, volume — WebKit retargets shadow-DOM control clicks to the video
    // element) — never a note action like opening the viewer.
    if (target.closest("video")) return;
    // inline play — the whole cover is the play surface: any click on a
    // playable video frame starts it, and during playback the pillarbox
    // margins toggle pause. Only the card body below opens the note viewer.
    // Frames with no play button (images, missing file, gallery, thumbs/dots)
    // fall through to their existing handling.
    const media = target.closest<HTMLElement>(".note-media");
    if (media) {
      const play = media.querySelector<HTMLElement>("[data-note-play]");
      if (play) {
        e.preventDefault();
        e.stopPropagation();
        playInline(play);
        return;
      }
      const vid = media.querySelector<HTMLVideoElement>("video.note-media__video");
      if (vid) {
        e.preventDefault();
        e.stopPropagation();
        if (vid.paused) vid.play().catch(() => {});
        else vid.pause();
        return;
      }
    }
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
    // close: the corner glyph (inside the data-note-stop panel, so it needs
    // its own branch), or a click on the backdrop itself (outside the panel)
    if (target.closest(".note-viewer-corner[data-note-close]")) {
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
    if (target.closest("input, textarea, select, [contenteditable], video")) return;
    const actionable = target.closest<HTMLElement>(
      "[data-note-open], [data-note-external], [data-note-nav], [data-note-play], [data-gallery-nav], [data-gallery-thumb], [data-note-close], .note-viewer-corner",
    );
    if (!actionable) return;
    e.preventDefault();
    actionable.click();
  });

  // One playing note video at a time — starting any (card-inline or viewer
  // gallery) pauses the rest. Media events don't bubble, so capture.
  document.addEventListener(
    "play",
    (e) => {
      const el = e.target;
      if (!(el instanceof HTMLVideoElement) || !el.closest(".note-media")) return;
      document.querySelectorAll<HTMLVideoElement>(".note-media video").forEach((v) => {
        if (v !== el) v.pause();
      });
    },
    true,
  );

  // citation hover preview — the same rich card the search strips render
  // (position:fixed popover so it escapes overflow)
  document.addEventListener(
    "mouseover",
    (e) => {
      const wrap = (e.target as HTMLElement).closest<HTMLElement>(".note-cite-wrap[data-note-cite]");
      if (!wrap || wrap.querySelector(".note-cite-pop")) return;
      const note = resolveNote(wrap.getAttribute("data-note-cite") || "");
      if (!note) return;
      const r = wrap.getBoundingClientRect();
      const W = 208,
        H = 372,
        gap = 8;
      const above = r.top > H + gap;
      const left = Math.max(W / 2 + 8, Math.min(window.innerWidth - W / 2 - 8, r.left + r.width / 2));
      const top = above ? r.top - gap : r.bottom + gap;
      const pop = document.createElement("span");
      pop.className = "note-cite-pop";
      pop.style.left = `${left}px`;
      pop.style.top = `${top}px`;
      pop.style.transform = above ? "translate(-50%, -100%)" : "translate(-50%, 0)";
      pop.innerHTML = renderCard(note, "rich");
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
