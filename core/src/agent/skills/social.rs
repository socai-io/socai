//! On-demand task methods, not declarations of tool availability.
pub(super) struct SocialSkill {
    pub name: &'static str,
    pub description: &'static str,
    pub document: &'static str,
}

pub(super) const SKILLS: &[SocialSkill] = &[
    SocialSkill {
        name: "xhs-content-workflow",
        description: "小红书 内容工作流. Start from a short topic or reference post, collect evidence, offer topic choices, then draft after selection.",
        document: include_str!("xhs-content-workflow/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-keyword-insights",
        description: "小红书 关键词洞察. Research actual search suggestions, topic demand and benchmark candidates without inventing search volume.",
        document: include_str!("xhs-keyword-insights/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-viral-analysis",
        description: "小红书 爆款分析. Compare high-performing and ordinary posts with cited hooks, structure, visuals and comment evidence.",
        document: include_str!("xhs-viral-analysis/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-account-analysis",
        description: "小红书 账号分析. Analyze a public account using stratified post samples, positioning, themes and reusable content patterns.",
        document: include_str!("xhs-account-analysis/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-note-collection",
        description: "小红书 作品采集. Collect public search, profile or supplied-post records with stable IDs, detail enrichment and honest coverage.",
        document: include_str!("xhs-note-collection/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-comment-collection",
        description: "小红书 评论采集. Collect distinct top-level comments and replies with parent links, pagination coverage and partial reasons.",
        document: include_str!("xhs-comment-collection/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-media-download",
        description: "小红书 素材下载. Save available original images and video with provenance, verified files and no false watermark-removal promise.",
        document: include_str!("xhs-media-download/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-image-text",
        description: "小红书 图文文案提取. Extract image text in order with per-image coverage, source links and explicit OCR failures.",
        document: include_str!("xhs-image-text/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-video-transcript",
        description: "小红书 视频逐字稿. Transcribe actual video audio, preserve genuine time segments and distinguish captions from descriptions.",
        document: include_str!("xhs-video-transcript/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-comment-leads",
        description: "小红书 评论意向分析. Identify public intent signals from comments and prepare reviewable outreach drafts without unsolicited sends.",
        document: include_str!("xhs-comment-leads/SKILL.md"),
    },
    SocialSkill {
        name: "xhs-account-monitoring",
        description: "小红书 账号监控. Baseline and compare account updates; activate recurring monitoring only through a real authorized scheduler.",
        document: include_str!("xhs-account-monitoring/SKILL.md"),
    },
    SocialSkill {
        name: "dy-content-workflow",
        description: "抖音 内容工作流. Start from a short topic or reference post, collect evidence, offer topic choices, then draft after selection.",
        document: include_str!("dy-content-workflow/SKILL.md"),
    },
    SocialSkill {
        name: "dy-keyword-insights",
        description: "抖音 关键词洞察. Research actual search suggestions, topic demand and benchmark candidates without inventing search volume.",
        document: include_str!("dy-keyword-insights/SKILL.md"),
    },
    SocialSkill {
        name: "dy-viral-analysis",
        description: "抖音 爆款分析. Compare high-performing and ordinary posts with cited hooks, structure, visuals and comment evidence.",
        document: include_str!("dy-viral-analysis/SKILL.md"),
    },
    SocialSkill {
        name: "dy-account-analysis",
        description: "抖音 账号分析. Analyze a public account using stratified post samples, positioning, themes and reusable content patterns.",
        document: include_str!("dy-account-analysis/SKILL.md"),
    },
    SocialSkill {
        name: "dy-video-collection",
        description: "抖音 作品采集. Collect public search, profile or supplied-post records with stable IDs, detail enrichment and honest coverage.",
        document: include_str!("dy-video-collection/SKILL.md"),
    },
    SocialSkill {
        name: "dy-comment-collection",
        description: "抖音 评论采集. Collect distinct top-level comments and replies with parent links, pagination coverage and partial reasons.",
        document: include_str!("dy-comment-collection/SKILL.md"),
    },
    SocialSkill {
        name: "dy-media-download",
        description: "抖音 素材下载. Save available original images and video with provenance, verified files and no false watermark-removal promise.",
        document: include_str!("dy-media-download/SKILL.md"),
    },
    SocialSkill {
        name: "dy-image-text",
        description: "抖音 图文文案提取. Extract image text in order with per-image coverage, source links and explicit OCR failures.",
        document: include_str!("dy-image-text/SKILL.md"),
    },
    SocialSkill {
        name: "dy-video-transcript",
        description: "抖音 视频逐字稿. Transcribe actual video audio, preserve genuine time segments and distinguish captions from descriptions.",
        document: include_str!("dy-video-transcript/SKILL.md"),
    },
    SocialSkill {
        name: "dy-comment-leads",
        description: "抖音 评论意向分析. Identify public intent signals from comments and prepare reviewable outreach drafts without unsolicited sends.",
        document: include_str!("dy-comment-leads/SKILL.md"),
    },
    SocialSkill {
        name: "dy-account-monitoring",
        description: "抖音 账号监控. Baseline and compare account updates; activate recurring monitoring only through a real authorized scheduler.",
        document: include_str!("dy-account-monitoring/SKILL.md"),
    },
];

pub(super) fn catalog_prompt() -> String {
    let mut catalog = String::from("\nSocial content task skills (load the matching method with read_skill before the corresponding collection or analysis; the method does not enable unavailable tools):\n");
    for skill in SKILLS {
        catalog.push_str(&format!("- `{}` — {}\n", skill.name, skill.description));
    }
    catalog.push_str("Select only methods relevant to the user's current request. read_site_skills describes page tools, not these task methods. Loading a method does not prove a download, OCR, transcript, send, or scheduled monitor succeeded; verify the actual tools and user-visible result.\n");
    catalog.push_str("For Xiaohongshu or Douyin content ideas, topic selection, or writing (including a short topic and 'not drafting yet'), load `xhs-content-workflow` or `dy-content-workflow` first, before collecting. Keyword insights can supplement that workflow, not replace it. Do not load these workflows for unrelated tasks or an explicit keyword-only request. For text-only topic research, pass `download_media:false` when supported unless media download, visual analysis, OCR, or transcription is actually needed; retain source links and state the real collection scope.\n");
    catalog
}
