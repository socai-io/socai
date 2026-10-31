import { esc } from "../lib/html";
import { getLanguage } from "../lib/i18n";
import "./research-starters.css";

type Copy = readonly [string, string];
type Action = { id: string; label: Copy; goal: Copy; urls?: boolean; comments?: boolean };
type Scene = { id: string; label: Copy; description: Copy; actions: readonly Action[] };
const tr = (copy: Copy): string => copy[getLanguage() === "zh" ? 0 : 1];

// This catalog and its input UI are shared with Web. They prepare a real task,
// never simulate a result, submit automatically, or change browser permissions.
export const researchScenes: readonly Scene[] = [
  { id: "content", label: ["找选题与写初稿", "Find ideas and draft"], description: ["从真实内容和搜索需求，找到下一篇怎么写。", "Turn real content and search demand into your next draft."], actions: [
    { id: "first-draft", label: ["从参考作品到第一版稿件", "Reference to first draft"], urls: true, goal: ["采集参考作品，拆解标题、开头、结构、视觉或视频表达和评论反馈；给出3个有差异的选题，再选一个生成可编辑的标题、封面方案和60秒口播或图文初稿。区分原作事实与我的素材，不移植原作者经历。", "Read the reference, explain its title, opening, structure, visual or video choices and audience feedback. Propose three distinct ideas and draft one with a title, cover concept and editable 60-second script or carousel. Separate source facts from my own material; never copy the author's personal experience."] },
    { id: "low-follower-hit", label: ["找低粉高互动作品", "Find small-account hits"], goal: ["搜索该方向的真实作品，读取作者粉丝与作品互动，按明确阈值筛出小账号的高互动样本。缺少粉丝数不能判为低粉；区分单篇偶发与账号稳定表现，保留反例和来源。", "Find real posts and verify author followers and engagement before applying explicit small-account/high-engagement thresholds. Missing follower counts are unknown, not small accounts. Distinguish one-off hits from repeatable performance and keep counterexamples and links."] },
    { id: "topic-direction", label: ["找值得尝试的内容方向", "Discover content directions"], goal: ["按用户需求、使用场景和内容角度整理代表作品，给出各方向的样本数、互动分布、近期证据、反例和可执行选题；样本内热度不能描述为全平台排名或确定增长。", "Group representative posts by audience need, context and angle. Show sample size, engagement distribution, dates, counterexamples and actionable ideas. Do not turn sample popularity into a platform-wide ranking or proven growth."] },
    { id: "longtail-demand", label: ["找用户正在搜索的问题", "Explore search demand"], goal: ["读取平台真实搜索联想词，保留主词、联想词、时间及来源，按用户具体问题整理长尾需求和选题。没有搜索量数据时不编造流量数字；无法取得联想词时明确说明，不用模型生成词冒充平台词。", "Read actual platform search suggestions with their seed query, date and source. Group long-tail audience questions and ideas. Never invent search volume or substitute model-generated phrases for unavailable platform suggestions."] },
  ] },
  { id: "assets", label: ["保存和整理素材", "Save and organize assets"], description: ["保存附件、提取逐字稿，交付能打开的文件。", "Save media and extract transcripts into usable files."], actions: [
    { id: "materials", label: ["保存单篇作品的素材", "Save a post's media"], urls: true, goal: ["读取作品正文、原链接和基础数据，下载可取得的原始图片或视频，必要时提取图片文字或语音。交付真实可下载的文件清单，逐项标注成功或失败；不能把网页缩略图或仅有URL说成完整视频已下载。", "Read the post text, original URL and public metadata; download available images or video and extract text or speech when needed. Deliver actual downloadable files with per-file success or failure. A thumbnail or URL alone is not a downloaded full video."] },
    { id: "batch-transcript", label: ["批量提取视频逐字稿", "Transcribe videos"], urls: true, goal: ["逐条取得视频并实际转写口播，保存原链接、语言、完整逐字稿和真实可得的时间戳；区分无口播、下载失败、转写失败，不由标题推测逐字稿。单条失败不重放已完成项，交付逐条结果和汇总文件。", "Retrieve each video and transcribe its speech, preserving URLs, language, full transcripts and available real timestamps. Distinguish no speech, download failure and transcription failure; never infer speech from a title. Keep successful items when another fails and deliver itemized results and a combined file."] },
    { id: "batch-images", label: ["批量保存封面和配图", "Save covers and images"], urls: true, goal: ["逐篇提取封面和全部可取得的配图，按作品及图片顺序保存，附原链接和文件清单；按真实媒体标识去重。分别记录预期、成功、失败数量与原因，输出可下载文件，不把封面当成全部配图。", "Save available covers and post images in post/page order with source URLs and a file manifest. Deduplicate by media identity and report expected, saved and failed counts with reasons. Deliver downloadable files; a cover is not the complete image set."] },
  ] },
  { id: "research", label: ["研究作品和对标账号", "Study posts and creators"], description: ["从一篇内容，追到账号的方法和后续变化。", "Go from one post to a creator's repeatable methods."], actions: [
    { id: "note-breakdown", label: ["拆解一篇作品", "Break down a post"], urls: true, goal: ["读取正文、图片或视频证据及可取得的评论，分别解释标题、开头、结构、视觉、表达、互动反馈和可借鉴方法。视频精确节奏须有时间戳或实际观看证据；没有完播率等后台数据不推断为事实。", "Read the post, visual or video evidence and available comments. Explain title, opening, structure, visual choices, expression, audience response and transferable methods. Precise video pacing requires timestamps or viewing evidence. Never invent private retention metrics."] },
    { id: "benchmark-discovery", label: ["找适合学习的对标账号", "Find relevant creators"], goal: ["从搜索作品反查作者，按账号ID去重，结合多篇作品说明定位、内容质量和可学习之处。提供代表作、反例和下一步研究建议，不靠同名认定跨平台同人，不凭单篇高赞推断持续增长。", "Discover creators from real search results and deduplicate by account ID. Use multiple posts to explain positioning, quality and useful methods, with representative posts, counterexamples and next steps. Names alone do not prove cross-platform identity; one viral post does not prove sustained growth."] },
    { id: "account-breakdown", label: ["研究一个账号的稳定方法", "Analyze a creator"], urls: true, goal: ["读取账号公开资料和多篇作品，比较定位、人群、内容支柱、标题、视觉、表达、发布节奏和互动分布，区分稳定规律与偶发高值。每项结论关联代表作及反例，保留样本范围、日期和未获取字段。", "Read the public profile and multiple posts. Compare positioning, audience, pillars, titles, visual and verbal style, cadence and engagement distribution. Separate repeatable patterns from outliers and link claims to examples and counterexamples, with dates, scope and missing fields."] },
    { id: "account-monitor", label: ["制定账号监控计划", "Plan creator monitoring"], urls: true, goal: ["先读取账号和近期作品建立去重基线，生成可确认的监控计划：账号ID、时区、执行时间、频率、每次范围、增量去重键及通知目标。先交付一次真实基线；只有定时任务创建接口返回可再次查询的任务ID后才能宣称监控已启用，否则明确仍是待确认计划。", "Read the profile and recent posts to establish an ID-based baseline. Propose a monitoring plan with account ID, timezone, schedule, cadence, sample scope, incremental keys and notification destination. Deliver a real baseline first. Monitoring is enabled only after a schedule is created with a retrievable task ID; otherwise label it a plan awaiting confirmation."] },
  ] },
  { id: "leads", label: ["了解反馈与客户需求", "Understand feedback and demand"], description: ["从公开讨论找问题，保留上下文与判断依据。", "Find needs in public discussions with context and evidence."], actions: [
    { id: "public-opinion", label: ["整理品牌和关键词反馈", "Research public feedback"], goal: ["采集品牌或关键词的真实讨论并去重，区分作者观点、用户经历、已核实事实及疑问，整理主题、正负中性反馈、争议与需要核实的风险。附代表来源和反例；搜索样本不代表全网舆情。", "Collect and deduplicate real discussions about a brand or query. Distinguish opinions, reported experiences, verified facts and questions. Summarize themes, sentiment, disagreements and risks needing verification with sources and counterexamples. A search sample is not the whole public conversation."] },
    { id: "comment-leads", label: ["从评论里找需求和选题", "Find needs in comments"], urls: true, comments: true, goal: ["读取评论及回复，按唯一ID去重，区分一级评论和回复；记录请求数、页面可见数、实际获取数、是否完整及不足原因。按用户指定需求词或公开地区筛选，保留原评论、作者、原链接和上下文，解释购买意向与普通讨论的区别，给出选题及待人工确认的线索，不自动发私信或公开回复。", "Read comments and replies, deduplicating by ID and keeping top-level/reply distinctions. Report requested, visible and collected counts, completeness and reasons for gaps. Filter by stated needs or public region while preserving source comments, authors, links and context. Separate buying intent from ordinary discussion and propose ideas and reviewable leads, without automatically sending messages or replies."] },
  ] },
];

let selectedScene = "";
let selectedAction = "";
let target = "";
let context = "";
let count = "3";
let error = "";
let pendingPrompt = "";

export function renderResearchStarters(): string {
  const scene = researchScenes.find(item => item.id === selectedScene);
  const action = scene?.actions.find(item => item.id === selectedAction) ?? scene?.actions[0];
  return `<section class="research-starters" aria-label="${tr(["按场景开始", "Start with a goal"])}">
    <h3 class="t-small">${tr(["从一个具体任务开始", "Start with a concrete task"])}</h3>
    <div class="research-starters__scenes">${researchScenes.map(item => `<button type="button" data-research-scene="${item.id}" aria-pressed="${item === scene}"><strong>${esc(tr(item.label))}</strong><span>${esc(tr(item.description))}</span></button>`).join("")}</div>
    ${scene && action ? `<form class="research-starters__form" data-research-starter-form>
      <div class="research-starters__head"><label>${tr(["这次想完成什么", "Choose a task"])}<select name="action">${scene.actions.map(item => `<option value="${item.id}"${item === action ? " selected" : ""}>${esc(tr(item.label))}</option>`).join("")}</select></label><button type="button" class="btn-ghost" data-research-starter-close aria-label="${tr(["关闭场景", "Close scenario"])}">×</button></div>
      <label>${tr(action.urls ? ["作品或账号链接，每行一个", "Post or profile URLs, one per line"] : ["主题、品牌或关键词", "Topic, brand or search query"])}<textarea name="target" rows="2" maxlength="1500" required>${esc(target)}</textarea></label>
      <div class="research-starters__fields"><label>${tr(action.comments ? ["期望评论数", "Requested comments"] : ["最多读取多少篇作品", "Maximum posts"])}<input name="count" type="number" min="1" max="${action.comments ? 1000 : 20}" value="${esc(count)}" required></label><label>${tr(["你的目标和已有资料（可选）", "Your goal and own facts (optional)"])}<textarea name="context" rows="2" maxlength="500">${esc(context)}</textarea></label></div>
      <p class="t-small subtle">${tr(["先填入任务，检查平台和内容后再发送；不会自动执行。", "Prepare the task, review its platform and details, then send. Nothing runs automatically."])}</p>
      ${error ? `<p role="alert">${esc(error)}</p>` : ""}${pendingPrompt ? `<div class="research-starters__confirmation" role="group" aria-label="${tr(["确认替换草稿", "Confirm draft replacement"])}"><p role="alert">${tr(["你有未发送的草稿。要用这个场景任务替换吗？原草稿尚未更改。", "You have an unsent draft. Replace it with this task? Your original draft has not changed."])}</p><button type="button" class="btn-ghost" data-research-starter-keep>${tr(["保留原草稿", "Keep original draft"])}</button><button type="button" class="btn-primary" data-research-starter-replace>${tr(["替换草稿", "Replace draft"])}</button></div>` : `<button type="submit" class="btn-primary">${tr(["填入任务", "Prepare task"])}</button>`}
    </form>` : ""}</section>`;
}

export function bindResearchStarters(rerender: () => void, prepare: (prompt: string, replace: boolean) => boolean): void {
  const finish = () => {
    selectedScene = ""; error = ""; pendingPrompt = "";
    rerender();
    document.getElementById("composer-input")?.focus();
  };
  document.querySelectorAll<HTMLButtonElement>("[data-research-scene]").forEach(button => {
    button.addEventListener("click", () => {
      selectedScene = button.dataset.researchScene || "";
      selectedAction = ""; count = "3"; error = ""; pendingPrompt = "";
      rerender();
      document.querySelector<HTMLElement>("[data-research-starter-form] select")?.focus();
    });
  });
  document.querySelector("[data-research-starter-close]")?.addEventListener("click", () => { selectedScene = ""; error = ""; pendingPrompt = ""; rerender(); });
  const form = document.querySelector<HTMLFormElement>("[data-research-starter-form]");
  if (!form) return;
  form.querySelector("[data-research-starter-keep]")?.addEventListener("click", () => {
    pendingPrompt = ""; rerender();
    document.querySelector<HTMLElement>("[data-research-starter-form] textarea")?.focus();
  });
  form.querySelector("[data-research-starter-replace]")?.addEventListener("click", () => {
    if (pendingPrompt && prepare(pendingPrompt, true)) finish();
  });
  form.addEventListener("input", event => {
    const data = new FormData(form);
    target = String(data.get("target") || ""); context = String(data.get("context") || ""); count = String(data.get("count") || "");
    if (pendingPrompt) {
      const field = event.target as HTMLInputElement | HTMLTextAreaElement;
      const name = field.name;
      const start = field.selectionStart, end = field.selectionEnd;
      pendingPrompt = ""; rerender();
      const next = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[data-research-starter-form] [name="${name}"]`);
      next?.focus();
      if (next && typeof start === "number" && typeof end === "number") next.setSelectionRange(start, end);
    }
  });
  form.querySelector("select")?.addEventListener("change", () => {
    selectedAction = String(new FormData(form).get("action") || ""); count = selectedAction === "comment-leads" ? "100" : "3"; error = ""; pendingPrompt = ""; rerender();
  });
  form.addEventListener("submit", event => {
    event.preventDefault();
    if (!form.reportValidity()) return;
    const data = new FormData(form);
    target = String(data.get("target") || "");
    context = String(data.get("context") || "");
    count = String(data.get("count") || "");
    const scene = researchScenes.find(item => item.id === selectedScene);
    const action = scene?.actions.find(item => item.id === String(data.get("action")));
    if (!action) return;
    const trimmed = target.trim();
    const amount = Number(count);
    let valid = trimmed.length > 0 && trimmed.length <= 1500 && context.length <= 500 && Number.isInteger(amount) && amount >= 1 && amount <= (action.comments ? 1000 : 20);
    if (valid && action.urls) {
      const urls = trimmed.split(/\s+/);
      valid = urls.length <= 20 && urls.every(value => {
        try {
          const url = new URL(value);
          return url.protocol === "https:" && !url.username && !url.password && ["xiaohongshu.com", "xhslink.com", "douyin.com", "iesdouyin.com", "tiktok.com"].some(host => url.hostname === host || url.hostname.endsWith("." + host));
        } catch { return false; }
      });
    }
    if (!valid) { error = tr(["请填写有效的主题或小红书、抖音、TikTok HTTPS 原链接，并检查数量。", "Enter a valid topic or Xiaohongshu, Douyin or TikTok HTTPS URL and check the quantity."]); rerender(); return; }
    const prompt = getLanguage() === "zh"
      ? `任务：${tr(action.label)}\n研究对象：${JSON.stringify(trimmed)}\n范围：${action.comments ? `期望读取${amount}条评论` : `最多读取${amount}篇作品`}。\n${tr(action.goal)}\n我的目标和资料：${JSON.stringify(context.trim())}\n交付要求：实际执行采集，保留原始链接、作者、采集时间和样本范围，每项判断可回溯证据。缺少登录、正文、评论、粉丝或转写时明确标注未取得及原因，不生成替代证据。遇到登录或验证码保留当前任务等待接管，不反复重试。附件必须真实可打开或下载；不要把服务端路径当成可下载文件。先完成本轮，不循环扩样。`
      : `Task: ${tr(action.label)}\nResearch target: ${JSON.stringify(trimmed)}\nScope: ${action.comments ? `${amount} requested comments` : `at most ${amount} posts`}.\n${tr(action.goal)}\nMy goal and own facts: ${JSON.stringify(context.trim())}\nDelivery: actually collect evidence with original URLs, authors, capture time and sample scope. Make claims traceable. Label unavailable login, text, comments, follower counts or transcripts and explain why; do not invent evidence. Pause the same task for login or CAPTCHA rather than repeatedly retrying. Deliver real, accessible files, not server-only paths. Complete one round without expanding the sample repeatedly.`;
    if (prepare(prompt, false)) {
      finish();
    } else {
      pendingPrompt = prompt; error = ""; rerender();
      document.querySelector<HTMLElement>("[data-research-starter-keep]")?.focus();
    }
  });
}
