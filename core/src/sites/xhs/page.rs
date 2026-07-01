use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::cdp::PageSession;
use crate::media::MediaProcessor;
use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::sites::xhs::entities::{normalize_url, XhsNote, XhsNoteCard};

pub const XHS_HOME_URL: &str = "https://www.xiaohongshu.com/explore";

const PAGE_SCRIPTS_JS: &str = include_str!("page_scripts.js");

/// How long to wait for the search-results page to actually populate cards
/// after submitting. The wait polls and returns the instant cards appear, so a
/// generous ceiling only costs time on genuinely slow loads (e.g. over a VPN) —
/// the fast path is unaffected.
const SEARCH_TRANSITION_TIMEOUT_S: f64 = 12.0;

const XHS_PAGE_SCRIPT_FUNCTIONS: &[&str] = &[
    "note",
    "noteWithWait",
    "pageState",
    "searchCards",
    "searchInput",
    "setSearchInput",
    "searchState",
    "searchFilterTrigger",
    "searchFilters",
    "clickCard",
    "closeNote",
    "noteOpen",
    "comments",
    "commentsWithWait",
    "commentAreaState",
    "expandCommentReplies",
    "scrollFeed",
    "scrollInNote",
    "carouselImages",
    "profileInfo",
    "profileCards",
];

/// Single source of truth for the XHS search-filter vocabulary: canonical group
/// `key`, the group's visible Chinese `title` (used to join against the DOM the
/// page script reads), and the allowed option labels in panel order. The page
/// script no longer carries this vocabulary — it just reports whatever tags it
/// sees by title — so this table is the only place to keep in sync.
pub(crate) const XHS_SEARCH_FILTERS: &[(&str, &str, &[&str])] = &[
    (
        "sort",
        "排序依据",
        &["综合", "最新", "最多点赞", "最多评论", "最多收藏"],
    ),
    ("note_type", "笔记类型", &["不限", "视频", "图文"]),
    (
        "publish_time",
        "发布时间",
        &["不限", "一天内", "一周内", "半年内"],
    ),
    (
        "search_scope",
        "搜索范围",
        &["不限", "已看过", "未看过", "已关注"],
    ),
    ("distance", "位置距离", &["不限", "同城", "附近"]),
];

#[derive(Debug, Clone)]
pub struct ReadNoteOptions {
    pub level: String,
    pub include_media: bool,
    pub download_media: bool,
    /// OCR each downloaded image and attach `ocr_text` per image. Implies
    /// `download_media` (the caller forces it on), since OCR reads the saved
    /// files. Ignored for video notes.
    pub ocr: bool,
    pub max_images: usize,
    pub max_video_frames: usize,
    pub poster_url_fallback: String,
    pub note_id_fallback: String,
}

impl Default for ReadNoteOptions {
    fn default() -> Self {
        Self {
            level: "lite".into(),
            include_media: false,
            download_media: false,
            ocr: false,
            max_images: 12,
            max_video_frames: 4,
            poster_url_fallback: String::new(),
            note_id_fallback: String::new(),
        }
    }
}

/// Site-aware XHS operations on top of a CDP `PageSession`.
pub struct XhsPageRuntime<'a> {
    page: &'a PageSession,
    media: Option<MediaProcessor>,
    last_extracted_note_id: Mutex<String>,
}

impl<'a> XhsPageRuntime<'a> {
    pub fn new(page: &'a PageSession) -> Self {
        Self {
            page,
            media: None,
            last_extracted_note_id: Mutex::new(String::new()),
        }
    }

    pub fn new_with_media(page: &'a PageSession, media: Option<MediaProcessor>) -> Self {
        Self {
            page,
            media,
            last_extracted_note_id: Mutex::new(String::new()),
        }
    }

    fn set_last_note_id(&self, value: String) {
        if let Ok(mut guard) = self.last_extracted_note_id.lock() {
            *guard = value;
        }
    }

    fn last_note_id(&self) -> String {
        self.last_extracted_note_id
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Inject `page_scripts.js` (the IIFE that defines `SocaiXhsPageScripts`)
    /// and call one of its functions.
    pub async fn run_script(&self, name: &str, arg: Option<&Value>) -> Result<Value> {
        if !XHS_PAGE_SCRIPT_FUNCTIONS.contains(&name) {
            anyhow::bail!("Unknown XHS page script: {name}");
        }
        let args = match arg {
            None => String::new(),
            Some(v) => serde_json::to_string(v)?,
        };
        let expr = format!(
            "{PAGE_SCRIPTS_JS}\n// SOCAI_XHS_CALL: {name}\nreturn SocaiXhsPageScripts.{name}({args});"
        );
        self.page.evaluate_json(&expr).await
    }

    pub async fn current_url(&self) -> Result<String> {
        Ok(self
            .page
            .page_info()
            .await?
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    pub async fn ensure_xhs(&self, navigate_if_needed: bool) -> Result<()> {
        let url = self.current_url().await?;
        if url.contains("xiaohongshu.com") {
            return Ok(());
        }
        if navigate_if_needed {
            self.page.navigate(XHS_HOME_URL).await?;
            return Ok(());
        }
        anyhow::bail!(
            "Current page is not Xiaohongshu: {}",
            if url.is_empty() { "unknown" } else { &url }
        );
    }

    pub async fn detect_state(&self) -> Result<Value> {
        self.ensure_xhs(false).await?;
        self.expect_object("pageState", None).await
    }

    pub async fn search_notes(
        &self,
        query: &str,
        filters: Option<&Value>,
        wait_seconds: f64,
        num_notes: Option<usize>,
    ) -> Result<Value> {
        let keyword = query.trim();
        if keyword.is_empty() {
            anyhow::bail!("query is required");
        }

        self.ensure_xhs(true).await?;
        let submit = self.submit_search(keyword, wait_seconds).await?;
        let ok = script_ok(&submit);
        // Apply any search-result filters before reading cards, so the returned
        // page reflects the filtered feed.
        let mut filter_result = Value::Object(Map::new());
        if ok {
            if let Some(filters) = filters {
                filter_result = self.apply_search_filters(filters, wait_seconds).await?;
            }
        }
        let cards = if ok {
            match num_notes {
                // Scroll the feed to lazy-load more cards until we reach the
                // requested count (or the feed stops growing). `None`/`Some(0)`
                // keeps the cheap first-page-only behaviour.
                Some(target) if target > 0 => self.collect_search_cards(target).await?,
                _ => self.extract_search_cards().await?,
            }
        } else {
            Vec::new()
        };
        let mut submit = submit;
        if let Some(state) = submit.get_mut("state").and_then(Value::as_object_mut) {
            state.remove("url_keyword");
        }
        Ok(json!({
            "ok": ok,
            "query": keyword,
            "submit": submit,
            "filters": filter_result,
            "url": self.current_url().await?,
            "count": cards.len(),
            "cards": cards,
            "reason": if ok { "" } else { submit.get("error").and_then(Value::as_str).unwrap_or("search_submit_failed") },
        }))
    }

    pub async fn submit_search(&self, query: &str, wait_seconds: f64) -> Result<Value> {
        let loc = self.expect_object("searchInput", None).await?;
        if !script_ok(&loc) {
            return Ok(json!({
                "ok": false,
                "strategy": "search_input_unavailable",
                "error": loc.get("error").and_then(Value::as_str).unwrap_or_default(),
            }));
        }

        if let Some(input) = loc.get("input") {
            self.page
                .click(number(input, "x"), number(input, "y"))
                .await?;
            sleep_ms(150).await;
        }

        let set_result = self
            .expect_object("setSearchInput", Some(&json!({ "query": query })))
            .await?;
        if !script_ok(&set_result) {
            return Ok(json!({
                "ok": false,
                "strategy": "set_search_input_failed",
                "state": set_result,
                "error": set_result
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Search input did not accept the requested keyword"),
            }));
        }

        sleep_ms(150).await;
        self.page.press_key("Enter").await?;
        let state = self
            .wait_for_search_transition(query, wait_seconds.max(SEARCH_TRANSITION_TIMEOUT_S))
            .await?;
        if search_transition_ok(&state, query) {
            return Ok(json!({
                "ok": true,
                "strategy": "click_input_set_value_enter",
                "state": state,
                "url": self.current_url().await?,
            }));
        }

        if let Some(submit) = loc.get("submit") {
            let x = number(submit, "x");
            let y = number(submit, "y");
            if x > 0.0 && y > 0.0 {
                self.page.click(x, y).await?;
                let state = self
                    .wait_for_search_transition(
                        query,
                        wait_seconds.max(SEARCH_TRANSITION_TIMEOUT_S),
                    )
                    .await?;
                if search_transition_ok(&state, query) {
                    return Ok(json!({
                        "ok": true,
                        "strategy": "click_search_button",
                        "state": state,
                        "url": self.current_url().await?,
                    }));
                }
            }
        }

        Ok(json!({
            "ok": false,
            "strategy": "manual_submit_failed",
            "state": state,
            "url": self.current_url().await?,
            "error": if state.get("login_required").and_then(Value::as_bool).unwrap_or(false) {
                "login_required"
            } else {
                "Search did not transition to a valid Xiaohongshu result page"
            },
        }))
    }

    pub async fn wait_for_search_transition(&self, query: &str, timeout_s: f64) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_s.max(0.2));
        let mut latest = Value::Object(Map::new());
        while Instant::now() < deadline {
            latest = self.expect_object("searchState", None).await?;
            if search_transition_ok(&latest, query) {
                return Ok(latest);
            }
            sleep_ms(150).await;
        }
        if latest.as_object().is_some_and(Map::is_empty) {
            self.expect_object("searchState", None).await
        } else {
            Ok(latest)
        }
    }

    pub async fn extract_search_cards(&self) -> Result<Vec<XhsNoteCard>> {
        self.ensure_xhs(false).await?;
        let raw = self.expect_array("searchCards", None).await?;
        Ok(parse_cards(&raw))
    }

    /// Scroll the search feed to lazy-load more cards. By default jumps to the
    /// document bottom (window-size independent, no hard-coded pixel step); with
    /// `nudge_up` it instead scrolls back up ~1/10 of a screen to jog XHS's
    /// infinite-scroll observer when a bottom jump failed to trigger a load.
    pub async fn scroll_feed(&self, nudge_up: bool) -> Result<Value> {
        self.ensure_xhs(false).await?;
        self.expect_object("scrollFeed", Some(&json!({ "nudge_up": nudge_up })))
            .await
    }

    /// Poll `extract_search_cards` until the count grows past `baseline` or
    /// `timeout` elapses; returns the latest cards either way.
    async fn wait_for_card_growth(
        &self,
        baseline: usize,
        timeout: Duration,
    ) -> Result<Vec<XhsNoteCard>> {
        const POLL: Duration = Duration::from_millis(400);
        let deadline = Instant::now() + timeout;
        loop {
            sleep_ms(POLL.as_millis() as u64).await;
            let cards = self.extract_search_cards().await?;
            if cards.len() > baseline || Instant::now() >= deadline {
                return Ok(cards);
            }
        }
    }

    /// Collect search cards up to `target`, scrolling the feed and waiting for
    /// lazy-loaded cards after each scroll. Stops once we have enough cards or
    /// the feed stops growing across a few consecutive scrolls (real end of
    /// results). Returns at most `target` cards in feed order.
    async fn collect_search_cards(&self, target: usize) -> Result<Vec<XhsNoteCard>> {
        // XHS sometimes ignores a too-fast jump to the bottom and won't fetch
        // more, so pause before each jump; if a jump still loads nothing within
        // SETTLE_TIMEOUT, a small reverse (upward) scroll reliably re-triggers
        // the lazy load. Give up only after MAX_STALLS rounds where even the
        // nudge fails (the real end of results).
        const PRE_SCROLL_DELAY: Duration = Duration::from_millis(1500);
        const SETTLE_TIMEOUT: Duration = Duration::from_millis(5000);
        const MAX_STALLS: usize = 3;

        let mut cards = self.extract_search_cards().await?;
        let mut stalls = 0usize;
        while cards.len() < target {
            let before = cards.len();

            // 1) Deliberate pause, then jump to the bottom to request more.
            sleep_ms(PRE_SCROLL_DELAY.as_millis() as u64).await;
            self.scroll_feed(false).await?;
            cards = self.wait_for_card_growth(before, SETTLE_TIMEOUT).await?;

            // 2) Nothing loaded in time — nudge back up to jog the observer and
            //    wait once more before counting this round as a stall.
            if cards.len() <= before {
                self.scroll_feed(true).await?;
                cards = self.wait_for_card_growth(before, SETTLE_TIMEOUT).await?;
            }

            if cards.len() <= before {
                stalls += 1;
                if stalls >= MAX_STALLS {
                    break;
                }
            } else {
                stalls = 0;
            }
        }
        cards.truncate(target);
        Ok(cards)
    }

    pub async fn open_note(
        &self,
        note_id: &str,
        index: Option<usize>,
        wait_seconds: f64,
    ) -> Result<Value> {
        let cards = self.extract_search_cards().await?;
        let selected = select_card(&cards, note_id, index).ok_or_else(|| {
            anyhow::anyhow!("Could not resolve note target from current search cards.")
        })?;

        let mut click_arg = Map::new();
        if !selected.note_id.is_empty() {
            click_arg.insert("note_id".into(), Value::String(selected.note_id.clone()));
        }
        if let Some(index) = index {
            click_arg.insert("index".into(), json!(index));
        } else {
            click_arg.insert("index".into(), json!(selected.position));
        }

        let target = self
            .expect_object("clickCard", Some(&Value::Object(click_arg.clone())))
            .await?;
        if !script_ok(&target) {
            anyhow::bail!(
                "Could not locate card to click for note {}: {}",
                selected.note_id,
                target
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
        }

        self.set_last_note_id(String::new());
        let per_attempt = (wait_seconds / 2.0).max(0.2);
        let click_target_kind = target
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or("cover");

        sleep_ms(180).await;
        self.page
            .click(number(&target, "x"), number(&target, "y"))
            .await?;
        let opened = self.wait_for_note_open(per_attempt).await?;
        if note_is_open(&opened) {
            return Ok(json!({
                "ok": true,
                "target": selected,
                "url": self.current_url().await?,
                "state": opened,
                "strategy": format!("{click_target_kind}_click"),
            }));
        }

        let retry = self
            .expect_object("clickCard", Some(&Value::Object(click_arg)))
            .await?;
        if script_ok(&retry) {
            sleep_ms(180).await;
            self.page
                .click(number(&retry, "x"), number(&retry, "y"))
                .await?;
            let opened = self.wait_for_note_open(per_attempt).await?;
            if note_is_open(&opened) {
                return Ok(json!({
                    "ok": true,
                    "target": selected,
                    "url": self.current_url().await?,
                    "state": opened,
                    "strategy": format!("retry_{}_click", retry.get("target").and_then(Value::as_str).unwrap_or("card")),
                }));
            }
        }

        Ok(json!({
            "ok": false,
            "target": selected,
            "url": self.current_url().await?,
            "state": opened,
            "strategy": "card_click_failed",
            "error": if opened.get("login_required").and_then(Value::as_bool).unwrap_or(false) {
                "login_required"
            } else {
                "Note overlay did not open after card-click attempts; site may be throttling or layout changed"
            },
        }))
    }

    pub async fn close_note(&self, wait_seconds: f64) -> Result<Value> {
        let before = self.expect_object("noteOpen", None).await?;
        if !note_is_open(&before) {
            self.set_last_note_id(String::new());
            return Ok(json!({ "ok": true, "strategy": "already_closed", "state": before }));
        }

        let per_attempt = wait_seconds.max(0.2);
        self.page.press_key("Escape").await?;
        let state = self.wait_for_note_closed(per_attempt).await?;
        if !note_is_open(&state) {
            self.set_last_note_id(String::new());
            return Ok(
                json!({ "ok": true, "strategy": "escape", "state": state, "url": self.current_url().await? }),
            );
        }

        let _ = self
            .page
            .evaluate_json("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', code: 'Escape', keyCode: 27, which: 27, bubbles: true}))")
            .await;
        let state = self.wait_for_note_closed(per_attempt).await?;
        if !note_is_open(&state) {
            self.set_last_note_id(String::new());
            return Ok(
                json!({ "ok": true, "strategy": "escape_dispatch", "state": state, "url": self.current_url().await? }),
            );
        }

        let close_btn = self.expect_object("closeNote", None).await?;
        if script_ok(&close_btn) {
            self.page
                .click(number(&close_btn, "x"), number(&close_btn, "y"))
                .await?;
            let state = self.wait_for_note_closed(per_attempt).await?;
            if !note_is_open(&state) {
                self.set_last_note_id(String::new());
                return Ok(json!({
                    "ok": true,
                    "strategy": "close_button",
                    "selector": close_btn.get("selector").and_then(Value::as_str).unwrap_or_default(),
                    "state": state,
                    "url": self.current_url().await?,
                }));
            }
        }

        if before
            .get("on_detail_route")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let _ = self
                .page
                .evaluate_json("history.back(); return {ok: true};")
                .await;
            let state = self.wait_for_note_closed(per_attempt.max(1.5)).await?;
            if !note_is_open(&state) {
                self.set_last_note_id(String::new());
                return Ok(json!({
                    "ok": true,
                    "strategy": "history_back",
                    "state": state,
                    "url": self.current_url().await?,
                }));
            }
        }

        Ok(json!({
            "ok": false,
            "strategy": "close_failed",
            "state": state,
            "url": self.current_url().await?,
            "error": "Note modal did not close after Escape, JS-dispatch Escape, and close-button attempts",
        }))
    }

    pub async fn read_note(
        &self,
        note_id: &str,
        index: Option<usize>,
        wait_seconds: f64,
    ) -> Result<Value> {
        self.read_note_with_options(note_id, index, wait_seconds, ReadNoteOptions::default())
            .await
    }

    pub async fn read_note_with_options(
        &self,
        note_id: &str,
        index: Option<usize>,
        wait_seconds: f64,
        mut options: ReadNoteOptions,
    ) -> Result<Value> {
        let mut open = None;
        if options.note_id_fallback.trim().is_empty() && !note_id.trim().is_empty() {
            options.note_id_fallback = note_id.trim().to_string();
        }
        if !note_id.is_empty() || index.is_some() {
            let opened = self.open_note(note_id, index, wait_seconds).await?;
            if !script_ok(&opened) {
                return Ok(
                    json!({ "ok": false, "open": opened, "error": opened.get("error").and_then(Value::as_str).unwrap_or("open_failed") }),
                );
            }
            if options.poster_url_fallback.trim().is_empty() {
                if let Some(cover_url) = opened
                    .get("target")
                    .and_then(|target| target.get("cover_url"))
                    .and_then(Value::as_str)
                    .filter(|url| !url.trim().is_empty())
                {
                    options.poster_url_fallback = cover_url.to_string();
                }
            }
            if options.note_id_fallback.trim().is_empty() {
                if let Some(target_note_id) = opened
                    .get("target")
                    .and_then(|target| target.get("note_id"))
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                {
                    options.note_id_fallback = target_note_id.to_string();
                }
            }
            open = Some(opened);
        }
        let note = self
            .extract_note_with_options(wait_seconds, options.clone())
            .await?;
        if !note_id.is_empty() && !note.note_id.is_empty() && note.note_id != note_id {
            // Opened the wrong note. Do NOT fall back to a full-page navigate
            // to the tokenized URL: that loads the note full-screen and tears
            // down the search grid + `__INITIAL_STATE__.search` state, which
            // breaks every subsequent open in a topic scan. Report a soft
            // failure; the caller closes the overlay and moves on intact.
            let _ = options;
            return Ok(json!({
                "ok": false,
                "entity": note,
                "open": open,
                "error": format!("stale_note: expected {note_id}, got {}", note.note_id),
            }));
        }
        Ok(json!({ "ok": true, "entity": note, "open": open }))
    }

    pub async fn extract_comments(&self, max_comments: i64) -> Result<Vec<Value>> {
        self.ensure_xhs(false).await?;
        let raw = self
            .expect_array(
                "comments",
                Some(&json!({ "prefer_hot": true, "max_comments": max_comments })),
            )
            .await?;
        Ok(raw.into_iter().filter(Value::is_object).collect())
    }

    pub async fn extract_comments_with_wait(
        &self,
        max_comments: i64,
        wait_seconds: f64,
    ) -> Result<Value> {
        self.ensure_xhs(false).await?;
        self.expect_object(
            "commentsWithWait",
            Some(&json!({
                "prefer_hot": true,
                "max_comments": max_comments,
                "timeout_ms": (wait_seconds.max(0.5) * 1000.0) as i64,
            })),
        )
        .await
    }

    /// Load up to `max_total` comments on the currently open note by driving the
    /// page like a human: wait for the list to hydrate, then loop
    /// scroll-comment-area → click "展开 N 条回复" → re-measure until the budget is
    /// met, the "- THE END -" sentinel shows with no reply buttons left, growth
    /// stalls, or the timeout hits. `max_total` counts primary comments *and*
    /// their replies together; `0`/negative means "load everything available".
    /// The returned `comments` array is hot-sorted and trimmed so primary + reply
    /// count does not exceed the budget.
    pub async fn load_comments(&self, max_total: i64, timeout_seconds: f64) -> Result<Value> {
        self.ensure_xhs(false).await?;
        // Let the (slow) comment list hydrate before the first measurement.
        let first = self.extract_comments_with_wait(0, 5.0).await.ok();

        let deadline = Instant::now() + Duration::from_secs_f64(timeout_seconds.max(2.0));
        let mut last_total: i64 = -1;
        let mut stale_rounds = 0;
        let mut rounds = 0;
        let stop_reason;
        loop {
            let state = self
                .expect_object("commentAreaState", None)
                .await
                .unwrap_or_else(|_| json!({}));
            let loaded_total = state.get("loaded_total").and_then(Value::as_i64).unwrap_or(0);
            let pending = state
                .get("pending_show_more")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let has_end = state.get("has_end").and_then(Value::as_bool).unwrap_or(false);

            // Budget reached — no need to load more.
            if max_total > 0 && loaded_total >= max_total {
                stop_reason = "budget_reached";
                break;
            }
            // Everything that exists is loaded and expanded: the end sentinel is
            // showing, no reply threads are left to expand, and the count has
            // settled.
            if has_end && pending == 0 && loaded_total == last_total {
                stop_reason = "exhausted";
                break;
            }
            if loaded_total == last_total {
                stale_rounds += 1;
            } else {
                stale_rounds = 0;
            }
            // No growth across several rounds despite scrolling/expanding.
            if stale_rounds >= 3 {
                stop_reason = "stalled";
                break;
            }
            if Instant::now() >= deadline {
                stop_reason = "timeout";
                break;
            }
            last_total = loaded_total;
            rounds += 1;

            // Expand any visible reply buttons first (cheap, surfaces sub-comments
            // that count toward the budget), then scroll for more parents.
            if pending > 0 {
                let _ = self.expect_object("expandCommentReplies", None).await;
                sleep_ms(500).await;
            }
            if !has_end {
                let _ = self.scroll_in_note(1200).await;
            }
            sleep_ms(500).await;
        }

        let all = self.extract_comments(0).await.unwrap_or_default();
        let loaded_primary = all.len() as i64;
        let loaded_total: i64 = loaded_primary
            + all
                .iter()
                .map(|c| c.get("sub_comments").and_then(Value::as_array).map_or(0, Vec::len) as i64)
                .sum::<i64>();
        let comments = trim_comments_to_budget(all, max_total);
        let returned_total: i64 = comments.len() as i64
            + comments
                .iter()
                .map(|c| c.get("sub_comments").and_then(Value::as_array).map_or(0, Vec::len) as i64)
                .sum::<i64>();

        Ok(json!({
            "ok": true,
            "comments": comments,
            "loaded_primary": loaded_primary,
            "loaded_total": loaded_total,
            "returned_total": returned_total,
            "rounds": rounds,
            "stop_reason": stop_reason,
            "ready": first
                .as_ref()
                .and_then(|v| v.get("ready"))
                .and_then(Value::as_bool)
                .unwrap_or(loaded_total > 0),
        }))
    }

    pub async fn collect_carousel_images(&self, max_images: i64) -> Result<Vec<String>> {
        self.ensure_xhs(false).await?;
        let raw = self
            .expect_object("carouselImages", Some(&json!({ "max_images": max_images })))
            .await?;
        let urls = raw
            .get("image_urls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(urls
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .filter(|s| !s.trim().is_empty())
            .collect())
    }

    pub async fn scroll_in_note(&self, pixels: i64) -> Result<Value> {
        self.ensure_xhs(false).await?;
        self.expect_object("scrollInNote", Some(&json!({ "pixels": pixels })))
            .await
    }

    /// Apply search-result filters from the hover popup and return the
    /// current filter state.
    pub async fn apply_search_filters(&self, filters: &Value, wait_seconds: f64) -> Result<Value> {
        // Normalize filter targets so we can compare the desired active option with the current one,
        // avoiding unnecessary clicks when they already match.
        let target_filters = normalize_search_filter_targets(filters)?;
        self.apply_search_filter_targets(&target_filters, wait_seconds)
            .await
    }

    async fn apply_search_filter_targets(
        &self,
        target_filters: &[(String, String)],
        wait_seconds: f64,
    ) -> Result<Value> {
        self.ensure_xhs(false).await?;
        if target_filters.is_empty() {
            anyhow::bail!("filter targets must include at least one selection");
        }
        // Applying filters too early can leave old cards visible.
        sleep_ms(1000).await;

        let initial_raw = self.open_search_filter_panel(wait_seconds).await?;
        if !script_ok(&initial_raw) {
            self.close_search_filter_panel(None).await?;
            return Ok(initial_raw);
        }
        let initial_filters = canonical_filter_state(&initial_raw);
        let mut changed_filters = false;
        for (group_key, label) in target_filters {
            let Some(option) = search_filter_option(&initial_filters, group_key, label) else {
                self.close_search_filter_panel(Some(&initial_filters))
                    .await?;
                return Ok(json!({
                    "ok": false,
                    "error": "filter_option_not_found",
                    "group": group_key,
                    "label": label,
                    "filters": initial_filters,
                }));
            };
            if option
                .get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let x = number(option, "x");
            let y = number(option, "y");
            self.page.click(x, y).await?;
            changed_filters = true;
        }

        let final_raw = self.expect_object("searchFilters", None).await?;
        if !script_ok(&final_raw) {
            self.close_search_filter_panel(None).await?;
            return Ok(final_raw);
        }
        let final_filters = canonical_filter_state(&final_raw);
        self.close_search_filter_panel(Some(&final_filters)).await?;
        if changed_filters && wait_seconds > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(wait_seconds.min(4.0))).await;
        }
        Ok(json!({
            "ok": true,
            "changed": changed_filters,
            "filters": final_filters,
        }))
    }

    /// Reset search-result filters.
    pub async fn reset_search_filters(&self, wait_seconds: f64) -> Result<Value> {
        self.ensure_xhs(false).await?;
        let current_raw = self.open_search_filter_panel(wait_seconds).await?;
        if !script_ok(&current_raw) {
            self.close_search_filter_panel(None).await?;
            return Ok(current_raw);
        }
        let current = canonical_filter_state(&current_raw);

        let Some(target) = current.get("reset").filter(|value| !value.is_null()) else {
            self.close_search_filter_panel(Some(&current)).await?;
            return Ok(json!({
                "ok": false,
                "error": "filter_reset_not_found",
                "filters": current,
            }));
        };
        let x = number(target, "x");
        let y = number(target, "y");
        self.page.click(x, y).await?;
        self.close_search_filter_panel(Some(&current)).await?;
        if wait_seconds > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(wait_seconds.min(4.0))).await;
        }
        Ok(json!({
            "ok": true,
            "reset": true,
        }))
    }

    /// Open the search filter panel and return the current filter state. If the
    /// trigger is off-screen, scroll to the top and retry.
    async fn open_search_filter_panel(&self, wait_seconds: f64) -> Result<Value> {
        let visible = self.expect_object("searchFilters", None).await?;
        if script_ok(&visible) {
            return Ok(visible);
        }

        let trigger = self.expect_object("searchFilterTrigger", None).await?;
        let trigger = if script_ok(&trigger) {
            trigger
        } else {
            self.page
                .evaluate_json(
                    "window.scrollTo({ left: 0, top: 0, behavior: 'instant' }); return { ok: true, y: scrollY };",
                )
                .await?;
            sleep_ms(120).await;
            let retry = self.expect_object("searchFilterTrigger", None).await?;
            if !script_ok(&retry) {
                return Ok(retry);
            }
            retry
        };

        self.page
            .mouse_move(number(&trigger, "x"), number(&trigger, "y"))
            .await?;
        let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(0.2, 3.0));
        let mut latest = Value::Object(Map::new());
        while Instant::now() < deadline {
            latest = self.expect_object("searchFilters", None).await?;
            if script_ok(&latest) {
                return Ok(latest);
            }
            sleep_ms(120).await;
        }
        if latest.as_object().is_some_and(Map::is_empty) {
            self.expect_object("searchFilters", None).await
        } else {
            Ok(latest)
        }
    }

    /// Close the filter popup, preferring the visible `收起` control and
    /// falling back to moving the mouse away from the popup trigger.
    async fn close_search_filter_panel(&self, filters: Option<&Value>) -> Result<()> {
        if let Some(target) = filters
            .and_then(|value| value.get("close"))
            .filter(|value| !value.is_null())
        {
            let x = number(target, "x");
            let y = number(target, "y");
            self.page.click(x, y).await?;
            sleep_ms(180).await;
            return Ok(());
        }
        self.page.mouse_move(20.0, 20.0).await?;
        sleep_ms(120).await;
        Ok(())
    }

    /// Read the currently visible profile page. Caller is responsible for
    /// having navigated to a profile URL beforehand; this method only
    /// extracts (and refuses if the current page isn't a profile).
    pub async fn extract_profile(
        &self,
        max_notes: i64,
        scroll_rounds: i64,
    ) -> Result<crate::sites::xhs::XhsAuthorProfile> {
        self.ensure_xhs(false).await?;
        let state = self.detect_state().await?;
        let state_kind = state.get("state").and_then(Value::as_str).unwrap_or("");
        if state_kind != "profile_page" {
            let url = state.get("url").and_then(Value::as_str).unwrap_or("");
            return Err(anyhow::anyhow!(
                "Current Xiaohongshu page is not a profile page: {url}"
            ));
        }
        let info_value = self.expect_object("profileInfo", None).await?;
        let info = info_value.as_object().cloned().unwrap_or_default();
        let cards = self.extract_profile_cards(max_notes, scroll_rounds).await?;
        let fallback_url = self.current_url().await.unwrap_or_default();
        Ok(crate::sites::xhs::XhsAuthorProfile {
            display_name: string_field(&info, "display_name"),
            xhs_id: string_field(&info, "xhs_id"),
            profile_url: {
                let candidate = string_field(&info, "profile_url");
                if candidate.is_empty() {
                    fallback_url
                } else {
                    candidate
                }
            },
            bio: string_field(&info, "bio"),
            ip_location: string_field(&info, "ip_location"),
            followers: string_field(&info, "followers"),
            following: string_field(&info, "following"),
            likes_and_collections: string_field(&info, "likes_and_collections"),
            note_cards: cards,
        })
    }

    /// Scroll the profile page while pulling visible note cards. Stops
    /// after `max_notes` unique cards or `scroll_rounds` rounds, whichever
    /// comes first.
    pub async fn extract_profile_cards(
        &self,
        max_notes: i64,
        scroll_rounds: i64,
    ) -> Result<Vec<XhsNoteCard>> {
        let rounds = scroll_rounds.max(1);
        let limit = max_notes.max(1) as usize;
        let mut cards: Vec<XhsNoteCard> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for round_index in 0..rounds {
            let raw = self.expect_array("profileCards", None).await?;
            for (index, item) in raw.iter().enumerate() {
                let Some(obj) = item.as_object() else {
                    continue;
                };
                let position = obj
                    .get("position")
                    .and_then(Value::as_i64)
                    .unwrap_or(index as i64);
                let card = XhsNoteCard {
                    note_id: string_field(obj, "note_id"),
                    title: string_field(obj, "title"),
                    author: string_field(obj, "author"),
                    author_id: string_field(obj, "author_id"),
                    author_url: string_field(obj, "author_url"),
                    likes: string_field(obj, "likes"),
                    link: string_field(obj, "link"),
                    cover_url: string_field(obj, "cover_url"),
                    r#type: string_field(obj, "type"),
                    position,
                    xsec_token: string_field(obj, "xsec_token"),
                };
                let key = if !card.note_id.is_empty() {
                    card.note_id.clone()
                } else if !card.link.is_empty() {
                    card.link.clone()
                } else {
                    format!("pos:{}", card.position)
                };
                if key.is_empty() || seen.contains(&key) {
                    continue;
                }
                seen.insert(key);
                cards.push(card);
                if cards.len() >= limit {
                    return Ok(cards);
                }
            }
            if round_index < rounds - 1 {
                self.page.scroll(900).await?;
                tokio::time::sleep(Duration::from_millis(800)).await;
            }
        }
        cards.truncate(limit);
        Ok(cards)
    }

    /// Navigate directly to an author's profile page by id and wait for the
    /// page to settle on a `profile_page` state. This is the single allowed
    /// direct-URL navigation — it's the entry point of an author scan; every
    /// subsequent action (scrolling, opening notes) is DOM-driven like the
    /// rest of the site. Stops early if a login wall appears.
    pub async fn open_profile(&self, author_id: &str, wait_seconds: f64) -> Result<Value> {
        let id = author_id.trim();
        if id.is_empty() {
            anyhow::bail!("author id is required");
        }
        let url = format!("https://www.xiaohongshu.com/user/profile/{id}");
        self.page.navigate_with_timeout(&url, 60.0).await?;

        let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.max(1.0));
        let mut state = self.expect_object("pageState", None).await?;
        loop {
            let kind = state.get("state").and_then(Value::as_str).unwrap_or("");
            let login = state
                .get("login_required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if kind == "profile_page" || login || Instant::now() >= deadline {
                break;
            }
            sleep_ms(300).await;
            state = self.expect_object("pageState", None).await?;
        }

        let kind = state.get("state").and_then(Value::as_str).unwrap_or("");
        let login = state
            .get("login_required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(json!({
            "ok": kind == "profile_page",
            "author_id": id,
            "url": self.current_url().await?,
            "login_required": login,
            "state": state,
            "reason": if kind == "profile_page" {
                ""
            } else if login {
                "login_required"
            } else {
                "not_profile_page"
            },
        }))
    }

    /// Collect note cards from the current profile page, scrolling the window
    /// to lazy-load more until `target` unique cards are gathered or the feed
    /// stops growing across a few consecutive scrolls (real end of the grid).
    /// Returns at most `target` cards in feed order.
    pub async fn collect_profile_cards(&self, target: usize) -> Result<Vec<XhsNoteCard>> {
        const MAX_STALLS: usize = 3;
        let limit = target.max(1);
        let mut seen: HashSet<String> = HashSet::new();
        let mut cards: Vec<XhsNoteCard> = Vec::new();
        let mut stalls = 0usize;
        loop {
            let raw = self.expect_array("profileCards", None).await?;
            let before = cards.len();
            for card in parse_cards(&raw) {
                let key = if !card.note_id.is_empty() {
                    card.note_id.clone()
                } else if !card.link.is_empty() {
                    card.link.clone()
                } else {
                    format!("pos:{}", card.position)
                };
                if key.is_empty() || !seen.insert(key) {
                    continue;
                }
                cards.push(card);
                if cards.len() >= limit {
                    return Ok(cards);
                }
            }
            if cards.len() <= before {
                stalls += 1;
                if stalls >= MAX_STALLS {
                    break;
                }
            } else {
                stalls = 0;
            }
            self.page.scroll(900).await?;
            sleep_ms(900).await;
        }
        Ok(cards)
    }

    /// Read the note cards currently rendered on the profile page without
    /// scrolling (the cheap first-page-only path).
    pub async fn extract_profile_cards_once(&self) -> Result<Vec<XhsNoteCard>> {
        let raw = self.expect_array("profileCards", None).await?;
        Ok(parse_cards(&raw))
    }

    /// Extract just the author header (display name, xhs id, bio, IP location,
    /// follower/following/like counts) from the current profile page.
    pub async fn profile_info(&self) -> Result<Value> {
        self.expect_object("profileInfo", None).await
    }

    /// Extract the currently open note. Caller is responsible for having
    /// navigated to the note URL (or having opened the note modal); the JS
    /// side polls via `noteWithWait` until content hydrates, so the caller
    /// doesn't need a separate readiness check.
    pub async fn extract_note(&self, wait_seconds: f64) -> Result<XhsNote> {
        self.extract_note_with_options(wait_seconds, ReadNoteOptions::default())
            .await
    }

    pub async fn extract_note_with_options(
        &self,
        wait_seconds: f64,
        options: ReadNoteOptions,
    ) -> Result<XhsNote> {
        let timeout_ms = (wait_seconds.max(0.5) * 1000.0) as i64;
        let raw = self
            .run_script("noteWithWait", Some(&json!({ "timeout_ms": timeout_ms })))
            .await?;

        let body = raw
            .get("note")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Map::new()));

        let mut note = parse_note(&body);

        if note.note_id.is_empty() && !options.note_id_fallback.trim().is_empty() {
            note.note_id = options.note_id_fallback.trim().to_string();
        }

        // Fall back to the live page URL when the JS payload didn't populate
        // body.url. One extra evaluate is cheap and keeps extraction stable
        // across navigation styles.
        if note.url.is_empty() {
            if let Ok(href) = self.page.evaluate_json("location.href").await {
                if let Some(s) = href.as_str() {
                    note.url = normalize_url(s);
                }
            }
        }

        if !note.note_id.is_empty() {
            let prev = self.last_note_id();
            if !prev.is_empty() && prev == note.note_id {
                note.stale_warning = Some(format!(
                    "This note (note_id={}) was already extracted in the previous read. The note modal may not have closed before opening the next card — call xhs_close_note to verify the modal is gone, then re-open the target card.",
                    note.note_id
                ));
            }
            self.set_last_note_id(note.note_id.clone());
        }

        note.wait_meta = Some(json!({
            "ready": raw.get("ready").and_then(Value::as_bool).unwrap_or(false),
            "reason": raw.get("reason").and_then(Value::as_str).unwrap_or(""),
            "waited_ms": raw.get("waited_ms").and_then(Value::as_i64).unwrap_or(0),
            "attempts": raw.get("attempts").and_then(Value::as_i64).unwrap_or(0),
        }));

        if note.r#type == "video" && !options.poster_url_fallback.trim().is_empty() {
            apply_video_poster_fallback(&mut note.video, &options.poster_url_fallback);
        }

        if options.include_media {
            self.enrich_note_media(&mut note, options.max_images, options.max_video_frames)
                .await?;
        }
        if options.download_media {
            self.download_note_media(&mut note, options.max_images)
                .await?;
            // OCR runs over the just-downloaded image files. Only meaningful for
            // image notes; for video the images vec is empty so this no-ops.
            if options.ocr {
                if let Some(media) = &self.media {
                    media.ocr_downloaded_images(&mut note.images).await;
                    note.image_count = note.images.len() as i64;
                }
            }
        }

        Ok(note)
    }

    async fn download_note_media(&self, note: &mut XhsNote, max_images: usize) -> Result<()> {
        let Some(media) = &self.media else {
            if note.r#type == "video" {
                insert_value_string(
                    &mut note.video,
                    "download_error",
                    "media processor unavailable",
                );
            } else {
                for image in &mut note.images {
                    insert_value_string(image, "download_error", "media processor unavailable");
                }
            }
            return Ok(());
        };

        let label = if note.note_id.is_empty() {
            if note.title.is_empty() {
                "xhs_note"
            } else {
                &note.title
            }
        } else {
            &note.note_id
        };

        if note.r#type == "video" {
            note.video = media
                .download_video(&note.video, &note.note_id, &note.title, &note.url)
                .await;
            return Ok(());
        }

        if note.images.is_empty() {
            let urls = self.collect_carousel_images(max_images as i64).await?;
            note.images = urls
                .into_iter()
                .take(max_images)
                .enumerate()
                .map(|(index, url)| json!({ "url": url, "index": index as i64 }))
                .collect();
            note.image_count = note.images.len() as i64;
        }

        let images: Vec<Value> = note.images.iter().take(max_images).cloned().collect();
        note.images = media.download_images(&images, &note.url, label).await;
        note.image_count = note.images.len() as i64;
        Ok(())
    }

    async fn enrich_note_media(
        &self,
        note: &mut XhsNote,
        max_images: usize,
        max_video_frames: usize,
    ) -> Result<()> {
        let Some(media) = &self.media else {
            if note.r#type == "video" {
                insert_value_string(
                    &mut note.video,
                    "media_error",
                    "media processor unavailable",
                );
            } else {
                for image in &mut note.images {
                    insert_value_string(image, "media_error", "media processor unavailable");
                }
            }
            return Ok(());
        };

        if note.r#type == "video" {
            note.video = media
                .enrich_video(
                    &note.video,
                    &note.note_id,
                    &note.title,
                    &note.url,
                    max_video_frames,
                    true,
                )
                .await;
            return Ok(());
        }

        if note.images.is_empty() {
            let urls = self.collect_carousel_images(max_images as i64).await?;
            note.images = urls
                .into_iter()
                .take(max_images)
                .enumerate()
                .map(|(index, url)| json!({ "url": url, "index": index as i64 }))
                .collect();
            note.image_count = note.images.len() as i64;
        }

        let images: Vec<Value> = note.images.iter().take(max_images).cloned().collect();
        note.images = media
            .enrich_images(
                &images,
                &note.url,
                if note.note_id.is_empty() {
                    if note.title.is_empty() {
                        "xhs_note"
                    } else {
                        &note.title
                    }
                } else {
                    &note.note_id
                },
                true,
            )
            .await;
        note.image_count = note.images.len() as i64;
        Ok(())
    }

    async fn expect_object(&self, name: &str, arg: Option<&Value>) -> Result<Value> {
        let value = self.run_script(name, arg).await?;
        if value.is_object() {
            Ok(value)
        } else {
            anyhow::bail!(
                "XHS page script {name} returned {}, expected object",
                value_type(&value)
            );
        }
    }

    async fn expect_array(&self, name: &str, arg: Option<&Value>) -> Result<Vec<Value>> {
        let value = self.run_script(name, arg).await?;
        value.as_array().cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "XHS page script {name} returned {}, expected array",
                value_type(&value)
            )
        })
    }

    async fn wait_for_note_open(&self, timeout_s: f64) -> Result<Value> {
        wait_for_note_state(self, timeout_s, true).await
    }

    async fn wait_for_note_closed(&self, timeout_s: f64) -> Result<Value> {
        wait_for_note_state(self, timeout_s, false).await
    }
}

fn normalize_search_filter_targets(filters: &Value) -> Result<Vec<(String, String)>> {
    let Some(input) = filters.as_object() else {
        anyhow::bail!("filters must be object");
    };
    if input.is_empty() {
        anyhow::bail!("filters must include at least one selection");
    }

    for key in input.keys() {
        if !XHS_SEARCH_FILTERS
            .iter()
            .any(|(filter_key, _, _)| *filter_key == key.as_str())
        {
            anyhow::bail!("unsupported filter group: {key}");
        }
    }

    XHS_SEARCH_FILTERS
        .iter()
        .map(|(key, _title, options)| {
            let label = match input.get(*key) {
                Some(value) => value
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("filter option must be a non-empty string: {}", key)
                    })?,
                None => options.first().copied().unwrap_or(""),
            };
            if !options.contains(&label) {
                anyhow::bail!("unsupported filter option for {}: {label}", key);
            }
            Ok((key.to_string(), label.to_string()))
        })
        .collect()
}

fn search_filter_option<'a>(filters: &'a Value, group_key: &str, label: &str) -> Option<&'a Value> {
    filters
        .get("groups")?
        .as_array()?
        .iter()
        .find(|item| item.get("key").and_then(Value::as_str) == Some(group_key))?
        .get("options")
        .and_then(Value::as_array)?
        .iter()
        .find(|item| item.get("label").and_then(Value::as_str) == Some(label))
}

/// Re-key the raw filter panel reported by the page script — which only knows
/// each group's visible Chinese `title` — into socai's canonical group `key`s,
/// using [`XHS_SEARCH_FILTERS`] for the title↔key join and group order, and
/// adding a per-group `active` summary. This is where the human-readable panel
/// is tied back to the schema keys callers used; downstream lookups (and the
/// reported `filters` payload) can then rely on `key`. Unknown groups/tags are
/// passed through untouched in `options` so a page-side addition never silently
/// drops, but only known titles get a canonical key.
fn canonical_filter_state(raw: &Value) -> Value {
    let mut out = raw.clone();
    let Some(groups) = raw.get("groups").and_then(Value::as_array) else {
        return out;
    };
    let canonical: Vec<Value> = XHS_SEARCH_FILTERS
        .iter()
        .filter_map(|(key, title, _options)| {
            let group = groups
                .iter()
                .find(|g| g.get("title").and_then(Value::as_str) == Some(*title))?;
            let active = group
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|o| o.get("active").and_then(Value::as_bool).unwrap_or(false))
                .and_then(|o| o.get("label").and_then(Value::as_str))
                .unwrap_or("");
            let mut group = group.clone();
            if let Some(map) = group.as_object_mut() {
                map.insert("key".to_string(), json!(key));
                map.insert("active".to_string(), json!(active));
            }
            Some(group)
        })
        .collect();
    if let Some(map) = out.as_object_mut() {
        map.insert("groups".to_string(), Value::Array(canonical));
    }
    out
}

/// Parse the JS-side `body` payload into a wire-ready XhsNote. Performs all
/// normalization up front so serde Serialize alone produces stable output.
fn parse_note(body: &Value) -> XhsNote {
    let s = |k: &str| {
        body.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };

    let hashtags: Vec<String> = body
        .get("hashtags")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .take(12)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let image_urls: Vec<String> = body
        .get("image_urls")
        .and_then(Value::as_array)
        .map(|arr| {
            let mut seen = HashSet::new();
            arr.iter()
                .filter_map(Value::as_str)
                .map(normalize_image_url)
                .filter(|s| !s.is_empty())
                .filter(|s| seen.insert(s.clone()))
                .collect()
        })
        .unwrap_or_default();

    let images: Vec<Value> = image_urls
        .iter()
        .enumerate()
        .map(|(index, url)| json!({ "url": url, "index": index as i64 }))
        .collect();

    let video = body
        .get("video")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));

    let image_count = body
        .get("image_count")
        .and_then(Value::as_i64)
        .filter(|&n| n > 0)
        .unwrap_or(images.len() as i64);

    XhsNote {
        note_id: s("note_id"),
        url: normalize_url(&s("url")),
        r#type: s("type"),
        title: s("title"),
        author: s("author"),
        author_id: s("author_id"),
        author_url: normalize_url(&s("author_url")),
        content: s("content"),
        content_source: s("content_source"),
        hashtags,
        date: s("date"),
        location: s("location"),
        likes: s("likes"),
        favorites: s("favorites"),
        comments_count: s("comments_count"),
        image_count,
        images,
        video,
        wait_meta: None,
        stale_warning: None,
    }
}

async fn wait_for_note_state(
    runtime: &XhsPageRuntime<'_>,
    timeout_s: f64,
    want_open: bool,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout_s.max(0.2));
    let mut latest = Value::Object(Map::new());
    while Instant::now() < deadline {
        latest = runtime.expect_object("noteOpen", None).await?;
        if note_is_open(&latest) == want_open {
            return Ok(latest);
        }
        sleep_ms(150).await;
    }
    if latest.as_object().is_some_and(Map::is_empty) {
        runtime.expect_object("noteOpen", None).await
    } else {
        Ok(latest)
    }
}

fn parse_cards(raw: &[Value]) -> Vec<XhsNoteCard> {
    raw.iter()
        .enumerate()
        .filter_map(|(index, item)| {
            item.as_object().map(|obj| XhsNoteCard {
                note_id: string_field(obj, "note_id"),
                title: string_field(obj, "title"),
                author: string_field(obj, "author"),
                author_id: string_field(obj, "author_id"),
                author_url: string_field(obj, "author_url"),
                likes: string_field(obj, "likes"),
                link: string_field(obj, "link"),
                cover_url: string_field(obj, "cover_url"),
                r#type: string_field(obj, "type"),
                position: obj
                    .get("position")
                    .and_then(Value::as_i64)
                    .unwrap_or(index as i64),
                xsec_token: string_field(obj, "xsec_token"),
            })
        })
        .collect()
}

fn select_card(cards: &[XhsNoteCard], note_id: &str, index: Option<usize>) -> Option<XhsNoteCard> {
    if !note_id.is_empty() {
        if let Some(card) = cards.iter().find(|card| card.note_id == note_id) {
            return Some(card.clone());
        }
    }
    index.and_then(|idx| cards.get(idx).cloned())
}

fn insert_value_string(value: &mut Value, key: &str, text: &str) {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    if let Some(map) = value.as_object_mut() {
        map.insert(key.to_string(), Value::String(text.to_string()));
    }
}

fn apply_video_poster_fallback(video: &mut Value, fallback_url: &str) {
    let fallback_url = fallback_url.trim();
    if fallback_url.is_empty() {
        return;
    }
    let has_poster = video
        .get("poster_url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.trim().is_empty());
    if !has_poster {
        insert_value_string(video, "poster_url", fallback_url);
    }
}

fn search_transition_ok(state: &Value, query: &str) -> bool {
    if state
        .get("page_state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        != "search_results"
    {
        return false;
    }
    let keyword = normalize_keyword(query);
    let visible_keyword = normalize_keyword(
        state
            .get("input_keyword")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let url_keyword = normalize_keyword(
        state
            .get("url_keyword")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    if !keyword.is_empty() && !visible_keyword.is_empty() && visible_keyword != keyword {
        return false;
    }
    if !keyword.is_empty()
        && visible_keyword.is_empty()
        && !url_keyword.is_empty()
        && url_keyword != keyword
    {
        return false;
    }
    if state.get("card_count").and_then(Value::as_i64).unwrap_or(0) > 0 {
        return true;
    }
    if state
        .get("loading")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }
    state
        .get("has_no_results")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn normalize_keyword(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn normalize_image_url(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.replacen("http://", "https://", 1)
}

fn note_is_open(state: &Value) -> bool {
    state
        .get("on_detail_route")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || state
            .get("has_modal")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn script_ok(value: &Value) -> bool {
    value.get("ok").and_then(Value::as_bool).unwrap_or(false)
}

fn string_field(obj: &Map<String, Value>, key: &str) -> String {
    obj.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

async fn sleep_ms(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// Trim a hot-sorted comment list so primary comments plus their replies do not
/// exceed `max_total`. Each parent counts as one; its `sub_comments` are then
/// truncated to whatever budget remains. `max_total <= 0` keeps everything.
fn trim_comments_to_budget(comments: Vec<Value>, max_total: i64) -> Vec<Value> {
    if max_total <= 0 {
        return comments;
    }
    let mut out = Vec::new();
    let mut count: i64 = 0;
    for mut comment in comments {
        if count >= max_total {
            break;
        }
        count += 1; // the parent comment itself
        let remaining = max_total - count;
        if let Some(subs) = comment.get_mut("sub_comments").and_then(Value::as_array_mut) {
            if remaining <= 0 {
                subs.clear();
            } else if subs.len() as i64 > remaining {
                subs.truncate(remaining as usize);
            }
            count += subs.len() as i64;
        }
        out.push(comment);
    }
    out
}

// AGENTS.md: do NOT add new Rust tests unless the user explicitly asks. Update
// the existing ones when an API they cover changes; don't grow this module.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_body_yields_defaults() {
        let note = parse_note(&Value::Object(Map::new()));
        assert_eq!(note.note_id, "");
        assert_eq!(note.image_count, 0);
        assert!(note.images.is_empty());
    }

    #[test]
    fn parse_populates_basic_fields() {
        let body = json!({
            "note_id": "abc123",
            "title": "测试笔记",
            "author": "张三",
            "image_urls": ["https://img.example/1.jpg", "https://img.example/2.jpg"],
            "hashtags": ["#tag1", "#tag2"],
        });
        let note = parse_note(&body);
        assert_eq!(note.note_id, "abc123");
        assert_eq!(note.title, "测试笔记");
        assert_eq!(note.author, "张三");
        assert_eq!(note.image_count, 2);
        assert_eq!(note.images.len(), 2);
        assert_eq!(note.images[0]["url"], "https://img.example/1.jpg");
        assert_eq!(note.images[0]["index"], 0);
        assert_eq!(note.hashtags, vec!["#tag1", "#tag2"]);
    }

    #[test]
    fn hashtags_clipped_to_12() {
        let tags: Vec<String> = (0..20).map(|i| format!("#t{i}")).collect();
        let body = json!({ "hashtags": tags });
        let note = parse_note(&body);
        assert_eq!(note.hashtags.len(), 12);
    }

    #[test]
    fn image_count_prefers_explicit_then_images_len() {
        let body = json!({ "image_count": 5, "image_urls": ["a", "b"] });
        assert_eq!(parse_note(&body).image_count, 5);

        let body = json!({ "image_urls": ["a", "b", "c"] });
        assert_eq!(parse_note(&body).image_count, 3);

        let body = json!({});
        assert_eq!(parse_note(&body).image_count, 0);
    }

    #[test]
    fn video_poster_fallback_fills_missing_url() {
        let mut video = json!({});

        apply_video_poster_fallback(&mut video, " https://img.example/cover.jpg ");

        assert_eq!(video["poster_url"], "https://img.example/cover.jpg");
    }

    #[test]
    fn video_poster_fallback_preserves_existing_url() {
        let mut video = json!({ "poster_url": "https://img.example/poster.jpg" });

        apply_video_poster_fallback(&mut video, "https://img.example/cover.jpg");

        assert_eq!(video["poster_url"], "https://img.example/poster.jpg");
    }
}
