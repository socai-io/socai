use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::cdp::PageSession;
use crate::sites::tiktok::entities::{
    extract_handle, extract_video_id, is_tiktok_page_url, tiktok_author_url, tiktok_video_url,
    TikTokAuthorProfile, TikTokComment, TikTokVideo, TikTokVideoCard,
};

pub const TIKTOK_HOME_URL: &str = "https://www.tiktok.com/";

const PAGE_SCRIPTS_JS: &str = include_str!("page_scripts.js");
const TIKTOK_PAGE_SCRIPT_FUNCTIONS: &[&str] = &[
    "pageState",
    "searchState",
    "videoCards",
    "scrollFeed",
    "videoState",
    "videoDetail",
    "playerPlayButton",
    "commentActivation",
    "comments",
    "scrollComments",
    "authorState",
    "authorProfile",
];
const TRANSITION_TIMEOUT_S: f64 = 20.0;

pub struct TikTokPageRuntime<'a> {
    page: &'a PageSession,
}

impl<'a> TikTokPageRuntime<'a> {
    pub fn new(page: &'a PageSession) -> Self {
        Self { page }
    }

    pub async fn run_script(&self, name: &str, arg: Option<&Value>) -> Result<Value> {
        if !TIKTOK_PAGE_SCRIPT_FUNCTIONS.contains(&name) {
            anyhow::bail!("Unknown TikTok page script: {name}");
        }
        let args = match arg {
            None => String::new(),
            Some(value) => serde_json::to_string(value)?,
        };
        let expr = format!(
            "(function() {{\n{PAGE_SCRIPTS_JS}\n// SOCAI_TIKTOK_CALL: {name}\nreturn SocaiTikTokPageScripts.{name}({args});\n}})()"
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

    pub async fn ensure_tiktok(&self, navigate_if_needed: bool) -> Result<()> {
        let url = self.current_url().await.unwrap_or_default();
        if is_tiktok_page(&url) {
            return Ok(());
        }
        if navigate_if_needed {
            self.soft_navigate(TIKTOK_HOME_URL).await?;
            return Ok(());
        }
        anyhow::bail!(
            "Current page is not TikTok: {}",
            if url.is_empty() { "unknown" } else { &url }
        );
    }

    pub async fn wait_until_interactive(&self, timeout_seconds: f64) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_seconds.max(0.5));
        let mut latest = json!({
            "ok": false,
            "blank_or_throttled": true,
            "reason": "waiting_for_tiktok",
        });
        while Instant::now() < deadline {
            latest = match self.detect_state().await {
                Ok(state) => state,
                Err(err) => json!({
                    "ok": false,
                    "blank_or_throttled": true,
                    "reason": "detect_state_failed",
                    "error": err.to_string(),
                    "url": self.current_url().await.unwrap_or_default(),
                }),
            };
            if state_terminal(&latest)
                || !latest
                    .get("blank_or_throttled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            {
                return Ok(latest);
            }
            sleep_ms(500).await;
        }
        Ok(latest)
    }

    pub async fn detect_state(&self) -> Result<Value> {
        self.ensure_tiktok(false).await?;
        self.expect_object("pageState", None).await
    }

    pub async fn search_videos(
        &self,
        query: &str,
        wait_seconds: f64,
        num_videos: usize,
    ) -> Result<Value> {
        let keyword = query.trim();
        if keyword.is_empty() {
            anyhow::bail!("query is required");
        }
        let target = format!(
            "https://www.tiktok.com/search?q={}",
            percent_encode_query(keyword)
        );
        let navigation = self.navigate_to(&target).await?;
        let state = self
            .wait_for_search_transition(
                keyword,
                wait_seconds.max(TRANSITION_TIMEOUT_S),
                &navigation,
            )
            .await?;
        if let Some(reason) = state_failure_reason(&state) {
            return Ok(json!({
                "ok": false,
                "query": keyword,
                "reason": reason,
                "state": state,
                "count": 0,
                "cards": [],
            }));
        }
        if !search_transition_ok(&state) {
            return Ok(json!({
                "ok": false,
                "query": keyword,
                "reason": "search_results_unavailable",
                "state": state,
                "count": 0,
                "cards": [],
            }));
        }
        let cards = self.collect_video_cards(num_videos.max(1)).await?;
        Ok(json!({
            "ok": true,
            "query": keyword,
            "url": self.current_url().await?,
            "count": cards.len(),
            "cards": cards,
        }))
    }

    pub async fn read_video(
        &self,
        locator: &str,
        wait_seconds: f64,
        num_comments: usize,
        resolve_media: bool,
    ) -> Result<Value> {
        let target = tiktok_video_url(locator)?;
        let expected_id = extract_video_id(&target);
        let navigation = self.navigate_to(&target).await?;
        let mut state = self
            .wait_for_named_state(
                "videoState",
                wait_seconds.max(TRANSITION_TIMEOUT_S),
                "video_id",
                &expected_id,
                &navigation,
            )
            .await?;
        let current_id = extract_video_id(&self.current_url().await.unwrap_or_default());
        let fallback_id = if expected_id.is_empty() {
            state
                .get("video_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    state
                        .get("observed_url")
                        .and_then(Value::as_str)
                        .map(extract_video_id)
                        .filter(|value| !value.is_empty())
                })
                .or_else(|| {
                    state
                        .pointer("/observed_state/video_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or(current_id)
        } else {
            expected_id.clone()
        };
        if matches!(
            state.get("reason").and_then(Value::as_str),
            Some("navigation_timeout" | "navigation_failed")
        ) && !fallback_id.is_empty()
            && !target.contains("/player/v1/")
        {
            let fallback = format!("https://www.tiktok.com/player/v1/{fallback_id}");
            let navigation = self.navigate_to(&fallback).await?;
            state = self
                .wait_for_named_state(
                    "videoState",
                    wait_seconds.max(TRANSITION_TIMEOUT_S),
                    "video_id",
                    &fallback_id,
                    &navigation,
                )
                .await?;
        }
        if let Some(reason) = state_failure_reason(&state) {
            return Ok(json!({ "ok": false, "reason": reason, "state": state }));
        }
        if state.get("unavailable").and_then(Value::as_bool) == Some(true) {
            return Ok(json!({
                "ok": false,
                "reason": "video_unavailable",
                "state": state,
            }));
        }
        if !script_ok(&state) {
            return Ok(json!({
                "ok": false,
                "reason": "not_video_detail",
                "state": state,
            }));
        }
        if resolve_media
            && self
                .current_url()
                .await
                .unwrap_or_default()
                .contains("/player/v1/")
        {
            let _ = self.start_player_media().await;
        }
        let raw = self.wait_for_video_detail(wait_seconds).await?;
        let mut entity: TikTokVideo = serde_json::from_value(raw)
            .context("TikTok video detail returned an invalid entity")?;
        let mut landed_on_player = self
            .current_url()
            .await
            .unwrap_or_default()
            .contains("/player/v1/");
        let landed_id = state
            .get("video_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if entity.video_id != landed_id {
            return Ok(json!({
                "ok": false,
                "reason": "video_navigation_mismatch",
                "state": state,
            }));
        }
        if landed_on_player {
            match self
                .open_canonical_video_from_player(&entity, wait_seconds)
                .await
            {
                Ok(CanonicalVideoOpen::Opened(canonical)) => {
                    entity = merge_video_entities(*canonical, entity)?;
                }
                Ok(CanonicalVideoOpen::Failed { reason, state }) => {
                    return Ok(json!({
                        "ok": false,
                        "reason": reason,
                        "state": state,
                        "entity": entity,
                    }));
                }
                Err(error) => {
                    return Ok(json!({
                        "ok": false,
                        "reason": "video_canonical_navigation_failed",
                        "error": format!("{error:#}"),
                        "entity": entity,
                    }));
                }
            }
            landed_on_player = match self.current_url().await {
                Ok(url) => url.contains("/player/v1/"),
                Err(error) => {
                    return Ok(json!({
                        "ok": false,
                        "reason": "video_canonical_status_failed",
                        "error": format!("{error:#}"),
                        "entity": entity,
                    }));
                }
            };
        }
        if entity.author_id.trim().is_empty() {
            return Ok(json!({
                "ok": false,
                "reason": "video_detail_incomplete",
                "state": state,
                "has_author": false,
                "entity": entity,
            }));
        }
        let comments_error = if num_comments > 0 && !landed_on_player {
            match self.collect_comments(num_comments).await {
                Ok(comments) => {
                    entity.top_comments = comments;
                    None
                }
                Err(error) => Some(format!("{error:#}")),
            }
        } else {
            None
        };
        if let Some(error) = comments_error {
            return Ok(json!({
                "ok": false,
                "reason": "video_comments_failed",
                "error": error,
                "entity": entity,
            }));
        }
        let mut missing = Vec::new();
        let reported_zero_comments = entity.comments_count.trim() == "0";
        if num_comments > 0
            && !reported_zero_comments
            && (landed_on_player || entity.top_comments.is_empty())
        {
            missing.push("comments");
        }
        let has_media = entity
            .video
            .get("resolved_url")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
        if resolve_media && !has_media {
            missing.push("media");
        }
        if !missing.is_empty() {
            return Ok(json!({
                "ok": false,
                "reason": "video_detail_incomplete",
                "missing": missing,
                "entity": entity,
            }));
        }
        Ok(json!({ "ok": true, "entity": entity }))
    }

    async fn open_canonical_video_from_player(
        &self,
        entity: &TikTokVideo,
        wait_seconds: f64,
    ) -> Result<CanonicalVideoOpen> {
        // The embed player can expose the video and media while omitting the
        // author. TikTok still resolves the canonical detail for the neutral
        // `@_` handle, after which videoDetail reads the real author from the
        // page's initial state.
        let author_url = tiktok_author_url(&entity.author_id)
            .unwrap_or_else(|_| "https://www.tiktok.com/@_".to_string());
        let target = format!("{author_url}/video/{}", entity.video_id);
        let navigation = self.navigate_to(&target).await?;
        let state = self
            .wait_for_named_state(
                "videoState",
                wait_seconds.max(TRANSITION_TIMEOUT_S),
                "video_id",
                &entity.video_id,
                &navigation,
            )
            .await?;
        if let Some(reason) = state_failure_reason(&state) {
            return Ok(CanonicalVideoOpen::Failed {
                reason: reason.to_string(),
                state,
            });
        }
        if !script_ok(&state) {
            return Ok(CanonicalVideoOpen::Failed {
                reason: "not_video_detail".to_string(),
                state,
            });
        }
        let raw = self.wait_for_video_detail(wait_seconds).await?;
        let canonical: TikTokVideo = serde_json::from_value(raw)
            .context("canonical TikTok video detail returned an invalid entity")?;
        if canonical.video_id != entity.video_id {
            return Ok(CanonicalVideoOpen::Failed {
                reason: "video_navigation_mismatch".to_string(),
                state: json!({
                    "expected_video_id": entity.video_id,
                    "observed_video_id": canonical.video_id,
                    "url": self.current_url().await.unwrap_or_default(),
                }),
            });
        }
        if canonical.author_id.trim().is_empty() {
            return Ok(CanonicalVideoOpen::Failed {
                reason: "video_detail_incomplete".to_string(),
                state: json!({
                    "video_id": canonical.video_id,
                    "has_author": false,
                    "url": self.current_url().await.unwrap_or_default(),
                }),
            });
        }
        Ok(CanonicalVideoOpen::Opened(Box::new(canonical)))
    }

    async fn start_player_media(&self) -> Result<()> {
        let button = self.expect_object("playerPlayButton", None).await?;
        if button.get("found").and_then(Value::as_bool) != Some(true) {
            return Ok(());
        }
        let x = button
            .get("x")
            .and_then(Value::as_f64)
            .context("TikTok play button omitted x coordinate")?;
        let y = button
            .get("y")
            .and_then(Value::as_f64)
            .context("TikTok play button omitted y coordinate")?;
        self.page.click(x, y).await?;
        sleep_ms(750).await;
        Ok(())
    }

    async fn wait_for_video_detail(&self, wait_seconds: f64) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 12.0));
        loop {
            let latest = self.expect_object("videoDetail", None).await?;
            let has_author = latest
                .get("author_id")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty());
            let has_media = latest
                .pointer("/video/resolved_url")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty());
            if (has_author && has_media) || Instant::now() >= deadline {
                return Ok(latest);
            }
            sleep_ms(250).await;
        }
    }

    pub async fn read_author(
        &self,
        locator: &str,
        wait_seconds: f64,
        num_videos: Option<usize>,
    ) -> Result<Value> {
        let target = tiktok_author_url(locator)?;
        let expected_handle = extract_handle(&target);
        let navigation = self.navigate_to(&target).await?;
        let state = self
            .wait_for_named_state(
                "authorState",
                wait_seconds.max(TRANSITION_TIMEOUT_S),
                "handle",
                &expected_handle,
                &navigation,
            )
            .await?;
        if let Some(reason) = state_failure_reason(&state) {
            return Ok(json!({ "ok": false, "reason": reason, "state": state }));
        }
        if state.get("unavailable").and_then(Value::as_bool) == Some(true) {
            return Ok(json!({
                "ok": false,
                "reason": "author_unavailable",
                "state": state,
            }));
        }
        if !script_ok(&state) {
            return Ok(json!({
                "ok": false,
                "reason": "not_author_profile",
                "state": state,
            }));
        }

        let target_count = num_videos.unwrap_or(100).max(1);
        let first_screen_only = num_videos.is_none();
        if first_screen_only {
            self.expect_object("scrollFeed", Some(&json!({ "to_top": true })))
                .await?;
            sleep_ms(350).await;
        }
        let mut cards = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stalls = 0usize;
        let mut raw_profile = Value::Null;
        while cards.len() < target_count && stalls < 4 {
            raw_profile = self
                .expect_object(
                    "authorProfile",
                    Some(&json!({
                        "limit": target_count,
                        "viewport_only": first_screen_only,
                    })),
                )
                .await?;
            let latest_state = self.expect_object("authorState", None).await?;
            if let Some(reason) = state_failure_reason(&latest_state) {
                return Ok(json!({
                    "ok": false,
                    "reason": reason,
                    "state": latest_state,
                }));
            }
            let parsed: TikTokAuthorProfile = serde_json::from_value(raw_profile.clone())
                .context("TikTok author page returned an invalid entity")?;
            let before = cards.len();
            for card in parsed.video_cards {
                let key = if card.video_id.is_empty() {
                    card.url.clone()
                } else {
                    card.video_id.clone()
                };
                if !key.is_empty() && seen.insert(key) {
                    cards.push(card);
                }
            }
            if first_screen_only || cards.len() >= target_count {
                break;
            }
            stalls = if cards.len() == before { stalls + 1 } else { 0 };
            self.expect_object("scrollFeed", Some(&json!({ "nudge_up": false })))
                .await?;
            sleep_ms(900).await;
        }
        let mut profile: TikTokAuthorProfile = serde_json::from_value(raw_profile)
            .context("TikTok author page returned an invalid entity")?;
        let landed_handle = state
            .get("handle")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !profile.handle.eq_ignore_ascii_case(landed_handle) {
            return Ok(json!({
                "ok": false,
                "reason": "author_navigation_mismatch",
                "state": state,
            }));
        }
        if let Some(target_count) = num_videos {
            cards.truncate(target_count.max(1));
        }
        profile.video_cards = cards;
        if num_videos.is_some() && profile.video_cards.is_empty() {
            return Ok(json!({
                "ok": false,
                "reason": "author_videos_incomplete",
                "missing": ["video_cards"],
                "profile": profile,
            }));
        }
        Ok(json!({ "ok": true, "profile": profile }))
    }

    async fn wait_for_search_transition(
        &self,
        query: &str,
        timeout_seconds: f64,
        navigation: &NavigationExpectation,
    ) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_seconds.max(0.5));
        let mut latest = Value::Object(Map::new());
        let mut login_gate_since = None;
        while Instant::now() < deadline {
            latest = match self
                .expect_object("searchState", Some(&json!({ "query": query })))
                .await
            {
                Ok(state) => state,
                Err(err) => {
                    sleep_ms(200).await;
                    json!({
                        "ok": false,
                        "reason": "search_transition_in_progress",
                        "error": err.to_string(),
                    })
                }
            };
            let current = self.current_url().await.unwrap_or_default();
            let committed = navigation_committed(navigation, &current);
            if committed
                && current.starts_with("chrome-error://")
                && latest.get("ready_state").and_then(Value::as_str) == Some("complete")
            {
                return Ok(json!({
                    "ok": false,
                    "reason": "navigation_failed",
                    "query": query,
                    "observed_url": current,
                    "observed_state": latest,
                }));
            }
            if committed && is_tiktok_page_url(&current) {
                if latest.get("login_required").and_then(Value::as_bool) == Some(true) {
                    let since = login_gate_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(3) {
                        return Ok(latest);
                    }
                } else if state_terminal(&latest) {
                    return Ok(latest);
                } else {
                    login_gate_since = None;
                }
            }
            if committed && tiktok_search_matches(&current, query) && search_transition_ok(&latest)
            {
                return Ok(latest);
            }
            sleep_ms(400).await;
        }
        Ok(json!({
            "ok": false,
            "reason": "search_navigation_timeout",
            "query": query,
            "observed_url": self.current_url().await.unwrap_or_default(),
            "observed_state": latest,
        }))
    }

    async fn collect_video_cards(&self, target: usize) -> Result<Vec<TikTokVideoCard>> {
        let mut cards = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stalls = 0usize;
        while cards.len() < target && stalls < 4 {
            let before = cards.len();
            for card in self.extract_video_cards(target).await? {
                let key = if card.video_id.is_empty() {
                    card.url.clone()
                } else {
                    card.video_id.clone()
                };
                if !key.is_empty() && seen.insert(key) {
                    cards.push(card);
                }
            }
            if cards.len() >= target {
                break;
            }
            stalls = if cards.len() == before { stalls + 1 } else { 0 };
            self.expect_object("scrollFeed", Some(&json!({ "nudge_up": false })))
                .await?;
            sleep_ms(1100).await;
        }
        cards.truncate(target);
        Ok(cards)
    }

    async fn extract_video_cards(&self, limit: usize) -> Result<Vec<TikTokVideoCard>> {
        let raw = self
            .expect_array("videoCards", Some(&json!({ "limit": limit })))
            .await?;
        Ok(raw
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect())
    }

    async fn collect_comments(&self, target: usize) -> Result<Vec<TikTokComment>> {
        self.activate_comments().await?;
        let mut comments = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stalls = 0usize;
        while comments.len() < target && stalls < 4 {
            let raw = self
                .expect_array("comments", Some(&json!({ "limit": target })))
                .await?;
            let before = comments.len();
            for item in raw {
                let comment: TikTokComment = serde_json::from_value(item)
                    .context("TikTok comments returned an invalid entity")?;
                let key = if comment.comment_id.is_empty() {
                    format!("{}\n{}", comment.author, comment.text)
                } else {
                    comment.comment_id.clone()
                };
                if !comment.text.is_empty() && seen.insert(key) {
                    comments.push(comment);
                }
            }
            if comments.len() >= target {
                break;
            }
            stalls = if comments.len() == before {
                stalls + 1
            } else {
                0
            };
            self.expect_object("scrollComments", None).await?;
            sleep_ms(650).await;
        }
        comments.truncate(target);
        Ok(comments)
    }

    async fn activate_comments(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut last_open_attempt = None;
        while Instant::now() < deadline {
            let activation = self.expect_object("commentActivation", None).await?;
            if activation.get("ready").and_then(Value::as_bool) == Some(true) {
                return Ok(());
            }
            let action = activation
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let open_retry_ready = last_open_attempt
                .as_ref()
                .is_none_or(|attempt: &Instant| attempt.elapsed() >= Duration::from_secs(2));
            if activation.get("found").and_then(Value::as_bool) == Some(true)
                && (action == "dismiss_guide" || (action == "open_comments" && open_retry_ready))
            {
                let x = activation
                    .get("x")
                    .and_then(Value::as_f64)
                    .context("TikTok comment control omitted x coordinate")?;
                let y = activation
                    .get("y")
                    .and_then(Value::as_f64)
                    .context("TikTok comment control omitted y coordinate")?;
                self.page.click(x, y).await?;
                if action == "open_comments" {
                    last_open_attempt = Some(Instant::now());
                }
            }
            sleep_ms(500).await;
        }
        Ok(())
    }

    async fn navigate_to(&self, target: &str) -> Result<NavigationExpectation> {
        let current = self.current_url().await.unwrap_or_default();
        let navigated = without_fragment(&current) != without_fragment(target);
        if navigated {
            self.soft_navigate(target).await?;
        }
        Ok(NavigationExpectation {
            previous_url: current,
            navigated,
        })
    }

    async fn soft_navigate(&self, url: &str) -> Result<()> {
        let url = serde_json::to_string(url)?;
        let expr = format!(
            "window.location.assign({url}); return {{ ok: true, url: window.location.href }};"
        );
        // Chrome can unload the frame before returning the evaluate result.
        // The following state poll verifies the destination.
        let _ = self.page.evaluate_json(&expr).await;
        Ok(())
    }

    async fn wait_for_named_state(
        &self,
        name: &str,
        timeout_seconds: f64,
        identity_key: &str,
        expected_identity: &str,
        navigation: &NavigationExpectation,
    ) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_seconds.max(0.5));
        let mut latest = Value::Object(Map::new());
        let mut login_gate_since = None;
        while Instant::now() < deadline {
            latest = match self.expect_object(name, None).await {
                Ok(state) => state,
                Err(err) => {
                    sleep_ms(200).await;
                    json!({
                        "ok": false,
                        "reason": "page_transition_in_progress",
                        "error": err.to_string(),
                    })
                }
            };
            let current = self.current_url().await.unwrap_or_default();
            let committed = navigation_committed(navigation, &current);
            if committed
                && current.starts_with("chrome-error://")
                && latest.get("ready_state").and_then(Value::as_str) == Some("complete")
            {
                return Ok(json!({
                    "ok": false,
                    "reason": "navigation_failed",
                    "expected_identity": expected_identity,
                    "observed_url": current,
                    "observed_state": latest,
                }));
            }
            let identity = latest
                .get(identity_key)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let identity_matches = if expected_identity.is_empty() {
                !identity.is_empty()
            } else {
                identity.eq_ignore_ascii_case(expected_identity)
            };
            if committed && is_tiktok_page_url(&current) {
                if script_ok(&latest) && identity_matches {
                    return Ok(latest);
                }
                if latest.get("unavailable").and_then(Value::as_bool) == Some(true)
                    && latest.get("ready_state").and_then(Value::as_str) == Some("complete")
                {
                    return Ok(latest);
                }
                if latest.get("login_required").and_then(Value::as_bool) == Some(true) {
                    let since = login_gate_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(3) {
                        return Ok(latest);
                    }
                } else if state_terminal(&latest) {
                    return Ok(latest);
                } else {
                    login_gate_since = None;
                }
            }
            sleep_ms(400).await;
        }
        Ok(json!({
            "ok": false,
            "reason": "navigation_timeout",
            "expected_identity": expected_identity,
            "observed_url": self.current_url().await.unwrap_or_default(),
            "observed_state": latest,
        }))
    }

    async fn expect_object(&self, name: &str, arg: Option<&Value>) -> Result<Value> {
        let value = self.run_script(name, arg).await?;
        if value.is_object() {
            Ok(value)
        } else {
            anyhow::bail!(
                "TikTok page script {name} returned {}, expected object",
                value_type(&value)
            )
        }
    }

    async fn expect_array(&self, name: &str, arg: Option<&Value>) -> Result<Vec<Value>> {
        let value = self.run_script(name, arg).await?;
        value.as_array().cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "TikTok page script {name} returned {}, expected array",
                value_type(&value)
            )
        })
    }
}

struct NavigationExpectation {
    previous_url: String,
    navigated: bool,
}

enum CanonicalVideoOpen {
    Opened(Box<TikTokVideo>),
    Failed { reason: String, state: Value },
}

fn merge_video_entities(canonical: TikTokVideo, player: TikTokVideo) -> Result<TikTokVideo> {
    let mut canonical = serde_json::to_value(canonical)?;
    let player = serde_json::to_value(player)?;
    fill_missing_json(&mut canonical, &player);
    serde_json::from_value(canonical).context("merged TikTok video detail is invalid")
}

fn fill_missing_json(value: &mut Value, fallback: &Value) {
    match (value, fallback) {
        (Value::Object(value), Value::Object(fallback)) => {
            for (key, fallback_value) in fallback {
                match value.get_mut(key) {
                    Some(value) => fill_missing_json(value, fallback_value),
                    None => {
                        value.insert(key.clone(), fallback_value.clone());
                    }
                }
            }
        }
        (Value::Array(value), Value::Array(fallback)) => {
            for item in fallback {
                if !value.contains(item) {
                    value.push(item.clone());
                }
            }
        }
        (Value::String(value), Value::String(fallback)) if value.trim().is_empty() => {
            *value = fallback.clone();
        }
        (Value::Number(value), Value::Number(fallback))
            if number_is_zero(value) && !number_is_zero(fallback) =>
        {
            *value = fallback.clone();
        }
        (value @ Value::Null, fallback) => *value = fallback.clone(),
        _ => {}
    }
}

fn number_is_zero(value: &serde_json::Number) -> bool {
    value.as_i64() == Some(0) || value.as_u64() == Some(0) || value.as_f64() == Some(0.0)
}

fn is_tiktok_page(value: &str) -> bool {
    is_tiktok_page_url(value)
}

fn percent_encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn search_transition_ok(value: &Value) -> bool {
    !value
        .get("blank_or_throttled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && value
            .get("query_visible")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        && (value.get("card_count").and_then(Value::as_u64).unwrap_or(0) > 0
            || value
                .get("has_no_results")
                .and_then(Value::as_bool)
                .unwrap_or(false))
}

fn tiktok_search_matches(value: &str, query: &str) -> bool {
    let Some(url) = reqwest::Url::parse(value).ok().filter(|url| {
        is_tiktok_page_url(url.as_str()) && url.path().trim_end_matches('/') == "/search"
    }) else {
        return false;
    };
    url.query_pairs()
        .any(|(key, value)| key == "q" && value == query)
}

fn navigation_committed(navigation: &NavigationExpectation, current: &str) -> bool {
    !navigation.navigated || without_fragment(current) != without_fragment(&navigation.previous_url)
}

fn without_fragment(value: &str) -> &str {
    value.split('#').next().unwrap_or(value)
}

fn state_terminal(value: &Value) -> bool {
    value.get("login_required").and_then(Value::as_bool) == Some(true)
        || value.get("challenge_required").and_then(Value::as_bool) == Some(true)
        || value.get("reason").and_then(Value::as_str) == Some("search_unavailable")
}

fn state_failure_reason(value: &Value) -> Option<&str> {
    if value.get("challenge_required").and_then(Value::as_bool) == Some(true) {
        Some("challenge_required")
    } else if value.get("login_required").and_then(Value::as_bool) == Some(true) {
        Some("login_required")
    } else {
        value
            .get("reason")
            .and_then(Value::as_str)
            .filter(|reason| {
                matches!(
                    *reason,
                    "navigation_timeout"
                        | "navigation_failed"
                        | "search_navigation_timeout"
                        | "search_unavailable"
                )
            })
    }
}

fn script_ok(value: &Value) -> bool {
    value.get("ok").and_then(Value::as_bool).unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_video_entities_preserves_complementary_player_media() {
        let canonical = TikTokVideo {
            video_id: "123".into(),
            author_id: "creator".into(),
            duration_seconds: 0,
            video: json!({
                "resolved_url": "https://canonical.example/video.mp4",
                "source_urls": ["https://canonical.example/video.mp4"],
                "candidates": [{"url": "https://canonical.example/video.mp4"}],
            }),
            ..TikTokVideo::default()
        };
        let player = TikTokVideo {
            video_id: "123".into(),
            author_id: "creator".into(),
            duration_seconds: 18,
            video: json!({
                "resolved_url": "https://player.example/video.mp4",
                "source_urls": [
                    "https://canonical.example/video.mp4",
                    "https://player.example/video.mp4"
                ],
                "candidates": [{"url": "https://player.example/video.mp4"}],
            }),
            ..TikTokVideo::default()
        };

        let merged = merge_video_entities(canonical, player).unwrap();

        assert_eq!(merged.duration_seconds, 18);
        assert_eq!(
            merged.video["resolved_url"],
            "https://canonical.example/video.mp4"
        );
        assert_eq!(merged.video["source_urls"].as_array().unwrap().len(), 2);
        assert_eq!(merged.video["candidates"].as_array().unwrap().len(), 2);
    }
}
