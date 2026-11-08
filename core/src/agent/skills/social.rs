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
    // The hosted attachment index is reference metadata, not another user
    // request. An old attachment's OCR fields or filename must not choose this
    // turn's preparation method. This changes hints, never tool permissions.
    let task = task
        .split_once("\n\n[Server attachment index: metadata only,")
        .map_or(task, |(request, _)| request);
    // Host delivery/navigation guidance is not another collection request.
    // Keep it in the model prompt, but do not let "extracted", "save" or
    // "comments" in that guidance select unrelated preparatory methods.
    let task = task
        .split_once("\n\n[Web creation delivery contract]\n")
        .map_or(task, |(request, _)| request);
    let task = task
        .split_once("\n\nServer-verified read targets (navigation metadata,")
        .map_or(task, |(request, _)| request);
    // A confirmed brief describes what to write, including quoted research and
    // explicit prohibitions. Do not treat those mentions as collection orders:
    // e.g. "不要…OCR、素材下载或音频转写" used to preload all three methods.
    // Parse the existing envelope, rather than keying on an arbitrary "初稿"
    // substring. The data remain untrusted and grant no collection/write access.
    let confirmed_brief = task
        .strip_prefix("根据以下用户已确认的创作简报完成初稿。")
        .and_then(|rest| rest.rsplit_once("\n已确认简报：\n"))
        .and_then(|(_, raw)| serde_json::from_str::<serde_json::Value>(raw).ok())
        .is_some_and(|brief| {
            matches!(brief["medium"].as_str(), Some("image_text" | "video"))
                && brief["title"].is_string()
                && brief["core_judgment"].is_string()
                && brief["outline"].is_array()
                && brief["topic"]["source_run_id"].is_string()
        });
    if confirmed_brief {
        return ["xhs", "douyin"]
            .into_iter()
            .filter(|site| enabled_sites.iter().any(|enabled| enabled == site))
            .map(|site| {
                format!(
                    "{}-content-workflow",
                    if site == "xhs" { "xhs" } else { "dy" }
                )
            })
            .collect();
    }
    // Preparation is only a hint, not a permission gate. Match affirmative
    // request text, not a forbidden-work list such as "不要OCR、评论或转写".
    // Keep comma/顿号 lists together so a negation applies to the whole list;
    // an explicit contrast starts another clause and can request a different
    // action. The model still receives the complete, unchanged user request.
    let request = task
        .to_ascii_lowercase()
        .replace("但是", "。")
        .replace("不过", "。")
        .replace("而是", "。")
        .replace('但', "。")
        .replace(" but ", ".");
    let text = request
        .split(['。', '.', '！', '!', '？', '?', '；', ';', '\n'])
        .map(|clause| {
            let end = [
                "不要",
                "不需要",
                "不用",
                "无需",
                "禁止",
                "别再",
                "先不",
                "暂不",
                "不做",
                "不重新",
                "不再",
                "do not ",
                "don't ",
                "without ",
                "no ",
            ]
            .iter()
            .filter_map(|marker| {
                clause.match_indices(marker).find_map(|(index, _)| {
                    // English "no " must not match a word ending, e.g. piano.
                    (!marker.is_ascii()
                        || index == 0
                        || !clause.as_bytes()[index - 1].is_ascii_alphanumeric())
                    .then_some(index)
                })
            })
            .min()
            .unwrap_or(clause.len());
            &clause[..end]
        })
        .collect::<Vec<_>>()
        .join(" ");
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
    let leads = comments
        && has(&[
            "截流",
            "意向",
            "获客",
            "线索",
            "触达",
            "引流",
            "回复草稿",
            "回复建议",
            "建议回复",
            "reply draft",
            "draft replies",
        ]);
    let monitoring = has(&["监控", "monitor"])
        || (has(&["账号", "博主", "达人", "主页"])
            && has(&["定时", "每天", "追踪更新", "新增作品"]));
    // A downloadable transcript/OCR document is not a request for source media.
    // Those methods already handle their real input; additional media methods
    // remain available on demand for explicitly requested source downloads.
    let media = !image_text
        && !transcript
        && has(&["下载", "保存", "download", "save", "无水印"])
        && has(&["配图", "图片", "图集", "视频", "素材", "原图", "media"]);
    let account = has(&["账号", "博主", "达人", "主页", "account"])
        && has(&[
            "分析", "对标", "对比", "比较", "对照", "拆解", "定位", "analy", "compar",
        ]);
    // Users can ask for a high/medium/low comparison without knowing the
    // onboarding label. This selects a method only, never new collection.
    let stratified = has(&["高中低", "高、中、低", "高/中/低", "分档", "分层"])
        && has(&["点赞", "赞数", "互动"]);
    let viral = (has(&["爆款", "viral"]) || stratified)
        && has(&[
            "分析", "拆解", "研究", "比较", "对比", "对照", "差异", "analy", "compar",
        ]);
    let keyword = has(&["关键词", "搜索词", "下拉词", "联想词", "keyword"]);
    // OCR/transcription wording can include 文案; that alone is not a new draft.
    let keyword_only = has(&[
        "只要关键词",
        "仅关键词",
        "只做关键词",
        "keyword only",
        "keyword-only",
    ]);
    // A short revision request is still writing: users need not repeat the
    // onboarding label (e.g. 初稿/创作) to retain the same fact/quote boundaries.
    // Require both a draft target and an editing action; merely mentioning
    // source正文, OCR or a transcript must not turn collection into drafting.
    let editing = !image_text
        && !transcript
        && !has(&[
            "不要改稿",
            "不用改稿",
            "不改正文",
            "不要修改正文",
            "不要重写正文",
            "不要润色",
            "不需要改写",
            "暂不改稿",
            "先不改稿",
        ])
        && (has(&["改稿", "润色稿", "精简稿"])
            || (has(&["正文", "稿件", "这稿", "这篇稿"])
                && has(&[
                    "精简", "润色", "删掉", "删去", "改短", "缩短", "重写", "改写", "修改",
                ])));
    let workflow = !keyword_only
        && (has(&["内容工作流", "选题", "灵感"])
            || (!has(&["不要写", "先不写", "不写稿", "暂不写"])
                && (has(&["写稿", "初稿", "成稿", "创作"])
                    || editing
                    || (!image_text && !transcript && has(&["文案", "写一篇", "写笔记"])))));
    let collection = has(&["采集", "抓取", "收集", "导出", "保存", "整理", "collect"])
        && has(&["帖子", "笔记", "作品", "视频", "链接", "posts"])
        && !image_text
        && !transcript
        && !comments
        && !media
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
