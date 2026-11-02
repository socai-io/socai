import { esc } from "./html";
import { getLanguage } from "./i18n";
import "./task-examples.css";

type Platform = "xhs" | "dy";
export interface TaskExample { id: string; platform: Platform; title: string; prompts: string[] }
const opened = new Set<string>();
const entries = [
  ["content-workflow", "内容工作流", "Content workflow",
    "围绕咖啡机为新手用户规划内容。先采集并比较真实样本，给出5个有来源的选题，等我选题后再写原创稿。",
    "围绕租房收纳，比较近期内容和评论中的问题，给出选题、标题及结构建议；先让我选择方向，不要直接发布。",
    "Plan coffee-machine content for beginners. Collect and compare real examples, propose five source-linked topics, then wait for my choice before drafting.",
    "Research rental-home storage posts and audience questions. Suggest topics, titles and outlines, then let me choose before writing. Do not publish."],
  ["keyword-insights", "关键词洞察", "Keyword insights",
    "从“咖啡机”出发读取真实搜索建议和可获取的长尾词，去重分组，说明搜索样本里的需求和竞争；不要编造搜索量。",
    "围绕“租房收纳”找真实长尾问题，并为优先的3个方向各采集代表内容，输出关键词表和原始链接。",
    "Read actual search suggestions and available long-tail keywords for coffee machines. Deduplicate and group them, assess sampled demand and competition, and do not invent search volumes.",
    "Find real long-tail queries about rental-home storage. Collect representative posts for three priority directions and deliver a keyword table with sources."],
  ["viral-analysis", "爆款分析", "Viral content analysis",
    "搜索最近30天的咖啡机内容，比较高、中、低互动样本，拆解标题、开头、视觉和评论，优先寻找低粉高互动案例，附来源和筛选口径。",
    "我想拆解一篇高互动作品。先问我要链接，再结合账号基线、内容结构和评论反馈分析可借鉴的做法，不把相关性当因果。",
    "Compare high-, medium- and low-engagement coffee-machine posts from the last 30 days. Analyze titles, openings, visuals and comments, prioritizing smaller creators with strong engagement. Include sources and selection criteria.",
    "Help analyze a high-engagement post. Ask me for its link, then compare the creator baseline, structure and audience feedback without claiming causation."],
  ["account-analysis", "账号分析", "Account analysis",
    "围绕咖啡机找到3个值得研究的账号，比较定位、主题、标题或开头、内容形式、发布节奏和公开互动，给出来源与样本限制。",
    "我想比较两个对标账号。先问我要主页链接和目标，再读取各自近期作品，输出差异表和可实践的内容建议。",
    "Find three coffee-machine creators worth studying. Compare positioning, themes, titles or hooks, format, posting cadence and public engagement, with sources and sampling limits.",
    "Help compare two benchmark creators. Ask for their profile links and my objective, then read recent posts and deliver a comparison with actionable content ideas."],
  ["collection", "作品采集", "Post collection",
    "搜索“咖啡机”，采集20条去重作品，保留标题、作者、原始链接、发布时间和可获取的互动指标，导出CSV；不足20条时说明实际覆盖。",
    "我想采集一个账号的近期作品。先问我要主页链接、时间范围和数量，再按作品ID去重，保留来源并交付可下载表格。",
    "Search for coffee machines and collect 20 unique posts with titles, authors, original links, publication dates and available engagement metrics. Export CSV and report actual coverage if fewer are available.",
    "Help collect a creator's recent posts. Ask for the profile link, date range and quantity, then deduplicate by post ID and deliver a downloadable table with sources."],
  ["comment-collection", "评论采集", "Comment collection",
    "先让我提供作品链接，再采集一级评论和可获取的回复；分别记录层级、作者、正文、时间和来源，导出CSV并说明停止原因，不把已加载当全部。",
    "找3篇咖啡机讨论作品，采集各篇可获取的评论及回复，去重后按作品整理，说明页面显示总数、实际读取数和未完成原因。",
    "Ask for a post link, then collect top-level comments and available replies, keeping hierarchy, authors, text, timestamps and sources. Export CSV and explain stop reasons; loaded does not mean complete.",
    "Find three coffee-machine discussions and collect available comments and replies. Deduplicate by post and report displayed totals, collected counts and any incomplete coverage."],
  ["media-download", "无水印下载", "Original media download",
    "我想保存有权使用的作品素材。先问我要作品链接，再下载可获取的原始图片或视频，交付可打开的文件和清单，明确水印及失败情况。",
    "整理我提供的3个作品链接的原始素材，按作品和图片顺序命名，给出文件、来源及失败列表；不要只返回媒体URL。",
    "Ask me for a post link and download the available original images or video that I am entitled to use. Deliver usable files and a manifest, stating watermark and failure limitations.",
    "Organize original media from three post links I provide. Name files by post and image order, deliver files and sources, and list failures instead of only returning media URLs."],
  ["image-text", "图文文案提取", "Image text extraction",
    "先问我要一篇图文作品链接，再按封面和全部配图的顺序做OCR，逐图标注提取结果和失败项；不要把正文当图片文字。",
    "提取我提供的教程图文里的步骤和商品信息，保留逐图原文、图片序号和来源，另附整理版；识别不出的内容明确标注。",
    "Ask me for an image-post link, then OCR the cover and every available image in order. Label results and failures per image; do not substitute the post caption for image text.",
    "Extract tutorial steps and product details from an image post I provide. Preserve per-image text, image order and sources, with a separate cleaned version and unreadable sections marked."],
  ["video-transcript", "视频逐字稿", "Video transcript",
    "先问我要视频作品链接，再从真实音频提取带时间戳的分句逐字稿，另附内容结构和摘要；无音轨或ASR不可用时明确说明。",
    "对我提供的两条视频生成逐字稿，对比开头钩子、口播结构和节奏；时间戳必须来自实际转写结果，不要用摘要冒充逐字稿。",
    "Ask me for a video link, then transcribe its actual audio into timestamped segments and provide a separate structure analysis and summary. State when audio or ASR is unavailable.",
    "Transcribe two videos I provide and compare hooks, spoken structure and pacing. Use real transcription timestamps, not a summary presented as a verbatim transcript."],
  ["comment-leads", "评论区截流", "Comment intent analysis",
    "研究咖啡机相关作品的公开评论，识别明确求推荐、询价和选购问题，列出原文、来源与判断依据，供我人工跟进，不要自动评论或私信。",
    "先问我要作品链接和产品信息，再从评论里整理高意向问题与回复草稿，区分求助、抱怨和购买意向，发送前逐条让我确认。",
    "Analyze public coffee-machine comments for explicit recommendation requests, pricing questions and purchase intent. Preserve quotes, sources and reasoning for manual review; do not comment or message automatically.",
    "Ask for post links and product facts, then identify high-intent questions and draft replies. Distinguish help requests, complaints and purchase intent, and ask for approval before sending each reply."],
  ["account-monitoring", "账号监控", "Account monitoring",
    "我想每天监控对标账号更新。先问我要账号链接、时区、执行时间和提醒方式，确认后创建真实定时任务，读回配置并说明暂停方法。",
    "为我提供的账号建立一次内容基线，再按确认的时间跟踪新增作品和互动变化；按作品ID去重，提供历史结果和来源，不能只写监控计划。",
    "Help monitor creator updates daily. Ask for profile links, timezone, execution time and notification preferences, then confirm before creating a real schedule. Read back its settings and explain how to pause it.",
    "Create a baseline for creator profiles I provide, then track new posts and engagement changes on an agreed schedule. Deduplicate by post ID and retain source-linked history rather than just writing a monitoring plan."],
] as const;

export function taskExamples(platform: Platform, chinese = getLanguage() === "zh"): TaskExample[] {
  const name = platform === "xhs" ? (chinese ? "小红书" : "RedNote") : (chinese ? "抖音" : "Douyin");
  return entries.map(([key, zh, en, a, b, c, d]) => {
    const suffix = key === "collection" ? (platform === "xhs" ? "note-collection" : "video-collection") : key;
    let title: string = zh;
    if (platform === "xhs" && key === "viral-analysis") title = "爆款笔记分析";
    if (platform === "dy" && key === "viral-analysis") title = "爆款视频分析";
    if (platform === "xhs" && key === "account-analysis") title = "对标账号分析";
    if (key === "collection") title = platform === "xhs" ? "笔记采集" : "视频采集";
    return {
      id: `${platform}-${suffix}`, platform,
      title: `${name} ${chinese ? title : en}`,
      prompts: (chinese ? [a, b] : [c, d]).map(text => chinese
        ? `请使用${name}的${title}能力。${text}`
        : `Use ${name} ${en.toLowerCase()}. ${text}`),
    };
  });
}

export function renderTaskExamples(): string {
  const chinese = getLanguage() === "zh";
  return `<section class="task-examples" data-task-examples aria-label="${chinese ? "任务示例" : "Task examples"}">
    <p class="t-small subtle">${chinese ? "从一个例子开始，填入后可以继续修改" : "Start with an example, then make it your own"}</p>
    ${(["xhs", "dy"] as const).map(platform => `<details data-example-group="${platform}" ${opened.has(platform) ? "open" : ""}>
      <summary>${platform === "xhs" ? (chinese ? "小红书" : "RedNote") : (chinese ? "抖音" : "Douyin")}<span class="task-examples__count">11</span></summary>
      <div class="task-examples__list">${taskExamples(platform, chinese).map(example => `<details data-example-group="${example.id}" ${opened.has(example.id) ? "open" : ""}>
        <summary>${esc(example.title)}</summary>
        <div class="task-examples__prompts">${example.prompts.map((prompt, index) => `<button type="button" data-task-example="${example.id}" data-example-platform="${platform}" data-example-index="${index}">${esc(prompt)}</button>`).join("")}</div>
      </details>`).join("")}</div>
    </details>`).join("")}
  </section>`;
}

export function bindTaskExamples(select: (example: { platform: Platform; prompt: string }) => void): void {
  document.querySelectorAll<HTMLDetailsElement>("[data-example-group]").forEach(details => {
    details.addEventListener("toggle", () => {
      const key = details.dataset.exampleGroup!;
      if (details.open) opened.add(key); else opened.delete(key);
    });
  });
  document.querySelectorAll<HTMLButtonElement>("[data-task-example]").forEach(button => {
    button.addEventListener("click", () => {
      const platform = button.dataset.examplePlatform;
      if (platform !== "xhs" && platform !== "dy") return;
      const example = taskExamples(platform).find(item => item.id === button.dataset.taskExample);
      const prompt = example?.prompts[Number(button.dataset.exampleIndex)];
      if (prompt) select({ platform, prompt });
    });
  });
}
