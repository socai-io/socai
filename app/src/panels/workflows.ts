import { esc } from "../lib/html";
import { getLanguage } from "../lib/i18n";
export interface WorkflowSkill { id: string; name: string; description: string; instructions: string; }
const key = "socai.custom-workflows.v1";
const tr = (zh: string, en: string): string => getLanguage() === "zh" ? zh : en;
const icons: Record<string, string> = { general: '<circle cx="12" cy="12" r="8"/><path d="m15 9-2 4-4 2 2-4Z"/>', vc: '<circle cx="6" cy="7" r="2"/><circle cx="18" cy="6" r="2"/><circle cx="12" cy="18" r="2"/><path d="m8 7 8-1M7 9l4 7m6-8-4 8"/>', custom: '<path d="m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5Z"/>', add: '<circle cx="12" cy="12" r="9" fill="var(--fg)" stroke="var(--fg)"/><path d="M12 7v10M7 12h10" stroke="var(--canvas)"/>' };
export const workflowIcon = (id: string): string => `<svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">${icons[id] ?? icons.custom}</svg>`;
export function customWorkflows(): WorkflowSkill[] {
  try { const rows: unknown = JSON.parse(localStorage.getItem(key) || "[]"); return Array.isArray(rows) ? rows.filter((r): r is WorkflowSkill => r && ["id", "name", "description", "instructions"].every(k => typeof r[k] === "string")) : []; } catch { return []; }
}
export function workflowLabel(id: string, saved?: WorkflowSkill | null): string {
  return id === "vc" ? tr("VC 研究", "VC research") : id === "general" ? tr("通用", "General") : customWorkflows().find(w => w.id === id)?.name || saved?.name || tr("自定义专家模式", "Custom expert mode");
}
let opened = false;
let editing: WorkflowSkill | null = null;
let error = "";
export function openWorkflows(): void { opened = true; editing = null; error = ""; }
export function renderWorkflows(selected: string): string {
  if (!opened) return "";
  const builtins = [
    { id: "general", name: tr("通用", "General"), description: tr("围绕问题自由探索，连接跨平台信息与线索。", "Explore a question and connect findings across platforms.") },
    { id: "vc", name: tr("VC 研究", "VC research"), description: tr("发现尚未被熟知的团队与新趋势，绘制机会地图。", "Discover emerging teams and signals. Map new opportunities.") },
  ];
  return `<div class="workflow-backdrop" data-workflow-dismiss><section class="workflow-dialog" role="dialog" aria-modal="true" aria-labelledby="workflow-title" tabindex="-1">
    <header><div><h2 id="workflow-title" class="t-h2">${tr("选择专家模式", "Choose expert mode")}</h2></div><button class="btn-ghost" data-workflow-close aria-label="${tr("关闭", "Close")}">×</button></header>
    ${editing ? `<form id="workflow-editor"><label class="t-small">${tr("名称", "Name")}<input name="name" maxlength="80" required value="${esc(editing.name)}"></label><label class="t-small">${tr("简介", "Description")}<input name="description" maxlength="300" value="${esc(editing.description)}"></label><label class="t-small">${tr("研究指导 / skill.md", "Instructions / skill.md")}<textarea name="instructions" maxlength="40000" required rows="10" placeholder="${tr("写下关注的信号、研究方法和期望交付…", "Describe signals, research methods and expected deliverables…")}">${esc(editing.instructions)}</textarea></label><div class="workflow-editor-actions"><label class="btn-ghost workflow-import">${tr("导入 .md", "Import .md")}<input type="file" accept=".md,.markdown,text/markdown,text/plain" id="workflow-file"></label><button type="button" class="btn-ghost" data-workflow-back>${tr("返回", "Back")}</button><button type="submit" class="btn-primary">${tr("保存并使用", "Save and use")}</button></div></form>` : `<div class="workflow-grid">${[...builtins, ...customWorkflows()].map(w => `<article class="workflow-card ${selected === w.id ? "is-selected" : ""}"><button data-workflow-pick="${esc(w.id)}" aria-pressed="${selected === w.id}"><span class="workflow-card-icon">${workflowIcon(w.id)}</span><strong class="t-h3">${esc(w.name)}</strong><span class="t-small">${esc(w.description)}</span><span class="workflow-card-check">${selected === w.id ? "✓" : ""}</span></button>${"instructions" in w ? `<button class="workflow-edit t-small" data-workflow-edit="${esc(w.id)}">${tr("编辑", "Edit")}</button>` : ""}</article>`).join("")}<button class="workflow-card workflow-card-add" data-workflow-new>${workflowIcon("add")}<strong class="t-h3">${tr("定制专家模式", "Custom expert mode")}</strong><span class="t-small">${tr("自己编写，或导入一个 skill.md", "Write your own or import a skill.md")}</span></button></div>`}
    <p class="workflow-error t-small" role="alert">${esc(error)}</p>
  </section></div>`;
}
export function bindWorkflows(rerender: () => void, choose: (id: string, skill?: WorkflowSkill) => void): void {
  const backdrop = document.querySelector<HTMLElement>(".workflow-backdrop");
  if (!backdrop) return;
  const dialog = backdrop.querySelector<HTMLElement>(".workflow-dialog")!;
  const close = (): void => { opened = false; rerender(); document.getElementById("composer-workflow")?.focus(); };
  backdrop.addEventListener("click", e => { if (e.target === backdrop) close(); });
  backdrop.querySelector("[data-workflow-close]")?.addEventListener("click", close);
  dialog.focus();
  dialog.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.stopPropagation(); close(); }
    if (e.key === "Tab") {
      const nodes = [...dialog.querySelectorAll<HTMLElement>('button, input:not([type="file"]), textarea')].filter(n => !n.hasAttribute("disabled"));
      const first = nodes[0], last = nodes.at(-1);
      if (e.shiftKey && (document.activeElement === first || document.activeElement === dialog)) { e.preventDefault(); last?.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
    }
  });
  backdrop.querySelectorAll<HTMLElement>("[data-workflow-pick]").forEach(b => b.addEventListener("click", () => { const id = b.dataset.workflowPick!; choose(id, customWorkflows().find(w => w.id === id)); close(); }));
  backdrop.querySelector("[data-workflow-new]")?.addEventListener("click", () => { editing = { id: `custom-${crypto.randomUUID()}`, name: "", description: "", instructions: "" }; rerender(); });
  backdrop.querySelectorAll<HTMLElement>("[data-workflow-edit]").forEach(b => b.addEventListener("click", () => { editing = { ...customWorkflows().find(w => w.id === b.dataset.workflowEdit)! }; rerender(); }));
  backdrop.querySelector("[data-workflow-back]")?.addEventListener("click", () => { editing = null; error = ""; rerender(); });
  const form = backdrop.querySelector<HTMLFormElement>("#workflow-editor");
  form?.addEventListener("input", () => { if (!editing) return; const data = new FormData(form); for (const k of ["name", "description", "instructions"] as const) editing[k] = String(data.get(k) || ""); });
  backdrop.querySelector<HTMLInputElement>("#workflow-file")?.addEventListener("change", async e => {
    const file = (e.target as HTMLInputElement).files?.[0]; if (!file || !editing) return;
    if (file.size > 40000) { error = tr("文件请控制在 40 KB 内。", "Use a file smaller than 40 KB."); rerender(); return; }
    try { editing.instructions = await file.text(); if (!editing.name) editing.name = file.name.replace(/\.(md|markdown)$/i, ""); error = ""; } catch (e) { error = String(e); }
    rerender();
  });
  form?.addEventListener("submit", e => {
    e.preventDefault(); if (!editing?.name.trim() || !editing.instructions.trim()) return;
    try { const rows = customWorkflows().filter(w => w.id !== editing!.id); rows.push(editing); localStorage.setItem(key, JSON.stringify(rows)); choose(editing.id, editing); close(); }
    catch { error = tr("保存失败：设备存储空间不足或不可用。", "Could not save: local storage is unavailable or full."); rerender(); }
  });
}
