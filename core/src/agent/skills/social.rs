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

/// Preparation hints only: this never enables a site or proves source identity.
/// Unrelated requests still use the ordinary on-demand catalogue.
pub(super) fn initial_skills(task: &str, enabled_sites: &[String]) -> Vec<String> {
    let text = task.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| text.contains(word));
    let explicit_xhs = has(&["小红书", "xiaohongshu", "xhslink", "rednote", "xhs-"]);
    let explicit_dy = has(&["抖音", "douyin", "dy-"]);
    let explicit_platform = explicit_xhs || explicit_dy;
    let archived = has(&[
        "已归档",
        "归档的",
        "已采集",
        "之前采集",
        "上一轮",
        "上次采集",
    ]);
    let social_intent = has(&["选题", "爆款", "笔记", "博主", "达人", "评论区", "账号监控"]);
    let selected = |site: &str| enabled_sites.iter().any(|name| name == site);
    let single_social_site = enabled_sites.len() == 1;
    let xhs = if explicit_platform {
        explicit_xhs
    } else {
        selected("xhs") && (single_social_site || archived || social_intent)
    };
    let dy = if explicit_platform {
        explicit_dy
    } else {
        selected("douyin") && (single_social_site || archived || social_intent)
    };
    if !xhs && !dy {
        return Vec::new();
    }

    let image_text = has(&["ocr", "图文文案提取", "图文提取", "图片文字"])
        || (has(&["配图", "图片", "逐图", "图集"])
            && has(&["提取", "识别", "文字", "extract", "recognize"]));
    let transcript = has(&["逐字稿", "转录", "transcript", "视频文案提取"])
        || (has(&["视频", "音频"]) && has(&["转写", "字幕", "转文字"]));
    let comments = has(&["评论", "comments", "replies"]);
    let leads = comments && has(&["截流", "意向", "获客", "线索", "触达", "引流"]);
    let monitoring = has(&["监控", "monitor"])
        || (has(&["账号", "博主", "达人", "主页"])
            && has(&["定时", "每天", "追踪更新", "新增作品"]));
    let media = has(&["下载", "download", "无水印"])
        && has(&["配图", "图片", "图集", "视频", "素材", "原图", "media"]);
    let account = has(&["账号", "博主", "达人", "主页", "account"])
        && has(&["分析", "对标", "拆解", "定位", "analy"]);
    let viral = has(&["爆款", "viral"]) && has(&["分析", "拆解", "研究", "analy"]);
    let keyword = has(&["关键词", "搜索词", "下拉词", "联想词", "keyword"]);
    // OCR/transcription wording can include 文案; that alone is not a new draft.
    let keyword_only = has(&[
        "只要关键词",
        "仅关键词",
        "只做关键词",
        "keyword only",
        "keyword-only",
    ]);
    let workflow = !keyword_only
        && (has(&["内容工作流", "选题", "灵感"])
            || (!has(&["不要写", "先不写", "不写稿", "暂不写"])
                && (has(&["写稿", "初稿", "成稿", "创作"])
                    || (!image_text && !transcript && has(&["文案", "写一篇", "写笔记"])))));
    let collection = has(&["采集", "抓取", "收集", "导出", "保存", "整理", "collect"])
        && has(&["帖子", "笔记", "作品", "视频", "链接", "posts"])
        && !image_text
        && !transcript
        && !comments
        && !monitoring;
    let methods = [
        (image_text, "image-text"),
        (transcript, "video-transcript"),
        (leads, "comment-leads"),
        (monitoring, "account-monitoring"),
        (comments && !leads, "comment-collection"),
        (media, "media-download"),
        (account && !monitoring, "account-analysis"),
        (viral, "viral-analysis"),
        (workflow, "content-workflow"),
        (keyword, "keyword-insights"),
        (collection, "collection"),
    ];
    let mut names = Vec::new();
    for (active, prefix) in [(xhs, "xhs"), (dy, "dy")] {
        if !active {
            continue;
        }
        for (matching, method) in methods {
            let suffix = if method == "collection" {
                if prefix == "xhs" {
                    "note-collection"
                } else {
                    "video-collection"
                }
            } else {
                method
            };
            let name = format!("{prefix}-{suffix}");
            if matching || text.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

pub(super) fn catalog_prompt() -> String {
    let mut catalog = String::from("\nSocial content task skills (load the matching method with read_skill before the corresponding collection or analysis; the method does not enable unavailable tools):\n");
    for skill in SKILLS {
        catalog.push_str(&format!("- `{}` — {}\n", skill.name, skill.description));
    }
    catalog.push_str("Select only methods relevant to the user's current request. read_site_skills describes page tools, not these task methods. Loading a method does not prove a download, OCR, transcript, send, or scheduled monitor succeeded; verify the actual tools and user-visible result.\n");
    catalog.push_str("For Xiaohongshu or Douyin content ideas, topic selection, or writing (including a short topic and 'not drafting yet'), load `xhs-content-workflow` or `dy-content-workflow` first, before collecting. Keyword insights can supplement that workflow, not replace it. Do not load these workflows for unrelated tasks or an explicit keyword-only request. For text-only topic research, pass `download_media:false` when supported unless media download, visual analysis, OCR, or transcription is actually needed; retain source links and state the real collection scope.\n");
    catalog
}
