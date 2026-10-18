import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import DOMPurify from "dompurify";
import { esc } from "../lib/html";
import { getLanguage, getLocale } from "../lib/i18n";
import type { NoteData } from "../main";

interface GithubData {
  kind?: string;
  fetched_at?: string;
  metadata?: Record<string, unknown>;
  readme_available?: boolean;
  readme_html?: string;
  readme_error?: string;
  avatar_source?: string;
  avatar_state?: string;
  page_data?: { items?: Array<{ full_name?: string; login?: string; description?: string; url?: string }> };
}
type Preview = NoteData & { github?: GithubData };
type Entry = { note?: Preview; promise?: Promise<void>; error?: string; timer?: ReturnType<typeof setTimeout> };
const cache = new Map<string, Entry>();
let notify: (key: string) => void = () => {};
export function onGithubPreviewUpdate(callback: (key: string) => void): void { notify = callback; }
const tr = (zh: string, en: string): string => getLanguage() === "zh" ? zh : en;
const reserved = new Set(["trending", "topics", "search", "collections", "explore", "marketplace", "sponsors", "orgs", "organizations", "settings", "login", "signup", "features", "about", "pricing", "readme", "notifications", "new", "codespaces", "enterprise", "security", "site", "apps", "events"]);

function keyFor(raw: string): string | null {
  try {
    const u = new URL(raw);
    if (u.protocol !== "https:" || u.username || u.password || (u.port && u.port !== "443")) return null;
    let p = u.pathname.split("/").filter(Boolean);
    if (u.hostname === "api.github.com" && p[0] === "repos") p = p.slice(1);
    else if (u.hostname === "api.github.com" && ["users", "orgs"].includes(p[0]) && p[1]) p = [p[1]];
    else if (!["github.com", "raw.githubusercontent.com"].includes(u.hostname)) return null;
    if (p.length >= 2 && !reserved.has(p[0]) && p.slice(0, 2).every(v => /^[\w.-]+$/.test(v))) return `https://github.com/${p[0]}/${p[1].replace(/\.git$/, "")}`.toLowerCase();
    if (p.length === 1 && !reserved.has(p[0])) return `https://github.com/${p[0]}`.toLowerCase();
    u.hash = ""; return u.href;
  } catch { return null; }
}

export function githubNote(raw: string, original?: NoteData): Preview | null {
  const key = keyFor(raw);
  if (!key) return null;
  const entry = cache.get(key);
  const title = key.replace("https://github.com/", "") || "GitHub";
  const note: Preview = { site: "github", title, media: [], ...original, ...entry?.note,
    note_id: original?.note_id || `github-link:${encodeURIComponent(raw)}`, url: raw, source_url: key };
  if (!entry) queueMicrotask(() => { void load(raw); });
  return note;
}

function pending(note?: Preview): boolean {
  return !!note && (!!note.media?.some(i => i.download_state === "loading")
    || (!!note.github?.avatar_source && !note.github.avatar_state));
}
async function load(url: string, refresh = false): Promise<void> {
  const key = keyFor(url);
  if (!key) return;
  let entry = cache.get(key);
  if (!entry) { entry = {}; cache.set(key, entry); }
  if (entry.promise) return entry.promise;
  if (entry.timer) clearTimeout(entry.timer);
  const current = entry;
  current.error = undefined;
  current.promise = (async () => {
    try { current.note = await invoke<Preview>("github_preview", { url: key, refresh }); }
    catch (error) { current.error = String(error); }
    finally {
      current.promise = undefined;
      notify(key);
      if (!current.error && pending(current.note)) current.timer = setTimeout(() => { void load(url); }, 2000);
    }
  })();
  return current.promise;
}

function count(value: unknown): string {
  return typeof value === "number" ? value.toLocaleString(getLocale()) : "";
}
function date(value: unknown): string {
  if (typeof value !== "string") return "";
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleDateString(getLocale(), { year: "numeric", month: "short", day: "numeric" });
}
function localImage(path?: string): string {
  return path && (path.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(path)) ? convertFileSrc(path) : "";
}
export function githubMetadata(note: Preview, detailed = false): string {
  const m = note.github?.metadata || {};
  const values: string[] = [];
  if (typeof m.stars === "number") values.push(`☆ ${count(m.stars)} stars`);
  if (typeof m.language === "string" && m.language) values.push(m.language);
  if (typeof m.forks === "number" && detailed) values.push(`${count(m.forks)} forks`);
  if (date(m.created_at)) values.push(`${tr("创建于", "Created")} ${date(m.created_at)}`);
  if (detailed && date(m.pushed_at)) values.push(`${tr("最近推送", "Last push")} ${date(m.pushed_at)}`);
  if (m.archived === true) values.push(tr("已归档", "Archived"));
  if (typeof m.followers === "number") values.push(`${count(m.followers)} ${tr("关注者", "followers")}`);
  if (typeof m.public_repos === "number") values.push(`${count(m.public_repos)} ${tr("公开仓库", "public repos")}`);
  return `<div class="github-metadata t-small">${values.map(v => `<span>${esc(v)}</span>`).join("")}</div>`;
}

export function githubCard(input: NoteData): string {
  const note = githubNote(input.url || "", input) || input as Preview;
  const entry = cache.get(keyFor(note.url || "") || "");
  const cover = note.media?.find(i => !/badge|shields\.io/i.test(i.source_url || ""));
  const src = localImage(cover?.src);
  const state = entry?.error ? tr("预览暂不可用", "Preview unavailable")
    : !entry?.note ? tr("正在读取仓库信息…", "Loading GitHub preview…") : "";
  return `<div class="note-article-card github-card" data-github-card="${esc(note.note_id)}" data-note-open="${esc(note.note_id)}" role="button" tabindex="0">
    <div class="note-article-card__author"><span>GitHub</span><span>${esc(note.author?.name || "")}</span></div>
    <h3 class="t-body">${esc(note.title || "GitHub")}</h3>
    ${githubMetadata(note)}
    ${src ? `<img class="github-card__image" src="${esc(src)}" alt="${esc(cover?.alt || "")}" loading="lazy" />` : ""}
    <p class="t-small">${esc(note.excerpt || note.content?.slice(0, 220) || "")}</p>
    ${state ? `<span class="t-small subtle">${esc(state)}</span>` : ""}</div>`;
}

function readme(note: Preview): string {
  const clean = DOMPurify.sanitize(note.github?.readme_html || "", {
    ALLOWED_TAGS: ["a", "p", "br", "hr", "div", "span", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "pre", "code", "blockquote", "strong", "b", "em", "i", "del", "s", "table", "thead", "tbody", "tr", "td", "th", "details", "summary", "img", "picture", "sup", "sub", "kbd"],
    ALLOWED_ATTR: ["href", "title", "alt", "width", "height", "colspan", "rowspan", "id", "open", "data-github-image"],
    ALLOW_DATA_ATTR: false, ALLOW_ARIA_ATTR: false,
  });
  const doc = new DOMParser().parseFromString(clean, "text/html");
  doc.querySelectorAll<HTMLImageElement>("img").forEach(img => {
    const index = Number(img.dataset.githubImage);
    const item = Number.isInteger(index) ? note.media?.[index] : undefined;
    const src = localImage(item?.src);
    if (src) { img.src = src; img.loading = "lazy"; }
    else img.hidden = true;
  });
  doc.querySelectorAll("h1").forEach(h => h.classList.add("t-h2"));
  doc.querySelectorAll("h2,h3,h4,h5,h6").forEach(h => h.classList.add("t-h3"));
  doc.querySelectorAll<HTMLAnchorElement>("a[href]").forEach(a => {
    const href = a.getAttribute("href") || "";
    if (/^https?:\/\//.test(href)) { a.dataset.noteExternal = href; a.rel = "noopener noreferrer"; }
    else if (!href.startsWith("#")) a.removeAttribute("href");
    else a.removeAttribute("target");
  });
  return doc.body.innerHTML;
}

export function githubBody(input: NoteData): string {
  const note = githubNote(input.url || "", input) || input as Preview;
  const key = keyFor(note.url || "") || "";
  const entry = cache.get(key);
  const info = note.github;
  const loading = !entry?.note && !entry?.error;
  const failedImages = note.media?.filter(m => m.download_state === "failed").length || 0;
  const status = entry?.error ? `<p class="t-small subtle">${esc(tr("暂时无法加载，已有的本地内容仍可阅读。", "Could not load the preview. Saved content remains available."))}</p>` : "";
  let body = loading ? `<p>${esc(tr("正在读取 README 和仓库信息…", "Loading README and repository metadata…"))}</p>` : "";
  if (info?.readme_available) body = `<h2 class="t-h3">README</h2><div class="github-readme">${readme(note)}</div>`;
  else if (info?.kind === "repository") body = `<p class="t-small subtle">${esc(tr("README 暂不可用。仓库可能未提供 README，或读取受限。", "README unavailable. It may be missing or access may be limited."))}</p>`;
  else if (info?.page_data?.items) body = info.page_data.items.map(item => `<p><a href="${esc(item.url || "")}" data-note-external="${esc(item.url || "")}">${esc(item.full_name || item.login || item.url || "GitHub")}</a><br>${esc(item.description || "")}</p>`).join("");
  else if (note.content) body = `<p>${esc(note.content)}</p>`;
  return `${githubMetadata(note, true)}${note.excerpt ? `<p>${esc(note.excerpt)}</p>` : ""}<div class="github-preview-status">${status}
    ${failedImages ? `<p class="t-small subtle">${esc(tr(`${failedImages} 张图片暂不可用。`, `${failedImages} images are unavailable.`))}</p>` : ""}</div><div class="github-preview-content">${body}</div>`;
}

// Keep loaded image nodes (and their decoded pixels) in place during polling.
// Only changed attributes/text and newly available content touch the live DOM.
export function updateGithubPreview(element: HTMLElement, html: string, contentsOnly = false): void {
  const template = document.createElement("template");
  template.innerHTML = html;
  function patch(current: Node, next: Node): void {
    if (current.nodeType !== next.nodeType || current.nodeName !== next.nodeName) {
      current.parentNode?.replaceChild(next.cloneNode(true), current);
      return;
    }
    if (current instanceof Element && next instanceof Element) {
      for (const attr of Array.from(current.attributes)) {
        if (current.tagName === "DETAILS" && attr.name === "open") continue;
        if (!next.hasAttribute(attr.name)) current.removeAttribute(attr.name);
      }
      for (const attr of Array.from(next.attributes)) {
        if (current.tagName === "DETAILS" && attr.name === "open") continue;
        if (current.getAttribute(attr.name) !== attr.value) current.setAttribute(attr.name, attr.value);
      }
      children(current, next);
    } else if (current.nodeValue !== next.nodeValue) current.nodeValue = next.nodeValue;
  }
  function children(current: Node, next: Node): void {
    const before = Array.from(current.childNodes);
    const after = Array.from(next.childNodes);
    for (let i = 0; i < Math.max(before.length, after.length); i++) {
      if (!before[i]) current.appendChild(after[i].cloneNode(true));
      else if (!after[i]) current.removeChild(before[i]);
      else patch(before[i], after[i]);
    }
  }
  if (contentsOnly) children(element, template.content);
  else if (template.content.firstElementChild) patch(element, template.content.firstElementChild);
}
