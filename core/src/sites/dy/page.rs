use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::cdp::PageSession;
use crate::sites::dy::entities::{
    douyin_author_url, douyin_video_url, extract_author_id, extract_video_id, is_douyin_page_url,
    DouyinAuthorProfile, DouyinComment, DouyinVideo, DouyinVideoCard,
};

pub const DOUYIN_HOME_URL: &str = "https://www.douyin.com/";

const PAGE_SCRIPTS_JS: &str = include_str!("page_scripts.js");
const DOUYIN_PAGE_SCRIPT_FUNCTIONS: &[&str] = &[
    "pageState",
    "searchInput",
    "setSearchInput",
    "searchState",
    "videoCards",
    "scrollFeed",
    "videoState",
    "videoDetail",
    "comments",
    "scrollComments",
    "authorState",
    "authorProfile",
];
const SEARCH_TRANSITION_TIMEOUT_S: f64 = 20.0;

pub struct DouyinPageRuntime<'a> {
    page: &'a PageSession,
}

impl<'a> DouyinPageRuntime<'a> {
    pub fn new(page: &'a PageSession) -> Self {
        Self { page }
    }

    pub async fn run_script(&self, name: &str, arg: Option<&Value>) -> Result<Value> {
        if !DOUYIN_PAGE_SCRIPT_FUNCTIONS.contains(&name) {
            anyhow::bail!("Unknown Douyin page script: {name}");
        }
        let args = match arg {
            None => String::new(),
            Some(v) => serde_json::to_string(v)?,
        };
        let expr = format!(
            "(function() {{\n{PAGE_SCRIPTS_JS}\n// SOCAI_DOUYIN_CALL: {name}\nreturn SocaiDouyinPageScripts.{name}({args});\n}})()"
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

    pub async fn ensure_douyin(
        &self,
        navigate_if_needed: bool,
        _timeout_seconds: f64,
    ) -> Result<()> {
        let url = self.current_url().await.unwrap_or_default();
        if is_douyin_page_url(&url) {
            return Ok(());
        }
        if navigate_if_needed {
            self.soft_navigate(DOUYIN_HOME_URL).await?;
            return Ok(());
        }
        anyhow::bail!(
            "Current page is not Douyin: {}",
            if url.is_empty() { "unknown" } else { &url }
        );
    }

    pub async fn wait_until_interactive(&self, timeout_seconds: f64) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_seconds.max(0.5));
        let mut latest = json!({
            "ok": false,
            "blank_or_throttled": true,
            "reason": "waiting_for_douyin",
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
            let terminal_gate = latest
                .get("challenge_required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || latest
                    .get("login_required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            if terminal_gate
                || !latest
                    .get("blank_or_throttled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            {
                return Ok(latest);
            }
            sleep_ms(1000).await;
        }
        Ok(latest)
    }

    async fn soft_navigate(&self, url: &str) -> Result<()> {
        let url = serde_json::to_string(url)?;
        let expr = format!(
            "window.location.assign({url}); return {{ ok: true, url: window.location.href }};"
        );
        match self.page.evaluate_json(&expr).await {
            Ok(_) => Ok(()),
            // The page may start unloading before Chrome returns the evaluate
            // result. Treat that as a successful navigation trigger; the
            // polling path will verify where we actually landed.
            Err(_) => Ok(()),
        }
    }

    pub async fn detect_state(&self) -> Result<Value> {
        self.ensure_douyin(false, 0.0).await?;
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
        self.ensure_douyin(true, wait_seconds.max(330.0)).await?;
        let initial = self
            .wait_until_interactive(wait_seconds.max(SEARCH_TRANSITION_TIMEOUT_S))
            .await?;
        if let Some(reason) = state_failure_reason(&initial) {
            return Ok(json!({
                "ok": false,
                "query": keyword,
                "reason": reason,
                "state": initial,
                "count": 0,
                "cards": [],
            }));
        }
        if initial
            .get("blank_or_throttled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(json!({
                "ok": false,
                "query": keyword,
                "reason": "blank_or_throttled",
                "state": initial,
                "count": 0,
                "cards": [],
            }));
        }
        let submit = self.submit_search(keyword, wait_seconds).await?;
        let ok = script_ok(&submit);
        let cards = if ok {
            self.collect_video_cards(num_videos.max(1)).await?
        } else {
            Vec::new()
        };
        Ok(json!({
            "ok": ok,
            "query": keyword,
            "submit": submit,
            "url": self.current_url().await?,
            "count": cards.len(),
            "cards": cards,
            "reason": if ok { "" } else { submit.get("error").and_then(Value::as_str).unwrap_or("search_submit_failed") },
        }))
    }

    pub async fn read_video(
        &self,
        locator: &str,
        wait_seconds: f64,
        num_comments: usize,
        require_audio: bool,
    ) -> Result<Value> {
        let target = douyin_video_url(locator)?;
        let expected_id = extract_video_id(&target);
        let navigation = self.navigate_to(&target).await?;
        let state = self
            .wait_for_named_state(
                "videoState",
                wait_seconds.max(SEARCH_TRANSITION_TIMEOUT_S),
                "video_id",
                &expected_id,
                &navigation,
            )
            .await?;
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

        let raw = self
            .wait_for_video_detail(wait_seconds, require_audio)
            .await?;
        let mut entity: DouyinVideo = serde_json::from_value(raw)
            .context("Douyin video detail returned an invalid entity")?;
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
        let comments_error = if num_comments > 0 {
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
        let reported_zero_comments = entity.comments_count.trim() == "0";
        if num_comments > 0 && !reported_zero_comments && entity.top_comments.is_empty() {
            return Ok(json!({
                "ok": false,
                "reason": "video_detail_incomplete",
                "missing": ["comments"],
                "entity": entity,
            }));
        }
        Ok(json!({ "ok": true, "entity": entity }))
    }

    async fn wait_for_video_detail(&self, wait_seconds: f64, require_audio: bool) -> Result<Value> {
        // The description shell commonly appears several seconds before the
        // author block and player sources hydrate. Keep this bounded, but give
        // the real detail page enough time to complete those fields.
        let deadline = Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(1.0, 12.0));
        let mut media_ready_since = None;
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
            let has_audio = latest
                .pointer("/video/audio_url")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty());
            if has_author && has_media {
                if !require_audio || has_audio {
                    return Ok(latest);
                }
                let since = media_ready_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(3) {
                    return Ok(latest);
                }
            } else {
                media_ready_since = None;
            }
            if Instant::now() >= deadline {
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
        let target = douyin_author_url(locator)?;
        let expected_id = extract_author_id(&target);
        let navigation = self.navigate_to(&target).await?;
        let state = self
            .wait_for_named_state(
                "authorState",
                wait_seconds.max(SEARCH_TRANSITION_TIMEOUT_S),
                "author_id",
                &expected_id,
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
        let content_deadline =
            Instant::now() + Duration::from_secs_f64(wait_seconds.clamp(5.0, 30.0));
        let mut profile_reports_posts = false;
        while cards.len() < target_count {
            raw_profile = self
                .expect_object(
                    "authorProfile",
                    Some(&json!({
                        "limit": target_count,
                        "viewport_only": first_screen_only,
                    })),
                )
                .await?;
            let parsed: DouyinAuthorProfile = serde_json::from_value(raw_profile.clone())
                .context("Douyin author page returned an invalid entity")?;
            let video_count = parsed.video_count.trim();
            let profile_count_known = !video_count.is_empty();
            profile_reports_posts |= profile_count_known && video_count != "0";
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
            if (first_screen_only && !cards.is_empty()) || cards.len() >= target_count {
                break;
            }
            if cards.len() == before {
                stalls += 1;
            } else {
                stalls = 0;
            }
            let settled_without_cards =
                profile_count_known && !profile_reports_posts && stalls >= 4;
            let pagination_stalled = !cards.is_empty() && stalls >= 4;
            if Instant::now() >= content_deadline || settled_without_cards || pagination_stalled {
                break;
            }
            // The profile header and work count hydrate before the post list.
            // Scrolling while the list is still empty can land on the footer
            // and leave the virtualized grid unmounted, so wait in place for
            // the first real card before starting pagination.
            if cards.is_empty() {
                sleep_ms(1000).await;
                continue;
            }
            self.expect_object("scrollFeed", Some(&json!({ "nudge_up": false })))
                .await?;
            sleep_ms(900).await;
        }
        let mut profile: DouyinAuthorProfile = serde_json::from_value(raw_profile)
            .context("Douyin author page returned an invalid entity")?;
        let landed_id = state
            .get("author_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if profile.author_id != landed_id {
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
        if profile_reports_posts && profile.video_cards.is_empty() {
            return Ok(json!({
                "ok": false,
                "reason": "author_videos_not_loaded",
                "profile": profile,
                "state": state,
            }));
        }
        Ok(json!({
            "ok": true,
            "profile": profile,
        }))
    }

    async fn submit_search(&self, query: &str, wait_seconds: f64) -> Result<Value> {
        // Douyin currently redirects synthetic Enter presses on the home-page
        // search box back to /jingxuan in some otherwise healthy sessions. The
        // canonical search route is both what the web UI opens and a more
        // reliable first strategy. Keep the input path below as a fallback for
        // page variants that reject direct navigation.
        let direct_url = douyin_search_url(query)?;
        self.soft_navigate(&direct_url).await?;
        let direct_state = self
            .wait_for_search_transition(query, wait_seconds.max(SEARCH_TRANSITION_TIMEOUT_S))
            .await?;
        if search_transition_ok(&direct_state) {
            return Ok(json!({
                "ok": true,
                "strategy": "direct_search_url",
                "state": direct_state,
                "url": self.current_url().await?,
            }));
        }
        if state_failure_reason(&direct_state).is_some() {
            return Ok(json!({
                "ok": false,
                "strategy": "direct_search_url",
                "state": direct_state,
                "url": self.current_url().await?,
                "error": direct_state
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("search_navigation_failed"),
            }));
        }

        let loc = self.expect_object("searchInput", None).await?;
        if !script_ok(&loc) {
            return Ok(json!({
                "ok": false,
                "strategy": "search_input_unavailable",
                "error": loc.get("error").and_then(Value::as_str).unwrap_or_default(),
                "state": loc,
            }));
        }
        if let Some(input) = loc.get("input") {
            self.page
                .click(number(input, "x"), number(input, "y"))
                .await?;
            sleep_ms(150).await;
        }
        let set = self
            .expect_object("setSearchInput", Some(&json!({ "query": query })))
            .await?;
        if !script_ok(&set) {
            return Ok(json!({
                "ok": false,
                "strategy": "set_search_input_failed",
                "error": set.get("error").and_then(Value::as_str).unwrap_or_default(),
                "state": set,
            }));
        }

        self.page.press_key("Enter").await?;
        let state = self
            .wait_for_search_transition(query, wait_seconds.max(SEARCH_TRANSITION_TIMEOUT_S))
            .await?;
        if search_transition_ok(&state) {
            return Ok(json!({
                "ok": true,
                "strategy": "input_enter",
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
                if search_transition_ok(&state) {
                    return Ok(json!({
                        "ok": true,
                        "strategy": "search_button_click",
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
            "error": if state.get("challenge_required").and_then(Value::as_bool).unwrap_or(false) {
                "challenge_required"
            } else if state.get("login_required").and_then(Value::as_bool).unwrap_or(false) {
                "login_required"
            } else if state.get("blank_or_throttled").and_then(Value::as_bool).unwrap_or(false) {
                "blank_or_throttled"
            } else {
                "Search did not transition to a Douyin result page"
            },
        }))
    }

    async fn wait_for_search_transition(&self, query: &str, timeout_s: f64) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_s.max(0.5));
        let mut latest = Value::Object(Map::new());
        let mut settled_elsewhere = 0usize;
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
            if search_transition_ok(&latest) || state_failure_reason(&latest).is_some() {
                return Ok(latest);
            }
            let settled = latest.get("ready_state").and_then(Value::as_str) == Some("complete")
                && !latest
                    .get("blank_or_throttled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                && !latest
                    .get("query_in_url")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            if settled {
                settled_elsewhere += 1;
                if settled_elsewhere >= 10 {
                    return Ok(json!({
                        "ok": false,
                        "reason": "search_navigation_rejected",
                        "query": query,
                        "observed_url": self.current_url().await.unwrap_or_default(),
                        "observed_state": latest,
                    }));
                }
            } else {
                settled_elsewhere = 0;
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

    async fn extract_video_cards(&self, limit: usize) -> Result<Vec<DouyinVideoCard>> {
        let raw = self
            .expect_array("videoCards", Some(&json!({ "limit": limit })))
            .await?;
        Ok(raw
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect())
    }

    async fn collect_video_cards(&self, target: usize) -> Result<Vec<DouyinVideoCard>> {
        const MAX_STALLS: usize = 4;
        let mut cards = self.extract_video_cards(target).await?;
        let mut stalls = 0usize;
        while cards.len() < target && stalls < MAX_STALLS {
            let before = cards.len();
            self.expect_object("scrollFeed", Some(&json!({ "nudge_up": false })))
                .await?;
            sleep_ms(1200).await;
            cards = self.extract_video_cards(target).await?;
            if cards.len() <= before {
                self.expect_object("scrollFeed", Some(&json!({ "nudge_up": true })))
                    .await?;
                sleep_ms(700).await;
                cards = self.extract_video_cards(target).await?;
            }
            if cards.len() <= before {
                stalls += 1;
            } else {
                stalls = 0;
            }
        }
        cards.truncate(target);
        Ok(cards)
    }

    async fn collect_comments(&self, target: usize) -> Result<Vec<DouyinComment>> {
        let mut comments = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stalls = 0usize;
        while comments.len() < target && stalls < 4 {
            let raw = self
                .expect_array("comments", Some(&json!({ "limit": target })))
                .await?;
            let before = comments.len();
            for item in raw {
                let comment: DouyinComment = serde_json::from_value(item)
                    .context("Douyin comments returned an invalid entity")?;
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
            if comments.len() == before {
                stalls += 1;
            } else {
                stalls = 0;
            }
            self.expect_object("scrollComments", None).await?;
            sleep_ms(650).await;
        }
        comments.truncate(target);
        Ok(comments)
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
            let committed = !navigation.navigated
                || without_fragment(&current) != without_fragment(&navigation.previous_url);
            let identity = latest
                .get(identity_key)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let identity_matches = if expected_identity.is_empty() {
                !identity.is_empty()
            } else {
                identity == expected_identity
            };
            if committed && is_douyin_page_url(&current) {
                if script_ok(&latest) && identity_matches {
                    return Ok(latest);
                }
                if latest.get("unavailable").and_then(Value::as_bool) == Some(true) {
                    return Ok(latest);
                }
                match state_failure_reason(&latest) {
                    Some("login_required") => {
                        let since = login_gate_since.get_or_insert_with(Instant::now);
                        // Douyin briefly paints its login panel while a public
                        // detail route hydrates. Treat it as terminal only when
                        // it remains stable across several state polls.
                        if since.elapsed() >= Duration::from_secs(3) {
                            return Ok(latest);
                        }
                    }
                    Some(_) => return Ok(latest),
                    None => login_gate_since = None,
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
                "Douyin page script {name} returned {}, expected object",
                value_type(&value)
            );
        }
    }

    async fn expect_array(&self, name: &str, arg: Option<&Value>) -> Result<Vec<Value>> {
        let value = self.run_script(name, arg).await?;
        value.as_array().cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "Douyin page script {name} returned {}, expected array",
                value_type(&value)
            )
        })
    }
}

fn douyin_search_url(query: &str) -> Result<String> {
    let mut url = reqwest::Url::parse("https://www.douyin.com/search")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Douyin search URL cannot be a base URL"))?
        .push(query);
    url.query_pairs_mut().append_pair("type", "general");
    Ok(url.to_string())
}

struct NavigationExpectation {
    previous_url: String,
    navigated: bool,
}

fn search_transition_ok(value: &Value) -> bool {
    !value
        .get("blank_or_throttled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && value
            .get("query_in_url")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        && (value.get("card_count").and_then(Value::as_u64).unwrap_or(0) > 0
            || value
                .get("has_no_results")
                .and_then(Value::as_bool)
                .unwrap_or(false))
}

fn without_fragment(value: &str) -> &str {
    value.split('#').next().unwrap_or(value)
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn script_ok(value: &Value) -> bool {
    value.get("ok").and_then(Value::as_bool).unwrap_or(false)
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
            .filter(|reason| matches!(*reason, "navigation_timeout" | "search_navigation_timeout"))
    }
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
