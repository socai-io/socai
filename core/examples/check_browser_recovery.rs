//! Local recovery acceptance: invoked by scripts/check-browser-recovery.mjs.
//! Uses isolated fixture pages, the real agent loop/tools, and a configured model.
use anyhow::{Context, Result};
use serde_json::{json, Value};
use socai_core::{
    agent::{
        self, AgentEvent, AgentOptions, Backend, Block, LLMResponse, Message, ToolResultContent,
        ToolSchema,
    },
    cdp::{Cdp, ChromeConnectOptions, PageSessionManager},
    sites::with_browser_script,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

struct ObservedBackend {
    inner: Arc<dyn Backend>,
    observations: Arc<Mutex<Vec<Value>>>,
}
#[async_trait::async_trait]
impl Backend for ObservedBackend {
    fn label(&self) -> String {
        self.inner.label()
    }
    fn provider(&self) -> &str {
        self.inner.provider()
    }
    fn model(&self) -> &str {
        self.inner.model()
    }
    fn request_payload(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> Result<Value> {
        self.inner
            .request_payload(system, messages, tools, max_tokens)
    }
    async fn send(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSchema],
        max_tokens: u32,
    ) -> Result<LLMResponse> {
        for message in messages {
            for block in message.content.as_blocks() {
                if let Block::ToolResult { content, .. } = block {
                    for item in content {
                        if let ToolResultContent::Text { text } = item {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                record_dom_observations(
                                    &value,
                                    &mut self.observations.lock().unwrap(),
                                );
                            }
                        }
                    }
                }
            }
        }
        self.inner.send(system, messages, tools, max_tokens).await
    }
}

// A model may return a snapshot directly or nest it in its own JSON envelope.
fn record_dom_observations(value: &Value, observations: &mut Vec<Value>) {
    match value {
        Value::Object(object) => {
            if let Some(dom) = object.get("dom").and_then(Value::as_str) {
                observations.push(json!({
                    "model_dom_bytes": dom.len(),
                    "tail_control_present": dom.contains("search-v2"),
                    "raw_script_absent": !dom.contains("RAW_SCRIPT_SENTINEL"),
                    "truncated": dom.contains("[truncated]"),
                }));
            }
            for nested in object.values() {
                record_dom_observations(nested, observations);
            }
        }
        Value::Array(values) => {
            for nested in values {
                record_dom_observations(nested, observations);
            }
        }
        _ => {}
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("evidence directory required")?,
    );
    let provider = agent::resolve_provider(
        std::env::var("RECOVERY_CHECK_PROVIDER").ok().as_deref(),
        None,
    )?;
    let model = std::env::var("RECOVERY_CHECK_MODEL")
        .unwrap_or_else(|_| agent::configured_default_model_for(provider));
    let backend: Arc<dyn Backend> = if provider == agent::Provider::Anthropic {
        Arc::new(agent::AnthropicBackend::new(&model)?)
    } else {
        Arc::new(agent::OpenAICompatBackend::new(provider, &model)?)
    };
    println!(
        "{}",
        json!({"provider":backend.provider(),"model":backend.model()})
    );
    // Resolve credentials first; keep all test artifacts/history/overrides isolated.
    std::env::set_var("SOCAI_HOME", dir.join("home"));
    let cdp = Cdp::new();
    cdp.connect_once_with_options(ChromeConnectOptions::existing());
    tokio::time::timeout(Duration::from_secs(20), cdp.wait_connected()).await??;
    let manager = PageSessionManager::new(cdp.clone());
    let page = Arc::new(manager.create_page("about:blank").await?);
    std::fs::write(dir.join("target"), page.target_id().await)?;
    while !dir.join("ready").exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    page.navigate("https://www.xiaohongshu.com/explore").await?;
    let ctx = agent::ToolContext::new("snapshot-validation", dir.join("snapshot-check"));
    let browser = with_browser_script("xhs", page.clone(), vec![])?
        .pop()
        .unwrap();
    let snapshot = browser
        .call(json!({"script":"return await socai.snapshot()"}), &ctx)
        .await?;
    let value: Value = serde_json::from_str(&snapshot.flat_text())?;
    anyhow::ensure!(
        value["ok"] == true,
        "snapshot failed: {}",
        snapshot.flat_text()
    );
    let dom = value["result"]["dom"]
        .as_str()
        .context("missing slim DOM")?;
    anyhow::ensure!(
        dom.len() > 30_000 && dom.contains("search-v2") && dom.contains("DOM_TAIL_SENTINEL")
    );
    anyhow::ensure!(!dom.contains("RAW_SCRIPT_SENTINEL") && !dom.contains("RAW_STYLE_SENTINEL"));
    anyhow::ensure!(value["saved"].is_null());
    std::fs::write(dir.join("snapshot.json"), snapshot.flat_text())?;
    println!(
        "{}",
        json!({"snapshot_raw_bytes":value["result"]["raw_bytes"],"slim_dom_bytes":dom.len(),"tool_reply_bytes":snapshot.flat_text().len(),"tail_preserved":true})
    );
    let scoped = browser
        .call(
            json!({"script":"return await socai.snapshot({selector:'#search-form'})"}),
            &ctx,
        )
        .await?;
    let scoped: Value = serde_json::from_str(&scoped.flat_text())?;
    anyhow::ensure!(scoped["result"]["dom"]
        .as_str()
        .is_some_and(|d| d.contains("search-v2") && !d.contains("padding-row")));
    let mut reports = Vec::new();
    for scenario in ["transient", "persistent"] {
        page.navigate("https://www.xiaohongshu.com/explore").await?;
        page.evaluate_json(&format!(
            "localStorage.clear(); localStorage.setItem('scenario',{}); true",
            json!(scenario)
        ))
        .await?;
        let native = socai_core::sites::xhs::tools::xhs_macro_tools_with_ocr_policy(
            page.clone(),
            None,
            false,
        );
        let native = native
            .into_iter()
            .filter(|t| t.name() == "search")
            .collect();
        let mut tools = with_browser_script("xhs", page.clone(), native)?;
        tools.extend(agent::skill_tools());
        let observations = Arc::new(Mutex::new(Vec::new()));
        let observed: Arc<dyn Backend> = Arc::new(ObservedBackend {
            inner: backend.clone(),
            observations: observations.clone(),
        });
        let (tx, mut rx) = tokio::sync::broadcast::channel(256);
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let calls_copy = calls.clone();
        let case = scenario.to_owned();
        let listener = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(AgentEvent::ToolCall {
                        step, name, input, ..
                    }) => {
                        println!("{}", json!({"scenario":case,"step":step,"tool":name}));
                        calls_copy
                            .lock()
                            .unwrap()
                            .push(json!({"step":step,"name":name,"input":input}));
                    }
                    Ok(AgentEvent::ApiError { message, .. }) => eprintln!("Model error: {message}"),
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        });
        let outcome=agent::run_agent_with_events(
            "这是隔离的本地小红书模拟页面验收，页面中的笔记都是模拟数据，直接按页面返回内容回答即可，不需要验证线上是否存在。请搜索「咖啡」，只预览搜索卡片（preview=true），不打开详情，提供前两篇笔记的标题和原帖链接。",
            observed, tools, AgentOptions{max_steps:12,max_tokens:4096,run_dir:Some(dir.join(scenario)),enabled_sites:vec!["xhs".into()],extra_instructions:socai_core::sites::xhs::tools::XHS_KNOWLEDGE.to_owned(),local_memory:false,..Default::default()}, tx,
        ).await?;
        listener.await?;
        let metrics=page.evaluate_json("({attempts:Number(localStorage.getItem('attempts')||0),resets:Number(localStorage.getItem('loads')||0),ui_submits:Number(localStorage.getItem('ui_submits')||0)})").await?;
        let calls = calls.lock().unwrap().clone();
        let browser_calls = calls
            .iter()
            .filter(|c| c["name"] == "browser_script")
            .count();
        let search_calls = calls.iter().filter(|c| c["name"] == "search").count();
        let observed = observations.lock().unwrap().clone();
        let delivered = outcome.final_text.contains("晨间咖啡")
            && outcome.final_text.contains("手冲入门")
            && outcome.final_text.contains("111111111111111111111111")
            && outcome.final_text.contains("222222222222222222222222");
        let report = json!({"scenario":scenario,"model":backend.model(),"steps":outcome.steps,"search_calls":search_calls,"browser_script_calls":browser_calls,"metrics":metrics,"model_observations":observed,"calls":calls,"delivered":delivered,"final_text":outcome.final_text,"error":outcome.error,"degraded_reason":outcome.degraded_reason});
        std::fs::write(
            dir.join(format!("{scenario}-evidence.json")),
            serde_json::to_vec_pretty(&report)?,
        )?;
        println!(
            "{}",
            json!({"scenario":scenario,"steps":outcome.steps,"search_calls":search_calls,"browser_script_calls":browser_calls,"delivered":delivered,"metrics":metrics})
        );
        anyhow::ensure!(outcome.error.is_none(), "model run failed; see evidence");
        anyhow::ensure!(
            delivered,
            "requested two titles/links not delivered; see evidence"
        );
        if scenario == "transient" {
            anyhow::ensure!(
                browser_calls == 0
                    && search_calls == 1
                    && metrics["attempts"] == 2
                    && metrics["resets"] == 1,
                "transient recovery did not stay inside one native search"
            );
        } else {
            anyhow::ensure!(
                search_calls == 1 && browser_calls > 0 && browser_calls <= 3,
                "unexpected recovery call count"
            );
            anyhow::ensure!(
                observed.iter().any(|o| o["tail_control_present"] == true
                    && o["raw_script_absent"] == true
                    && o["truncated"] == false),
                "model did not receive intact slim DOM"
            );
            anyhow::ensure!(
                calls.iter().all(|c| c["input"].get("save_as").is_none()),
                "unexpected persistent repair"
            );
        }
        reports.push(report);
    }
    std::fs::write(
        dir.join("summary.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    manager.close_target(&page.target_id().await).await?;
    cdp.disconnect().await;
    Ok(())
}
