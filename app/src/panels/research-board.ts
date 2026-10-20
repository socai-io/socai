import { renderLeadCitation } from "./notes";
import { esc } from "../lib/html";
import { getLanguage } from "../lib/i18n";
import { platformIcon, platformLabel } from "../lib/platforms";
import type { AgentTaskView } from "./tasks";
export interface BoardLead { id: string; name: string; platform: string; signal: string; uncertainty?: string; url: string; stage: string; }
export interface BoardRelation { id: string; from: string; to: string; label: string; url: string; uncertain: boolean; dimension?: string; kind?: "fact" | "comparison" | "hypothesis"; basis?: string; sources?: Array<{lead_id: string; url: string; excerpt?: string}>; }
export interface ResearchBoard { operations?: Array<{platform: string; tool: string; detail: string; failed: boolean; at: number}>; summary?: string; next_steps?: string[]; updated_at?: number; platforms?: Array<{ id: string; status: string; focus: string }>; leads?: BoardLead[]; relations?: BoardRelation[]; }
const tr = (zh: string, en: string): string => getLanguage() === "zh" ? zh : en;
const filters = new Map<string, string>();
const dimensionLabels = (): Record<string,string> => ({identity:tr("同一实体","Identity"),people:tr("人物与组织","People"),problem:tr("问题与场景","Problem"),audience:tr("用户与市场","Audience"),technology:tr("技术路线","Technology"),ecosystem:tr("生态依赖","Ecosystem"),business:tr("商业模式","Business"),stage:tr("发展阶段","Stage")});
const kindLabel = (edge: BoardRelation): string => edge.kind === "comparison" ? tr("分析关联","Comparison") : edge.uncertain || edge.kind === "hypothesis" ? tr("待核实假设","Hypothesis") : tr("来源所述关系","Source-stated connection");
const scrollPositions = new Map<string, {top: number; follow: boolean}>();

const layoutCache = new Map<string, Map<string, {x:number;y:number}>>();
function networkLayout(ids: string[], edges: BoardRelation[]): Map<string, {x:number;y:number}> {
  const key = JSON.stringify([ids,edges.map(e=>[e.from,e.to])]);
  const cached = layoutCache.get(key); if (cached) return cached;
  const nodes = ids.map((id,i) => ({id,x:480+Math.cos(i*2.39996)*Math.sqrt(i+1)*34,y:310+Math.sin(i*2.39996)*Math.sqrt(i+1)*26,dx:0,dy:0}));
  const lookup = new Map(nodes.map(n=>[n.id,n]));
  for (let step=0;step<220;step++) {
    for (const n of nodes) { n.dx=(480-n.x)*.008;n.dy=(310-n.y)*.008; }
    for (let i=0;i<nodes.length;i++) for(let j=i+1;j<nodes.length;j++) {
      const a=nodes[i],b=nodes[j],dx=a.x-b.x,dy=a.y-b.y,d2=Math.max(36,dx*dx+dy*dy),f=1700/d2;
      a.dx+=dx*f;a.dy+=dy*f;b.dx-=dx*f;b.dy-=dy*f;
      if(Math.abs(dx)<120&&Math.abs(dy)<55){const push=(55-Math.abs(dy))*.12*(dy>=0?1:-1);a.dy+=push;b.dy-=push;}
    }
    for (const e of edges) {const a=lookup.get(e.from)!,b=lookup.get(e.to)!,dx=b.x-a.x,dy=b.y-a.y,d=Math.max(1,Math.hypot(dx,dy)),f=(d-110)*.025/d;a.dx+=dx*f;a.dy+=dy*f;b.dx-=dx*f;b.dy-=dy*f;
      if(Math.abs(dx)<120&&Math.abs(dy)<55){const push=(55-Math.abs(dy))*.12*(dy>=0?1:-1);a.dy+=push;b.dy-=push;}}
    const cool=1-step/250;
    for(const n of nodes){n.x+=Math.max(-12,Math.min(12,n.dx))*cool;n.y+=Math.max(-12,Math.min(12,n.dy))*cool;}
  }
  const minX=Math.min(...nodes.map(n=>n.x)),maxX=Math.max(...nodes.map(n=>n.x)),minY=Math.min(...nodes.map(n=>n.y)),maxY=Math.max(...nodes.map(n=>n.y));
  const result=new Map(nodes.map(n=>[n.id,{x:65+(n.x-minX)/Math.max(1,maxX-minX)*830,y:40+(n.y-minY)/Math.max(1,maxY-minY)*530}]));
  if(layoutCache.size>16)layoutCache.clear();layoutCache.set(key,result);return result;
}
function graph(board: ResearchBoard, _taskId: string): string {
  const leads = new Map((board.leads ?? []).map(l => [l.id, l]));
  if (!leads.size) return "";
  const edges = (board.relations ?? []).filter(e => leads.has(e.from) && leads.has(e.to));
  const positions=networkLayout([...leads.keys()],edges);
  const degree=new Map<string,number>();for(const e of edges)for(const id of [e.from,e.to])degree.set(id,(degree.get(id)??0)+1);
  return `<div class="research-map"><div class="research-map-controls"><button data-map-zoom="in" aria-label="${tr('放大','Zoom in')}">+</button><button data-map-zoom="out" aria-label="${tr('缩小','Zoom out')}">−</button><button data-map-zoom="reset" aria-label="${tr('重置视图','Reset view')}">⤢</button></div><svg viewBox="0 0 960 640" role="group" aria-label="${tr('线索关系图','Lead connections')}"><g class="research-network">${edges.map(e=>{const a=positions.get(e.from)!,b=positions.get(e.to)!;return `<g role="button" tabindex="0" data-research-edge="${esc(e.id)}" data-edge-label="${esc(e.label)}" data-edge-from="${esc(e.from)}" data-edge-to="${esc(e.to)}" aria-label="${esc(`${leads.get(e.from)!.name} · ${e.label} · ${leads.get(e.to)!.name} · ${kindLabel(e)}`)}"><path class="research-edge-hit" d="M${a.x},${a.y} L${b.x},${b.y}"/><path class="research-edge ${e.kind==='comparison'?'is-comparison':e.uncertain?'is-hypothesis':''}" d="M${a.x},${a.y} L${b.x},${b.y}"/></g>`}).join('')}${[...leads].map(([id,lead])=>{const p=positions.get(id)!,r=5+Math.min(7,(degree.get(id)??0)*1.3);return `<g data-network-node="${esc(id)}" transform="translate(${p.x} ${p.y})"><title>${esc(lead.name)}</title><circle style="fill:${({xhs:'#b98180',x:'#7e91ac',weixin:'#7f9d86',podcast:'#a18bb2',github:'#ac9874'} as Record<string,string>)[lead.platform]||'#8b9fa1'}" r="${r}"/><foreignObject x="-58" y="${r+3}" width="116" height="38"><div xmlns="http://www.w3.org/1999/xhtml" class="research-map-node">${renderLeadCitation(lead.url,lead.name,lead.platform)}</div></foreignObject></g>`}).join('')}</g></svg></div>`;
}
export function renderResearchBoard(task: AgentTaskView): string {
  if (!task.research_mode) return "";
  const board = task.research_board ?? {};
  const running = ["running", "queued"].includes(task.status);
  const leads = board.leads ?? [];
  if (!running && !leads.length && !board.summary && !board.operations?.length) return "";
  const counts = new Map<string, number>();
  for (const note of task.notes ?? []) counts.set(note.site || "xhs", (counts.get(note.site || "xhs") ?? 0) + 1);
  const operations = [...(board.operations ?? [])];
  const start = task.events.map(e => e.kind).lastIndexOf("started");
  const events = task.events.slice(Math.max(0, start));
  for (const event of events.filter(e => e.kind === "tool_result" || e.kind === "tool_error")) {
    const call = events.find(e => e.kind === "tool_call" && e.id === event.id);
    if (!call || call.name === "explore_platforms" || call.name === "update_research_board") continue;
    const args = (call.args ?? {}) as Record<string, unknown>;
    const name = String(args.tool_name || call.name || "");
    let platform = typeof args.site_id === "string" ? args.site_id : "";
    if (!platform) {
      if (name.includes("weixin")) platform = "weixin";
      else if (name.includes("podcast")) platform = "podcast";
      else if (name === "github_request") platform = "github";
      else if (["search", "author_scan", "get_notes", "explore"].includes(name)) platform = "xhs";
    }
    if (!platform) continue;
    operations.push({platform, tool:name, detail: typeof args.query === "string" ? args.query : "", failed:event.kind === "tool_error" || event.ok === false, at:event.created_at});
  }
  operations.sort((a,b) => a.at-b.at);
  const dispatched = new Set<string>();
  for (const event of task.events) {
    if (event.kind === "tool_call" && event.name === "explore_platforms") {
      for (const b of (event.args as { branches?: Array<{site: string}> })?.branches ?? []) dispatched.add(b.site);
    }
  }
  const sites = [...new Set([...(task.sites ?? []).filter(s => s !== "auto"), ...dispatched, ...counts.keys(), ...(board.platforms ?? []).map(p => p.id), ...leads.map(l => l.platform), ...operations.map(o => o.platform)])];
  const statusLabel: Record<string,string> = {scouting:tr("发现", "Scouting"), reading:tr("阅读", "Reading"), following:tr("追踪", "Following"), synthesizing:tr("归纳", "Synthesis"), blocked:tr("受阻", "Blocked"), done:tr("已汇总", "Summarized")};

  const actionLabel = (name: string): string => {
    if (/comment/i.test(name)) return tr("阅读评论", "Read comments");
    if (/author|profile/i.test(name)) return tr("追踪作者", "Explore profile");
    if (/search/i.test(name)) return tr("发现内容", "Discover");
    if (/Detail|get_notes|read_weixin|read_podcast/i.test(name)) return tr("深入阅读", "Read details");
    if (name === "github_request") return tr("查看仓库 / 项目", "Inspect repository");
    if (name === "navigate_site") return tr("进入页面", "Open page");
    if (/skill/i.test(name)) return tr("加载平台能力", "Load platform tools");
    return tr("读取平台内容", "Read platform content");
  };
  const filter = filters.get(task.task_id) ?? "all";

  const shown = leads.filter(l => filter === "all" || l.platform === filter);
  return `<section class="research-board" data-board-task="${esc(task.task_id)}" aria-label="${tr("研究工作台", "Research workbench")}">
    <header class="research-board-head"><h3 class="t-h3">${tr("研究工作台", "Research workbench")}</h3></header>
    ${board.summary ? `<div class="research-hypothesis t-body">${esc(board.summary)}</div>` : ""}
    <div class="research-platforms">${sites.map(site => { const p = board.platforms?.find(p => p.id === site); return `<div class="research-platform ${p?.status === "blocked" ? "is-blocked" : ""}"><div>${platformIcon(site)}<strong class="t-small">${esc(platformLabel(site))}</strong><span class="t-small">${esc(p ? statusLabel[p.status] ?? p.status : counts.has(site) ? tr("已采集", "Collected") : dispatched.has(site) ? tr("已分派", "Assigned") : tr("待探索", "Pending"))}</span></div>${p?.focus ? `<p class="t-small">${esc(p.focus)}</p>` : ""}${platformStats(task, site, counts.get(site) ?? 0)}<ol class="research-operation-path">${operations.filter(o => o.platform === site).slice(-3).map(o => `<li title="${esc(o.detail)}"><span>${o.failed ? "!" : "↳"}</span>${esc(actionLabel(o.tool))}${o.failed ? ` · ${tr("受限", "limited")}` : ""}</li>`).join("")}</ol></div>`; }).join("") || `<p class="t-small research-empty">${tr("正在选择适合的平台", "Choosing relevant platforms")}</p>`}</div>
    ${graph(board, task.task_id)}
    <div class="research-board-nav"><h4 class="t-small">${tr("线索收集", "Leads")} · ${leads.length}</h4></div>
    <div class="research-filters"><button data-research-filter="all" aria-pressed="${filter === "all"}">${tr("全部", "All")}</button>${sites.map(s => `<button data-research-filter="${esc(s)}" aria-pressed="${filter === s}">${platformIcon(s)}${esc(platformLabel(s))}</button>`).join("")}</div>
    <div class="research-table-wrap">${shown.length ? `<table class="research-table"><thead><tr><th>${tr("团队 / 产品 / 方向", "Team / product / direction")}</th><th>${tr("发现的信号", "Signal")}</th></tr></thead><tbody>${shown.map(l => `<tr><td>${renderLeadCitation(l.url,l.name,l.platform)}</td><td>${esc(l.signal)}${l.uncertainty ? `<p>${esc(l.uncertainty)}</p>` : ""}</td></tr>`).join("")}</tbody></table>` : ""}</div>
    ${running && board.next_steps?.length ? `<footer class="research-next"><span class="t-eyebrow">${tr("接下来", "NEXT")}</span><ol>${board.next_steps.map(s => `<li>${esc(s)}</li>`).join("")}</ol></footer>` : ""}
  </section>`;
}
function platformStats(task: AgentTaskView, site: string, count: number): string {
  const latest = Math.max(0, ...(task.readingStats ?? []).map(s => s.turn_index));
  const stats = task.readingStats?.find(s => s.turn_index === latest && (s.site || 'xhs') === site);
  if (!stats) return count > 0 ? `<small>${count} ${tr("条已存来源","saved sources")}</small>` : "";
  const items: Array<[string, number]> = [[tr('速读','Preview'), stats.preview_notes], [tr('精读','Detail'),stats.detail_notes], [tr('评论','Comments'),stats.comments], [stats.authors_kind === 'observed' ? tr('来源作者','Source authors') : tr('追踪作者','Profiles read'),stats.authors]];
  const text = items.filter(([,n]) => n > 0).map(([label,n]) => `${label} ${n}`).join(' · ');
  return text ? `<small>${esc(text)}</small>` : "";
}
export function refreshResearchBoard(host: HTMLElement, task: AgentTaskView): void {
  const markup = renderResearchBoard(task);
  if (host.dataset.boardMarkup === markup) return;
  const previous = host.querySelector('.research-table-wrap');
  const tableScroll = previous?.scrollTop ?? 0;
  const follow = !previous || previous.scrollHeight - previous.clientHeight - previous.scrollTop < 32;
  const relationScroll = host.querySelector('.research-relations')?.scrollTop ?? 0;
  const graphScroll = host.querySelector('.research-map')?.scrollTop ?? 0;
  host.innerHTML = markup;
  host.dataset.boardMarkup = markup;
  const table = host.querySelector('.research-table-wrap'), relations = host.querySelector('.research-relations'), map = host.querySelector('.research-map');
  if (table) table.scrollTop = follow ? table.scrollHeight : tableScroll;
  if (relations) relations.scrollTop = relationScroll;
  if (map) map.scrollTop = graphScroll;
}
function showRelation(task: AgentTaskView, id: string): void {
  const edge = task.research_board?.relations?.find(e => e.id === id); if (!edge) return;
  document.querySelector('.research-relation-dialog')?.remove();
  const names = new Map(task.research_board?.leads?.map(l => [l.id,l.name]));
  const sources = edge.sources?.length ? edge.sources : [{lead_id:edge.from,url:edge.url}];
  const dialog = document.createElement('dialog');
  dialog.className = 'research-relation-dialog';
  dialog.setAttribute('aria-label', edge.label);
  dialog.innerHTML = `<button class="research-relation-close" aria-label="${tr('关闭','Close')}">×</button><p class="t-small">${esc(dimensionLabels()[edge.dimension ?? ''] ?? '')} · ${esc(kindLabel(edge))}</p><h3 class="t-h3">${esc(edge.label)}</h3><p class="t-small">${esc(names.get(edge.from) ?? edge.from)} ↔ ${esc(names.get(edge.to) ?? edge.to)}</p>${edge.basis ? `<p class="t-body">${esc(edge.basis)}</p>` : ''}<div class="research-evidence">${sources.map(s => { let url = ''; try { const parsed = new URL(s.url); if (['http:','https:'].includes(parsed.protocol)) url = parsed.href; } catch { /* Invalid legacy URL stays plain text. */ } return `<div>${url ? `<button class="research-source" data-note-external="${esc(url)}">${esc(names.get(s.lead_id) ?? tr('来源','Source'))} ↗</button>` : esc(names.get(s.lead_id) ?? '')}${s.excerpt ? `<blockquote>${esc(s.excerpt)}</blockquote>` : ''}</div>`; }).join('')}</div>`;
  const previousFocus = document.activeElement;
  const close = (): void => { dialog.close(); dialog.remove(); if (previousFocus instanceof HTMLElement || previousFocus instanceof SVGElement) previousFocus.focus(); };
  dialog.querySelector('button')!.addEventListener('click',close);
  dialog.addEventListener('cancel', e => { e.preventDefault(); close(); });
  dialog.addEventListener('click', e => { if (e.target === dialog) { const rect = dialog.getBoundingClientRect(); if (e.clientX < rect.left || e.clientX > rect.right || e.clientY < rect.top || e.clientY > rect.bottom) close(); } });
  document.body.append(dialog); dialog.showModal();
}
function bindNetworkGestures(): void {
  let edgeTip: HTMLElement | null = null;
  const hideTip = (): void => { edgeTip?.remove(); edgeTip = null; };
  document.addEventListener('pointerover', e => {
    const edge=(e.target as Element).closest<SVGGElement>('[data-edge-label]'); if(!edge)return;
    hideTip();edgeTip=document.createElement('div');edgeTip.className='research-edge-tooltip';edgeTip.setAttribute('role','tooltip');edgeTip.textContent=edge.dataset.edgeLabel??'';document.body.append(edgeTip);
    edgeTip.style.left=`${Math.max(8,Math.min(e.clientX+10,window.innerWidth-edgeTip.offsetWidth-8))}px`;
    edgeTip.style.top=`${Math.max(8,Math.min(e.clientY+12,window.innerHeight-edgeTip.offsetHeight-8))}px`;
  });
  document.addEventListener('pointerout', e => {const edge=(e.target as Element).closest('[data-edge-label]');if(edge&&(!(e.relatedTarget instanceof Node)||!edge.contains(e.relatedTarget)))hideTip();});
  document.addEventListener('scroll',hideTip,true);document.addEventListener('pointerdown',hideTip);
  let drag: {map:HTMLElement; svg:SVGSVGElement; node:SVGGElement|null; x:number; y:number; moved:boolean; pointer:number} | null=null;
  let suppressClick=false;
  const view=(map:HTMLElement):number[] => (map.dataset.networkView??"0 0 1").split(" ").map(Number);
  const apply=(map:HTMLElement,x:number,y:number,k:number):void=>{map.dataset.networkView=`${x} ${y} ${k}`;map.querySelector('.research-network')?.setAttribute('transform',`translate(${x} ${y}) scale(${k})`);};
  const zoom=(map:HTMLElement,factor:number):void=>{const [x,y,k]=view(map),next=Math.max(.4,Math.min(4,k*factor));apply(map,480-(480-x)*next/k,320-(320-y)*next/k,next);};
  const point=(svg:SVGSVGElement,e:PointerEvent):DOMPoint=>new DOMPoint(e.clientX,e.clientY).matrixTransform(svg.getScreenCTM()!.inverse());
  document.addEventListener('click',e=>{if(suppressClick){suppressClick=false;e.preventDefault();e.stopImmediatePropagation();return;}const button=(e.target as Element).closest<HTMLElement>('[data-map-zoom]');if(!button)return;const map=button.closest<HTMLElement>('.research-map')!;if(button.dataset.mapZoom==='reset')apply(map,0,0,1);else zoom(map,button.dataset.mapZoom==='in'?1.25:.8);},true);
  document.addEventListener('wheel',e=>{const map=(e.target as Element).closest<HTMLElement>('.research-map');if(!map||!e.ctrlKey&&!e.metaKey)return;e.preventDefault();zoom(map,e.deltaY<0?1.12:1/1.12);},{passive:false});
  document.addEventListener('pointerdown',e=>{
    if(e.button!==0)return;const map=(e.target as Element).closest<HTMLElement>('.research-map');if(!map||(e.target as Element).closest('button,foreignObject,[data-research-edge]'))return;
    const svg=map.querySelector('svg') as SVGSVGElement,p=point(svg,e);drag={map,svg,node:(e.target as Element).closest<SVGGElement>('[data-network-node]'),x:p.x,y:p.y,moved:false,pointer:e.pointerId};suppressClick=false;svg.setPointerCapture(e.pointerId);
  });
  document.addEventListener('pointermove',e=>{
    if(!drag||drag.pointer!==e.pointerId)return;const p=point(drag.svg,e),dx=p.x-drag.x,dy=p.y-drag.y;if(!drag.moved&&Math.hypot(dx,dy)<3)return;drag.moved=true;drag.map.classList.add('is-dragging');const [x,y,k]=view(drag.map);
    if(drag.node){const transform=drag.node.transform.baseVal.getItem(0).matrix,nx=transform.e+dx/k,ny=transform.f+dy/k;drag.node.setAttribute('transform',`translate(${nx} ${ny})`);const id=drag.node.dataset.networkNode;
      for(const edge of drag.map.querySelectorAll<SVGGElement>('[data-research-edge]')){if(edge.dataset.edgeFrom!==id&&edge.dataset.edgeTo!==id)continue;const endpoints=[edge.dataset.edgeFrom,edge.dataset.edgeTo].map(endpoint=>Array.from(drag!.map.querySelectorAll<SVGGElement>('[data-network-node]')).find(n=>n.dataset.networkNode===endpoint)!.transform.baseVal.getItem(0).matrix);for(const path of edge.querySelectorAll('path'))path.setAttribute('d',`M${endpoints[0].e},${endpoints[0].f} L${endpoints[1].e},${endpoints[1].f}`);}
    }else apply(drag.map,x+dx,y+dy,k);drag.x=p.x;drag.y=p.y;
  });
  const end=():void=>{if(!drag)return;suppressClick=drag.moved;drag.map.classList.remove('is-dragging');drag=null;};document.addEventListener('pointerup',end);document.addEventListener('pointercancel',end);
}
let bound = false;
export function bindResearchBoard(current: () => AgentTaskView | undefined): void {
  requestAnimationFrame(() => {
    const task = current(); if (!task) return;
    const table = document.querySelector('.research-table-wrap'); if (!table) return;
    const state = scrollPositions.get(task.task_id);
    table.scrollTop = state ? (state.follow ? table.scrollHeight : state.top) : (task.status === 'running' ? table.scrollHeight : 0);
  });
  if (bound) return; bound = true;
  bindNetworkGestures();
  document.addEventListener('scroll', e => {
    const table = e.target;
    if (!(table instanceof HTMLElement) || !table.matches('.research-table-wrap')) return;
    const id = table.closest<HTMLElement>('[data-board-task]')?.dataset.boardTask;
    if (id) scrollPositions.set(id, {top:table.scrollTop, follow:table.scrollHeight-table.clientHeight-table.scrollTop < 32});
  }, true);
  document.addEventListener('keydown', e => {
    const edge = (e.target as Element).closest<HTMLElement>('[data-research-edge]');
    if (edge && ['Enter',' '].includes(e.key)) { e.preventDefault(); const task = current(); if (task) showRelation(task,edge.dataset.researchEdge!); }
  });
  document.addEventListener('click', e => {
    const task = current();
    const circle = (e.target as Element).closest('[data-network-node] > circle');
    if (circle) { circle.parentElement?.querySelector<HTMLButtonElement>('[data-note-open]')?.click(); return; }
    const edge = (e.target as Element).closest<HTMLElement>('[data-research-edge]');
    if (task && edge) { showRelation(task,edge.dataset.researchEdge!); return; }
    const button = (e.target as Element).closest<HTMLElement>('[data-research-filter]');
    if (!button || !task) return;
    filters.set(task.task_id, button.dataset.researchFilter!);
    const host = button.closest<HTMLElement>('[data-research-board]');
    if (host) refreshResearchBoard(host, task);
  });
}
