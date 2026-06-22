use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::cdp::PageSession;
use crate::sites::dy::entities::DouyinVideoCard;

pub const DOUYIN_HOME_URL: &str = "https://www.douyin.com/";

const PAGE_SCRIPTS_JS: &str = include_str!("page_scripts.js");
const DOUYIN_PAGE_SCRIPT_FUNCTIONS: &[&str] = &[
    "pageState",
    "searchInput",
    "setSearchInput",
    "searchState",
    "videoCards",
    "scrollFeed",
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
        if url.contains("douyin.com") {
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
            if !latest
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

    async fn submit_search(&self, query: &str, wait_seconds: f64) -> Result<Value> {
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
            "error": if state.get("blank_or_throttled").and_then(Value::as_bool).unwrap_or(false) {
                "blank_or_throttled"
            } else if state.get("login_required").and_then(Value::as_bool).unwrap_or(false) {
                "login_required"
            } else {
                "Search did not transition to a Douyin result page"
            },
        }))
    }

    async fn wait_for_search_transition(&self, query: &str, timeout_s: f64) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_s.max(0.5));
        let mut latest = Value::Object(Map::new());
        while Instant::now() < deadline {
            latest = self
                .expect_object("searchState", Some(&json!({ "query": query })))
                .await?;
            if search_transition_ok(&latest) {
                return Ok(latest);
            }
            sleep_ms(400).await;
        }
        Ok(latest)
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

fn search_transition_ok(value: &Value) -> bool {
    value
        .get("blank_or_throttled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        == false
        && (value.get("card_count").and_then(Value::as_u64).unwrap_or(0) > 0
            || value
                .get("has_no_results")
                .and_then(Value::as_bool)
                .unwrap_or(false))
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
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
