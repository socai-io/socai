use crate::artifact_tool::PublishArtifactTool;
use crate::tasks::{app_data_dir, now_ms, AgentTaskRegistry, AgentTaskSnapshot};
use crate::telemetry::{duration_ms, short_error, DesktopTelemetry};
use crate::timeline::{agent_event_to_timeline, AgentTaskEventKind, AgentTaskEventPayload};
use anyhow::Result;
use calamine::{Data, DataRef, Reader, SheetType, Xlsx};
use quick_xml::{events::Event as XmlEvent, Reader as XmlReader};
use serde_json::{json, Map, Value};
use socai_core::agent::{
    catalog_models_for, configured_default_model_for, configured_default_provider,
    desktop_agent_tools, load_api_key, make_run_dir, mark_agent_run_status,
    provider_credential_kind, resolve_provider, save_default_model, AgentEvent, Conversation,
    CredentialKind, ModelCatalogEntry, Provider, TokenUsage,
};
use socai_core::runtime::{
    create_llm_provider_for_task, ensure_llm_provider_configured_for,
    run_agent_task as run_agent_with_tools, AgentRunConfig, BrowserBusy, BrowserBusyKind,
    BrowserLease, BrowserStatus, ChromeConnectOptions, ChromeProfile, RuntimePageSession,
    SocaiRuntime,
};
use socai_core::sites::xhs::XhsHistoryStore;
use socai_core::sites::{find_site, SiteSpec};
use socai_core::telemetry::query_text_enabled;
use socai_core::telemetry::tool_call::{summarize_tool_args, summarize_tool_result};
use socai_core::telemetry::trace::mark_run_trace_status;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

const TAURI_AGENT_PREAMBLE: &str =
    "You are running inside the socai desktop app as a conversational, multi-turn agent. \
     Besides the selected content-platform site tools you have unrestricted local environment tools: \
     `read_file` (read text, or view image/screenshot files) and `shell` (PowerShell on \
     Windows, `sh` on macOS/Linux; use absolute paths to work outside the current run \
     directory). Only access files relevant to the user's request. Maintain continuity \
     with earlier turns in this chat.";

// Appended AFTER the site playbook so it sits at the tail of the system
// prompt — the position models weight most (prepended into the preamble,
// qwen3.7-max ignored it). Desktop-only, not shared site knowledge: only the
// app renders `note:` links (as rich note pills); in the TUI's plain-markdown
// answer they would be dead links.
const TAURI_CITATION_RULES: &str = "\n\n## Citing notes in the final answer (required)\n\
    The app renders note citations as rich note cards. In your final answer, \
    every time you mention a specific note you read (including cached notes \
    returned by scans), cite it inline as a markdown link — \
    [<note title>](note:<note_id>) — using the exact note_id from tool results.\n\
    Example: 推荐 [湾区遛娃|坐小火车喂羊驼](note:65f0a1b2000000000c030d1e) 的路线。\n\
    - Link text is the note's title; drop any square brackets inside it.\n\
    - For notes only seen as preview cards and never read, link their url instead.\n\
    - Cite each note where it is discussed, not in a separate list at the end.";

const TAURI_ARTIFACT_RULES: &str = "\n\n## Deliverable files\n\
    After you create and verify any file the user should download, call \
    `publish_artifact` with that file's path. Creating or mentioning a file \
    alone does not display a download card. Only tell the user the file is \
    downloadable after `publish_artifact` succeeds.";

const DEFAULT_APP_SITE_ID: &str = "xhs";

fn app_site_id_for_intent(message: &str, fallback: Option<&str>) -> &'static str {
    const DOUYIN_MARKERS: &[&str] = &["抖音", "douyin.com", "v.douyin.com", "douyin"];
    const XHS_MARKERS: &[&str] = &["小红书", "xiaohongshu.com", "xhslink.com", "rednote", "xhs"];

    let message = message.to_lowercase();
    let first_match = |markers: &[&str]| {
        markers
            .iter()
            .filter_map(|marker| message.find(marker))
            .min()
    };
    match (first_match(DOUYIN_MARKERS), first_match(XHS_MARKERS)) {
        (Some(douyin), Some(xhs)) if douyin < xhs => "dy",
        (Some(_), Some(_)) => "xhs",
        (Some(_), None) => "dy",
        (None, Some(_)) => "xhs",
        (None, None) if fallback == Some("dy") => "dy",
        (None, None) => DEFAULT_APP_SITE_ID,
    }
}

fn app_site_for_intent(message: &str, fallback: Option<&str>) -> Result<&'static SiteSpec> {
    let site_id = app_site_id_for_intent(message, fallback);
    find_site(site_id).ok_or_else(|| anyhow::anyhow!("desktop site {site_id} is not registered"))
}

/// Conversation tabs start blank. The selected site tool owns navigation once
/// the agent has interpreted the user's request.
fn app_site_start_url() -> &'static str {
    ""
}

fn tauri_citation_rules(site_id: &str) -> &'static str {
    if site_id == "xhs" {
        TAURI_CITATION_RULES
    } else {
        ""
    }
}

// ── CDP connect tests (existing) ───────────────────────────────────────────

#[tauri::command]
pub async fn cdp_connect(
    runtime: State<'_, SocaiRuntime>,
    telemetry: State<'_, DesktopTelemetry>,
) -> Result<(), String> {
    let profile = ChromeConnectOptions::from_config()
        .map(|options| options.profile.as_str())
        .unwrap_or("unknown");
    runtime.connect_browser_once();
    telemetry.capture(
        "socai_browser_connect",
        json!({ "outcome": "requested", "browser_profile": profile }),
    );
    Ok(())
}

#[tauri::command]
pub async fn cdp_disconnect(runtime: State<'_, SocaiRuntime>) -> Result<(), String> {
    // Close any legacy shared site pages before tearing down the WS. The
    // desktop task runner now uses short-lived tabs, but this keeps old
    // tool/session state from leaving a stale automated tab behind.
    let _ = runtime.close_all_site_sessions().await;
    runtime.disconnect_browser().await;
    Ok(())
}

#[tauri::command]
pub async fn cdp_status(runtime: State<'_, SocaiRuntime>) -> Result<BrowserStatus, String> {
    Ok(runtime.browser_status().await)
}

#[tauri::command]
pub async fn cdp_remote_debugging_ready() -> Result<bool, String> {
    let Some(endpoint) = socai_core::cdp::discover_existing_chrome_endpoint()
        .await
        .map_err(|err| format!("{err:#}"))?
    else {
        return Ok(false);
    };
    let url = url::Url::parse(&endpoint.browser_ws_url)
        .map_err(|err| format!("invalid chrome debugging endpoint: {err}"))?;
    let Some(host) = url.host_str() else {
        return Ok(false);
    };
    let Some(port) = url.port_or_known_default() else {
        return Ok(false);
    };

    // DevToolsActivePort can remain after the user disables remote debugging.
    // A short TCP probe verifies the local listener without opening a CDP
    // websocket, which would itself trigger chrome's Allow confirmation.
    Ok(matches!(
        tokio::time::timeout(
            std::time::Duration::from_millis(350),
            tokio::net::TcpStream::connect((host, port)),
        )
        .await,
        Ok(Ok(_))
    ))
}

#[tauri::command]
pub async fn cdp_refresh(_runtime: State<'_, SocaiRuntime>) -> Result<(), String> {
    Ok(())
}

// ── App lifecycle ───────────────────────────────────────────────────────────

/// Relaunch into a staged update. The process plugin's `relaunch()` routes the
/// respawn through event-loop teardown (`request_restart`), and on macOS the
/// process dies before the replacement finishes spawning — the app quits and
/// never comes back (tauri-apps/tauri#11392). Spawn the replacement while the
/// app is still fully alive instead, after the same browser cleanup quit does.
#[tauri::command]
pub async fn app_relaunch(app: AppHandle) {
    app.state::<SocaiRuntime>().disconnect_browser().await;
    app.cleanup_before_exit();
    tauri::process::restart(&app.env());
}

async fn label_controlled_page(page: &RuntimePageSession, label: &str) {
    let prefix = format!("◼ socai · {}", title_safe(label));
    let Ok(prefix_json) = serde_json::to_string(&prefix) else {
        return;
    };
    let script = format!(
        r#"
(function() {{
  const prefix = {prefix_json};
  const clean = (value) => {{
    const text = String(value || '').trim();
    if (text === prefix) return '';
    if (text.startsWith(`${{prefix}} · `)) return text.slice(prefix.length + 3).trim();
    return text;
  }};
  const apply = () => {{
    const current = clean(document.title);
    document.title = current ? `${{prefix}} · ${{current}}` : prefix;
  }};
  apply();
  if (window.__socaiTitleTimer) clearInterval(window.__socaiTitleTimer);
  let count = 0;
  window.__socaiTitleTimer = setInterval(() => {{
    apply();
    count += 1;
    if (count > 1200) clearInterval(window.__socaiTitleTimer);
  }}, 500);
  return document.title;
}})()
"#
    );
    let _ = page.evaluate_json(&script).await;
}

fn title_safe(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(48).collect()
}

fn read_run_usage(run_dir: &str) -> Option<TokenUsage> {
    let value: Value =
        serde_json::from_slice(&std::fs::read(PathBuf::from(run_dir).join("run.json")).ok()?)
            .ok()?;
    serde_json::from_value(value.get("usage")?.clone()).ok()
}

pub(crate) fn persist_run_points_used(run_dir: Option<&str>, points_used: Option<i64>) {
    let (Some(run_dir), Some(points_used)) = (run_dir, points_used) else {
        return;
    };
    let path = PathBuf::from(run_dir).join("run.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let billing = object
        .entry("billing")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(billing) = billing.as_object_mut() else {
        return;
    };
    billing.insert("points_used".into(), json!(points_used));
    if let Ok(rendered) = serde_json::to_vec_pretty(&value) {
        let _ = std::fs::write(path, rendered);
    }
}

/// Older desktop builds captured the server-authoritative charge in the local
/// task-end telemetry row but did not copy it into that turn's run.json. Match
/// by run_id (not task_id: replies intentionally share one task id) so those
/// completed turns gain the same durable per-run field as new runs.
fn recover_run_points_from_local_telemetry(snapshots: &[AgentTaskSnapshot]) {
    let path = app_data_dir().join("telemetry/events.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let mut by_run_id = HashMap::<String, i64>::new();
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if row.get("event").and_then(Value::as_str) != Some("socai_agent_task_end") {
            continue;
        }
        let properties = row.get("properties").unwrap_or(&row);
        let Some(run_id) = properties
            .get("run_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(points_used) = properties.get("points_used").and_then(Value::as_i64) else {
            continue;
        };
        by_run_id.insert(run_id.to_string(), points_used);
    }
    if by_run_id.is_empty() {
        return;
    }
    for snapshot in snapshots {
        for (run_dir, _) in crate::timeline::conversation_run_dirs(snapshot) {
            let path = run_dir.join("run.json");
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(run) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if run.pointer("/billing/points_used").is_some() {
                continue;
            }
            let Some(points_used) = run
                .get("id")
                .and_then(Value::as_str)
                .and_then(|run_id| by_run_id.get(run_id))
                .copied()
            else {
                continue;
            };
            persist_run_points_used(run_dir.to_str(), Some(points_used));
        }
    }
}

fn with_usage_telemetry(mut properties: Value, usage: Option<&TokenUsage>) -> Value {
    let (Some(map), Some(usage)) = (properties.as_object_mut(), usage) else {
        return properties;
    };
    map.insert("input_tokens".into(), json!(usage.input_tokens));
    map.insert(
        "uncached_input_tokens".into(),
        json!(usage.uncached_input_tokens),
    );
    map.insert("output_tokens".into(), json!(usage.output_tokens));
    if let Some(tokens) = usage.reasoning_output_tokens {
        map.insert("reasoning_output_tokens".into(), json!(tokens));
    }
    map.insert(
        "cached_input_tokens".into(),
        json!(usage.cache_read_input_tokens),
    );
    map.insert(
        "cache_creation_input_tokens".into(),
        json!(usage.cache_creation_input_tokens),
    );
    if let Some(cost) = &usage.cost {
        map.insert("estimated_input_cost".into(), json!(cost.input));
        map.insert("estimated_output_cost".into(), json!(cost.output));
        map.insert("estimated_cache_read_cost".into(), json!(cost.cache_read));
        map.insert(
            "estimated_cache_creation_cost".into(),
            json!(cost.cache_creation),
        );
        map.insert("estimated_cost".into(), json!(cost.total));
        map.insert("cost_currency".into(), json!(cost.currency));
        map.insert("cost_estimated".into(), json!(cost.estimated));
        map.insert("cost_pricing_source".into(), json!(cost.pricing_source));
    }
    properties
}

// ── Agent tasks ────────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct AgentRunOutcome {
    run_id: String,
    run_dir: String,
    steps: u32,
    final_text: String,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_input_tokens: u64,
    estimated_cost: Option<f64>,
    cost_currency: Option<String>,
    #[serde(skip)]
    usage: TokenUsage,
}

#[tauri::command]
pub async fn agent_save_api_key(provider: String, api_key: String) -> Result<(), String> {
    let provider_enum = Provider::from_name(provider.trim())
        .ok_or_else(|| format!("unknown provider: {provider}"))?;
    if provider_enum == Provider::Socai {
        return Err("socai agent uses your signed-in account, not an API key".into());
    }
    socai_core::agent::save_api_key(provider_enum, api_key.trim())
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn agent_list_models() -> Result<Vec<Value>, String> {
    use socai_core::agent::PROVIDERS;
    let _ = socai_core::cloud::take_hosted_llm_default().map_err(|err| format!("{err:#}"))?;
    // The provider/model that would be used right now. Environment overrides
    // win when present; otherwise the desktop restores persisted defaults even
    // if the selected provider still needs a key. The frontend uses
    // `is_default` to restore this choice across relaunches.
    let default_provider = if provider_env_override_present() {
        resolve_provider(None, None)
            .ok()
            .or_else(configured_default_provider)
    } else if socai_core::cloud::hosted_llm_selected() {
        Some(Provider::Socai)
    } else {
        configured_default_provider().or_else(|| resolve_provider(None, None).ok())
    };
    let env_model = model_env_override();
    let mut out = Vec::new();
    for cfg in PROVIDERS {
        let credential_kind = provider_credential_kind(cfg.provider);
        if cfg.provider == Provider::Socai && credential_kind.is_none() {
            continue;
        }
        let credential_kind_label = match credential_kind {
            Some(CredentialKind::ApiKey) => Some("api_key"),
            Some(CredentialKind::CodexOAuth) => Some("codex_oauth"),
            None => None,
        };
        let credential_preview =
            if cfg.provider != Provider::Socai && credential_kind == Some(CredentialKind::ApiKey) {
                load_api_key(cfg.provider).map(|key| {
                    let prefix = key.chars().take(8).collect::<String>();
                    format!("{prefix}…")
                })
            } else {
                None
            };
        let selected_model = if cfg.provider == Provider::Socai {
            // The hosted model is a server concern. Keep an opaque value in
            // desktop state even when SOCAI_MODEL is set for BYOK providers.
            cfg.default_model.to_string()
        } else if Some(cfg.provider) == default_provider {
            env_model
                .clone()
                .unwrap_or_else(|| configured_default_model_for(cfg.provider))
        } else {
            configured_default_model_for(cfg.provider)
        };
        let mut models = catalog_models_for(cfg.provider);
        if cfg.provider == Provider::Socai {
            models.retain(|model| model.id == cfg.default_model);
        }
        if !selected_model.trim().is_empty()
            && !models.iter().any(|model| model.id == selected_model)
        {
            models.insert(
                0,
                ModelCatalogEntry {
                    id: selected_model.clone(),
                    display_name: Some(selected_model.clone()),
                    source: Some("saved-default".into()),
                    recommended: false,
                    pricing: None,
                },
            );
        }
        for model in models {
            let model_id = model.id.trim().to_string();
            if model_id.is_empty() {
                continue;
            }
            let display_name = model.label().to_string();
            let is_default = default_provider == Some(cfg.provider) && model_id == selected_model;
            out.push(serde_json::json!({
                "provider": cfg.provider.as_str(),
                "provider_display_name": cfg.display_name,
                "display_name": display_name,
                // Back-compat name kept for the existing frontend model state:
                // each row is now a concrete model version rather than a provider.
                "default_model": model_id,
                "model_id": model_id,
                "selected_model": selected_model,
                "has_key": credential_kind.is_some(),
                "credential_kind": credential_kind_label,
                "credential_preview": credential_preview.clone(),
                "is_default": is_default,
                "recommended": model.recommended,
                "source": model.source,
            }));
        }
    }
    Ok(out)
}

fn provider_env_override_present() -> bool {
    ["SOCAI_LLM_PROVIDER", "SOCAI_MODEL"]
        .into_iter()
        .any(|key| {
            std::env::var(key)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        })
}

fn model_env_override() -> Option<String> {
    std::env::var("SOCAI_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Persist the user's model choice so it survives a relaunch. The hosted model
/// is account-scoped; BYOK choices keep using the shared CLI defaults.
#[tauri::command]
pub async fn agent_set_default_model(provider: String, model: String) -> Result<(), String> {
    let provider_enum = Provider::from_name(provider.trim())
        .ok_or_else(|| format!("unknown provider: {provider}"))?;
    if provider_enum == Provider::Socai {
        return socai_core::cloud::set_hosted_llm_selected(true).map_err(|err| format!("{err:#}"));
    }
    socai_core::cloud::set_hosted_llm_selected(false).map_err(|err| format!("{err:#}"))?;
    save_default_model(provider_enum, model.trim())
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
}

/// Open a web URL in the user's default browser. Tauri's webview does not hand
/// `target="_blank"` links off to the OS browser, so external links (e.g. the
/// "how to enable remote debugging" guide) route through here. Restricted to
/// http(s) so the frontend can't open arbitrary schemes or local files.
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("refusing to open non-web url: {url}"));
    }

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(&url);
        c
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(&url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        // Avoid `cmd /C start`: cmd.exe treats `&` in query strings as command
        // separators, truncating URLs like Alipay checkout (missing-method).
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", &url]);
        c
    };

    command
        .status()
        .map_err(|e| format!("failed to open {url}: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn open_chrome_remote_debugging() -> Result<(), String> {
    socai_core::cdp::open_remote_debugging_page().map_err(|err| format!("{err:#}"))
}

#[tauri::command]
pub async fn agent_open_codex_login() -> Result<Value, String> {
    tokio::task::spawn_blocking(start_codex_login)
        .await
        .map_err(|e| format!("codex login task failed: {e}"))?
}

fn start_codex_login() -> Result<Value, String> {
    let codex = find_codex_binary().ok_or_else(|| {
        "could not find `codex`. Install Codex CLI or paste an OpenAI API key.".to_string()
    })?;
    // Headless loopback browser login; the frontend polls for the credential.
    let mut child = Command::new(codex)
        .arg("login")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start `codex login`: {e}"))?;

    // Drain stdout and reap the child off-thread once login completes.
    let stdout = child.stdout.take();
    std::thread::spawn(move || {
        if let Some(stdout) = stdout {
            let mut reader = BufReader::new(stdout);
            let mut rest = String::new();
            let _ = reader.read_to_string(&mut rest);
        }
        let _ = child.wait();
    });

    Ok(json!({
        "message": "Browser opened. Finish signing in to ChatGPT, then return to socai.",
    }))
}

fn find_codex_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("codex");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    [
        "/opt/homebrew/bin/codex",
        "/usr/local/bin/codex",
        "~/.cargo/bin/codex",
    ]
    .iter()
    .filter_map(|path| {
        if let Some(stripped) = path.strip_prefix("~/") {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(stripped))
        } else {
            Some(PathBuf::from(path))
        }
    })
    .find(|path| path.is_file())
}

#[tauri::command]
pub async fn agent_task_start(
    app: AppHandle,
    runtime: State<'_, SocaiRuntime>,
    tasks: State<'_, AgentTaskRegistry>,
    telemetry: State<'_, DesktopTelemetry>,
    task: String,
    provider: Option<String>,
    model: Option<String>,
) -> Result<AgentTaskSnapshot, String> {
    let task_text = task.trim().to_string();
    if task_text.is_empty() {
        return Err("task is empty".into());
    }
    run_task_preflight(provider.as_deref(), model.as_deref()).await?;

    // One conversation = one folder under the runs root, named after the
    // first task; each turn's run dir nests inside it (turn-01_…, turn-02_…).
    let site_id = app_site_for_intent(&task_text, None)
        .map_err(|error| task_preflight_error("preflight_site", format!("{error:#}")))?
        .id;
    let conversation_dir = make_run_dir(&format!("{site_id} {task_text}"));
    let conversation = Conversation::create_at(&conversation_dir, model.clone())
        .map_err(|err| format!("failed to create desktop conversation session for task: {err}"))?;
    let run_dir = conversation.next_turn_dir(&task_text);
    let _ = std::fs::create_dir_all(&run_dir);
    let session_dir = conversation.dir.display().to_string();
    let registry = tasks.inner().clone();
    let snapshot = registry
        .create(
            task_text.clone(),
            provider.clone(),
            model.clone(),
            site_id.to_string(),
            run_dir.display().to_string(),
            session_dir,
        )
        .await;
    let task_id = snapshot.task_id.clone();
    let background_media_generation = socai_core::media::begin_background_media_generation();
    let runtime = runtime.inner().clone();
    let telemetry = telemetry.inner().clone();
    let task_id_for_spawn = task_id.clone();
    let app_for_task = app.clone();
    let registry_for_task = registry.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        if start_rx.await.is_err() {
            return;
        }
        run_agent_task_background(
            app_for_task,
            registry_for_task,
            runtime,
            task_id_for_spawn,
            site_id.to_string(),
            task_text,
            provider,
            model,
            run_dir,
            background_media_generation,
            telemetry,
        )
        .await;
    });
    if let Some(handle) = tasks.set_abort_handle(&task_id, join.abort_handle()).await {
        handle.abort();
    } else {
        emit_task_event(
            &app,
            tasks.inner(),
            &task_id,
            "queued",
            "task queued".into(),
            Some(snapshot.clone()),
        )
        .await;
        let _ = start_tx.send(());
    }
    Ok(snapshot)
}

/// Continue an existing task's conversation with a follow-up message. The
/// task must be terminal (not queued/running). Replies and new tasks share the
/// global `MAX_CONCURRENT_AGENT_TASKS` limit. Keeps the same `task_id`
/// and `session_dir` (so the whole thread's history stays attached to one
/// sidebar entry) but starts a fresh run dir for this turn.
#[tauri::command]
pub async fn agent_task_reply(
    app: AppHandle,
    runtime: State<'_, SocaiRuntime>,
    tasks: State<'_, AgentTaskRegistry>,
    telemetry: State<'_, DesktopTelemetry>,
    task_id: String,
    message: String,
) -> Result<AgentTaskSnapshot, String> {
    let message_text = message.trim().to_string();
    if message_text.is_empty() {
        return Err("message is empty".into());
    }
    let registry = tasks.inner().clone();
    let existing = registry
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    if matches!(existing.status.as_str(), "queued" | "running") {
        return Err("task is still running — wait for it to finish before replying".into());
    }
    let Some(session_dir) = existing.session_dir.as_deref() else {
        return Err("task has no conversation to continue".into());
    };
    let provider = existing.provider.clone();
    let model = existing.model.clone();
    let inherited_site_id = existing
        .site_id
        .as_deref()
        .unwrap_or_else(|| app_site_id_for_intent(&existing.task, None));
    let site_id = app_site_id_for_intent(&message_text, Some(inherited_site_id)).to_string();
    run_task_preflight(provider.as_deref(), model.as_deref()).await?;
    if let Some(previous_run_dir) = existing.run_dir.as_deref() {
        socai_core::media::cancel_background_media_for_run(previous_run_dir);
    }

    // This turn's run dir nests inside the conversation dir. Tasks created
    // before nesting have their session dir under ~/.socai/sessions; their
    // new turns nest there too, which the timeline and delete paths handle
    // the same way.
    let conversation = Conversation::load(session_dir)
        .map_err(|err| format!("failed to load conversation for task: {err}"))?;
    let run_dir = conversation.next_turn_dir(&message_text);
    let _ = std::fs::create_dir_all(&run_dir);

    let snapshot = registry
        .update(&task_id, |snapshot| {
            snapshot.status = "queued".into();
            snapshot.started_at = None;
            snapshot.finished_at = None;
            snapshot.run_id = None;
            snapshot.run_dir = Some(run_dir.display().to_string());
            snapshot.final_text = None;
            snapshot.error = None;
            snapshot.steps = None;
            snapshot.input_tokens = None;
            snapshot.output_tokens = None;
            snapshot.cached_input_tokens = None;
            snapshot.cache_creation_input_tokens = None;
            snapshot.estimated_cost = None;
            snapshot.cost_currency = None;
            snapshot.points_used = None;
            snapshot.current_message = Some(message_text.clone());
            snapshot.site_id = Some(site_id.clone());
        })
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;

    let background_media_generation = socai_core::media::begin_background_media_generation();
    let runtime = runtime.inner().clone();
    let telemetry = telemetry.inner().clone();
    let task_id_for_spawn = task_id.clone();
    let app_for_task = app.clone();
    let registry_for_task = registry.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        if start_rx.await.is_err() {
            return;
        }
        run_agent_task_background(
            app_for_task,
            registry_for_task,
            runtime,
            task_id_for_spawn,
            site_id,
            message_text,
            provider,
            model,
            run_dir,
            background_media_generation,
            telemetry,
        )
        .await;
    });
    if let Some(handle) = tasks.set_abort_handle(&task_id, join.abort_handle()).await {
        handle.abort();
    } else {
        emit_task_event(
            &app,
            tasks.inner(),
            &task_id,
            "queued",
            "reply queued".into(),
            Some(snapshot.clone()),
        )
        .await;
        let _ = start_tx.send(());
    }
    Ok(snapshot)
}

async fn run_task_preflight(provider: Option<&str>, model: Option<&str>) -> Result<(), String> {
    let resolved_provider = ensure_llm_provider_configured_for(provider, model)
        .map_err(|error| task_preflight_error("preflight_model_config", format!("{error:#}")))?;
    if resolved_provider == Provider::Socai {
        if !socai_core::cloud::pro_activated() {
            return Err(task_preflight_error(
                "preflight_auth",
                "sign in before using Socai Agent",
            ));
        }
        let wallet = socai_core::cloud::wallet_balance().await.map_err(|error| {
            task_preflight_error("preflight_region_or_account", format!("{error:#}"))
        })?;
        validate_preflight_balance(wallet.balance_points)?;
    }

    Ok(())
}

fn task_preflight_error(code: &str, detail: impl std::fmt::Display) -> String {
    json!({
        "code": code,
        "detail": detail.to_string(),
    })
    .to_string()
}

fn validate_preflight_balance(balance_points: i64) -> Result<(), String> {
    if balance_points <= 0 {
        Err(task_preflight_error(
            "preflight_balance",
            "insufficient Socai points; recharge or switch provider",
        ))
    } else {
        Ok(())
    }
}

/// How many times a run retries a browser that will not open before it gives
/// up and reports the browser's own error. Capacity waits are unbounded — the
/// runs ahead always finish — but a browser that fails to connect may be a
/// server outage or a spent daily remote-browser quota, and a task that never
/// stops queueing hides that from the user.
const MAX_BROWSER_CONNECT_ATTEMPTS: u32 = 3;

/// Outcome of bringing up a run's Chrome tab. `Busy` means "ask again in a
/// moment" and keeps the task queued; `Failed` carries a coded preflight error
/// the UI translates.
enum PageAdmission {
    Busy(BrowserBusy),
    Failed(String),
}

struct UnboundPageGuard {
    runtime: SocaiRuntime,
    target_id: Option<String>,
}

impl UnboundPageGuard {
    fn disarm(&mut self) {
        self.target_id = None;
    }
}

impl Drop for UnboundPageGuard {
    fn drop(&mut self) {
        let Some(target_id) = self.target_id.take() else {
            return;
        };
        let runtime = self.runtime.clone();
        tauri::async_runtime::spawn(async move {
            let _ = runtime.close_target(&target_id).await;
        });
    }
}

fn remote_browser_selected() -> bool {
    ChromeConnectOptions::from_config()
        .map(|options| options.profile == ChromeProfile::Remote)
        .unwrap_or(false)
}

/// Bring up this conversation's Chrome tab under the run's browser lease.
/// Called from the admission loop so a refusal parks the task in the queue
/// rather than failing it.
async fn acquire_session_page(
    runtime: &SocaiRuntime,
    lease: &BrowserLease,
    session_id: &str,
    site: &'static SiteSpec,
) -> Result<(Arc<RuntimePageSession>, UnboundPageGuard), PageAdmission> {
    let options = ChromeConnectOptions::from_config().map_err(|error| {
        PageAdmission::Failed(task_preflight_error(
            "preflight_browser_config",
            format!("{error:#}"),
        ))
    })?;
    // Each conversation owns one site tab. Separate tasks run in parallel
    // without navigating or closing another session's target; replies keep the
    // same target while the configured browser connection stays available.
    let page = runtime
        .ensure_session_site_page_with_browser_options(
            lease,
            session_id,
            site.id,
            app_site_start_url(),
            options,
        )
        .await
        .map_err(|error| match BrowserBusy::find(&error) {
            Some(busy) => PageAdmission::Busy(busy.clone()),
            None => PageAdmission::Failed(browser_preflight_error(format!("{error:#}"))),
        })?;
    let guard = UnboundPageGuard {
        runtime: runtime.clone(),
        target_id: Some(page.target_id().to_string()),
    };
    Ok((page, guard))
}

/// Associate a tab with its task as soon as browser admission succeeds. The
/// caller keeps an armed cleanup guard across these awaits, so cancellation
/// closes a page that has not reached the registry yet.
async fn bind_task_page(
    app: &AppHandle,
    registry: &AgentTaskRegistry,
    task_id: &str,
    page: &RuntimePageSession,
    title_label: &str,
) -> bool {
    let target_id = page.target_id().to_string();
    let page_url = page
        .evaluate_json("location.href")
        .await
        .ok()
        .and_then(|value| value.as_str().map(str::to_string));
    let page_title_marker = format!("task:{task_id}");
    let Some((snapshot, target_changed)) = registry
        .bind_target_if_active(task_id, target_id, page_url, page_title_marker.clone())
        .await
    else {
        return false;
    };
    label_controlled_page(page, &format!("{page_title_marker} · {title_label}")).await;
    if target_changed {
        emit_task_event(
            app,
            registry,
            task_id,
            "tab",
            "chrome tab marked as controlled by socai".into(),
            Some(snapshot),
        )
        .await;
    }
    true
}

fn browser_preflight_error(detail: String) -> String {
    task_preflight_error(browser_preflight_code(&detail), detail)
}

/// Which browser failure the user is looking at. A spent daily allowance on the
/// hosted browser is called out separately: unlike the other remote failures it
/// is not fixed by retrying or by checking the network.
fn browser_preflight_code(detail: &str) -> &'static str {
    if !remote_browser_selected() {
        return "preflight_browser";
    }
    if detail
        .to_ascii_lowercase()
        .contains("daily remote browser time limit")
    {
        return "preflight_browser_remote_quota";
    }
    "preflight_browser_remote"
}

/// Wait out a browser refusal. Returns false once a run has spent its connect
/// attempts, which is the point at which the browser error becomes the task's
/// failure instead of another wait.
async fn wait_out_browser_busy(busy: &BrowserBusy, connect_attempts: &mut u32) -> bool {
    if busy.kind == BrowserBusyKind::Connect {
        *connect_attempts += 1;
        if *connect_attempts >= MAX_BROWSER_CONNECT_ATTEMPTS {
            return false;
        }
    } else {
        *connect_attempts = 0;
    }
    tokio::time::sleep(busy.retry_after).await;
    true
}

/// The conversation session a task belongs to, which is also its Chrome tab's
/// identity. Tasks created before conversations were introduced have none.
async fn task_session_id(registry: &AgentTaskRegistry, task_id: &str) -> Option<String> {
    let session_dir = registry
        .get(task_id)
        .await
        .and_then(|task| task.session_dir)?;
    Conversation::load(&session_dir).ok().map(|c| c.id)
}

#[tauri::command]
pub async fn agent_task_list(
    tasks: State<'_, AgentTaskRegistry>,
) -> Result<Vec<AgentTaskSnapshot>, String> {
    // Older cloud-usage tasks predate persisted settlement results. Replaying the
    // idempotent settlement once on launch lets those tasks show the exact
    // server charge instead of deriving points from the desktop cost estimate.
    let snapshots = tasks.list().await;
    recover_run_points_from_local_telemetry(&snapshots);
    let snapshots = tasks.list().await;
    let has_cloud_session = socai_core::cloud::pro_activated();
    for snapshot in snapshots {
        if !has_cloud_session
            || snapshot.points_used.is_some()
            || !matches!(
                snapshot.status.as_str(),
                "completed" | "failed" | "cancelled" | "interrupted"
            )
        {
            continue;
        }
        match socai_core::cloud::settle_llm_task(&snapshot.task_id, &snapshot.status).await {
            Ok(settlement) => {
                if let Some(updated) = tasks
                    .update(&snapshot.task_id, |task| {
                        task.points_used =
                            visible_billed_points(task.provider.as_deref(), &settlement);
                    })
                    .await
                {
                    persist_run_points_used(updated.run_dir.as_deref(), updated.points_used);
                }
            }
            Err(err) => eprintln!(
                "failed to recover hosted LLM settlement for {}: {err:#}",
                snapshot.task_id
            ),
        }
    }
    Ok(tasks.list().await)
}

#[tauri::command]
pub async fn agent_task_get(
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
) -> Result<AgentTaskSnapshot, String> {
    tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))
}

#[tauri::command]
pub async fn agent_task_events(
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
) -> Result<Vec<AgentTaskEventPayload>, String> {
    tasks
        .events(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))
}

/// Notes the agent saw across the task's whole conversation — full content +
/// resolved local media, aggregated from every run's `notes.json` (oldest
/// first, re-reads overwrite in place) so earlier turns' citations keep
/// resolving after a follow-up. Media paths are absolutized against each
/// note's own run dir, since one registry now spans several run dirs. Powers
/// the desktop app's embedded rich-note cards; works live (run_dir is set at
/// task creation) and on history reload.
#[tauri::command]
pub async fn agent_task_notes(
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
) -> Result<Vec<Value>, String> {
    let snapshot = tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for (run_dir, _) in crate::timeline::conversation_run_dirs(&snapshot) {
        for mut note in socai_core::agent::note_store::load_notes(&run_dir) {
            absolutize_note_media(&mut note, &run_dir);
            let Some(id) = note
                .get("note_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
            else {
                continue;
            };
            if !by_id.contains_key(&id) {
                order.push(id.clone());
            }
            by_id.insert(id, note);
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect())
}

/// A file produced during one conversation turn and safe to expose as a
/// download. `turn_index` matches the frontend's zero-based conversation turn.
#[derive(serde::Serialize)]
pub struct AgentArtifact {
    turn_index: usize,
    name: String,
    path: String,
    relative_path: String,
    kind: String,
    size_bytes: u64,
    version: String,
    preview_kind: Option<String>,
    #[serde(skip)]
    identity: same_file::Handle,
}

#[derive(serde::Serialize)]
pub struct AgentArtifactDownload {
    name: String,
    path: String,
    identity: String,
}

const ARTIFACT_TEXT_PREVIEW_MAX_BYTES: u64 = 4 * 1024 * 1024;
const ARTIFACT_BINARY_PREVIEW_MAX_BYTES: u64 = 24 * 1024 * 1024;
const ARTIFACT_IMAGE_PREVIEW_MAX_PIXELS: u64 = 24_000_000;
const ARTIFACT_SPREADSHEET_PREVIEW_MAX_BYTES: u64 = 12 * 1024 * 1024;
const ARTIFACT_SPREADSHEET_MAX_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;
const ARTIFACT_SPREADSHEET_MAX_ARCHIVE_ENTRIES: usize = 4_096;
const ARTIFACT_SPREADSHEET_MAX_SHEETS: usize = 5;
const ARTIFACT_SPREADSHEET_MAX_ROWS: usize = 500;
const ARTIFACT_SPREADSHEET_MAX_COLUMNS: usize = 80;
const ARTIFACT_SPREADSHEET_MAX_CELL_BYTES: usize = 8 * 1024;
const ARTIFACT_SPREADSHEET_MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
const ARTIFACT_SPREADSHEET_MAX_CELL_RECORDS: usize =
    ARTIFACT_SPREADSHEET_MAX_ROWS * ARTIFACT_SPREADSHEET_MAX_COLUMNS;
const ARTIFACT_SPREADSHEET_MAX_SHARED_STRINGS: usize = 100_000;

#[derive(serde::Serialize)]
struct SpreadsheetPreview {
    sheets: Vec<SpreadsheetSheetPreview>,
    sheet_count: usize,
    truncated: bool,
}

#[derive(serde::Serialize)]
struct SpreadsheetSheetPreview {
    name: String,
    rows: Vec<Vec<String>>,
    truncated: bool,
}

/// Download cards include tool-registered artifacts (`artifacts/**`) and
/// explicit user deliverables (`outputs/**`). Runtime logs, note media and
/// model request/response traces stay private implementation detail.
#[tauri::command]
pub async fn agent_task_artifacts(
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
) -> Result<Vec<AgentArtifact>, String> {
    let snapshot = tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    Ok(task_artifacts(&snapshot))
}

/// Read a previewable task artifact after authorizing it against the current
/// task snapshot. Returning bytes over IPC keeps run paths outside the
/// WebView's asset-protocol scope and applies one size limit on every platform.
#[tauri::command]
pub async fn agent_task_artifact_preview(
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
    path: String,
) -> Result<tauri::ipc::Response, String> {
    let snapshot = tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    let artifact = task_artifacts(&snapshot)
        .into_iter()
        .find(|artifact| artifact.path == path)
        .ok_or_else(|| "artifact is not part of this task".to_string())?;
    let preview_kind = artifact
        .preview_kind
        .clone()
        .ok_or_else(|| "artifact type is not previewable".to_string())?;
    let source = PathBuf::from(&artifact.path);
    let source_identity = artifact.identity;
    tokio::task::spawn_blocking(move || {
        let file = open_artifact_source(&source, &source_identity)?;
        if preview_kind == "spreadsheet" {
            return spreadsheet_preview_response(file);
        }
        let limit = if matches!(preview_kind.as_str(), "pdf" | "image") {
            ARTIFACT_BINARY_PREVIEW_MAX_BYTES
        } else {
            ARTIFACT_TEXT_PREVIEW_MAX_BYTES
        };
        let bytes = read_artifact_preview(file, limit)?;
        if preview_kind == "image" {
            validate_artifact_image_preview(&source, &bytes)?;
        } else if preview_kind != "pdf" {
            std::str::from_utf8(&bytes)
                .map_err(|_| format!("artifact is not valid UTF-8: {}", source.display()))?;
        }
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
    .map_err(|error| format!("artifact preview task failed: {error}"))?
}

/// Copy one artifact into the user's Downloads directory. The requested path
/// must exactly match the task's current artifact listing; callers cannot use
/// this command as a general-purpose local-file copier.
#[tauri::command]
pub async fn agent_task_artifact_download(
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
    path: String,
) -> Result<AgentArtifactDownload, String> {
    let snapshot = tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    let artifact = task_artifacts(&snapshot)
        .into_iter()
        .find(|artifact| artifact.path == path)
        .ok_or_else(|| "artifact is not part of this task".to_string())?;
    let source = PathBuf::from(&artifact.path);
    let source_identity = artifact.identity;
    let downloads = dirs::download_dir()
        .ok_or_else(|| "could not resolve the Downloads directory".to_string())?;
    let artifact_name = artifact.name.clone();

    let (destination, identity) = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&downloads)
            .map_err(|error| format!("could not create {}: {error}", downloads.display()))?;
        let mut source_file = open_artifact_source(&source, &source_identity)?;
        let (destination, mut destination_file) = create_unique_download(&downloads, &source)?;
        if let Err(error) = std::io::copy(&mut source_file, &mut destination_file)
            .and_then(|_| destination_file.flush())
        {
            drop(destination_file);
            let _ = std::fs::remove_file(&destination);
            return Err(format!(
                "could not download {} to {}: {error}",
                source.display(),
                destination.display()
            ));
        }
        let identity = match artifact_file_identity(&destination_file) {
            Ok(identity) => identity,
            Err(error) => {
                drop(destination_file);
                let _ = std::fs::remove_file(&destination);
                return Err(error);
            }
        };
        Ok::<(PathBuf, String), String>((destination, identity))
    })
    .await
    .map_err(|error| format!("artifact download task failed: {error}"))??;

    Ok(AgentArtifactDownload {
        name: artifact_name,
        path: destination.to_string_lossy().to_string(),
        identity,
    })
}

/// Check whether a previously downloaded artifact still exists at the exact
/// path returned by the download command. Both the task artifact and the
/// destination filename are re-authorized before touching the Downloads path.
#[tauri::command]
pub async fn agent_task_artifact_download_exists(
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
    path: String,
    download_path: String,
    download_identity: String,
) -> Result<bool, String> {
    let snapshot = tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    let Some((downloads, destination)) =
        authorize_artifact_download(&snapshot, &path, &download_path)?
    else {
        return Ok(false);
    };

    tokio::task::spawn_blocking(move || {
        downloaded_artifact_file(&downloads, &destination, &download_identity)
            .map(|file| file.is_some())
    })
    .await
    .map_err(|error| format!("artifact download check failed: {error}"))?
}

/// Reveal the downloaded copy in Finder or Explorer. Missing, moved, replaced,
/// linked, or non-file destinations return `false`, allowing the card to return
/// to its download state without opening an untrusted path.
#[tauri::command]
pub async fn agent_task_artifact_open(
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
    path: String,
    download_path: String,
    download_identity: String,
) -> Result<bool, String> {
    let snapshot = tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    let Some((downloads, destination)) =
        authorize_artifact_download(&snapshot, &path, &download_path)?
    else {
        return Ok(false);
    };

    tokio::task::spawn_blocking(move || {
        let Some(file) = downloaded_artifact_file(&downloads, &destination, &download_identity)?
        else {
            return Ok(false);
        };
        reveal_artifact_in_file_manager(&destination, file)?;
        Ok(true)
    })
    .await
    .map_err(|error| format!("artifact open task failed: {error}"))?
}

fn task_artifacts(snapshot: &AgentTaskSnapshot) -> Vec<AgentArtifact> {
    let mut artifacts = Vec::new();
    let mut seen = HashSet::new();
    for (turn_index, (run_dir, _)) in crate::timeline::conversation_run_dirs(snapshot)
        .into_iter()
        .enumerate()
    {
        collect_run_artifacts(&run_dir, turn_index, &mut artifacts, &mut seen);
    }
    artifacts.sort_by(|left, right| {
        left.turn_index
            .cmp(&right.turn_index)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
    artifacts
}

fn collect_run_artifacts(
    run_dir: &std::path::Path,
    turn_index: usize,
    artifacts: &mut Vec<AgentArtifact>,
    seen: &mut HashSet<PathBuf>,
) {
    let Ok(run_root) = run_dir.canonicalize() else {
        return;
    };
    let mut pending = ["artifacts", "outputs"]
        .into_iter()
        .map(|name| run_root.join(name))
        .collect::<Vec<_>>();
    while let Some(dir) = pending.pop() {
        let Ok(dir_type) = std::fs::symlink_metadata(&dir).map(|metadata| metadata.file_type())
        else {
            continue;
        };
        if dir_type.is_symlink() || !dir_type.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(&run_root) {
                continue;
            }
            if !seen.insert(canonical.clone()) {
                continue;
            }
            let Ok(file) = open_read_only_no_follow(&canonical) else {
                continue;
            };
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            if !metadata.is_file() || canonical.canonicalize().ok().as_ref() != Some(&canonical) {
                continue;
            }
            let Ok(identity) = same_file::Handle::from_file(file) else {
                continue;
            };
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let Ok(relative_path) = path.strip_prefix(&run_root) else {
                continue;
            };
            let relative_path = relative_path.to_string_lossy().to_string();
            let kind = path
                .extension()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("file")
                .to_ascii_uppercase();
            artifacts.push(AgentArtifact {
                turn_index,
                name: name.to_string(),
                path: canonical.to_string_lossy().to_string(),
                relative_path,
                kind,
                size_bytes: metadata.len(),
                version: artifact_version(&identity, &metadata),
                preview_kind: artifact_preview_kind(&path, metadata.len()).map(str::to_string),
                identity,
            });
        }
    }
}

fn artifact_preview_kind(path: &std::path::Path, size_bytes: u64) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let (kind, limit) = match extension.as_str() {
        "md" | "markdown" => ("markdown", ARTIFACT_TEXT_PREVIEW_MAX_BYTES),
        "csv" | "tsv" => ("csv", ARTIFACT_TEXT_PREVIEW_MAX_BYTES),
        "txt" | "json" | "jsonl" | "yaml" | "yml" | "toml" | "xml" | "html" | "htm" | "css"
        | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "rs" | "py" | "go" | "java" | "kt"
        | "swift" | "sh" | "zsh" | "fish" | "ps1" | "sql" | "log" => {
            ("text", ARTIFACT_TEXT_PREVIEW_MAX_BYTES)
        }
        "xlsx" | "xlsm" => ("spreadsheet", ARTIFACT_SPREADSHEET_PREVIEW_MAX_BYTES),
        "pdf" => ("pdf", ARTIFACT_BINARY_PREVIEW_MAX_BYTES),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => {
            ("image", ARTIFACT_BINARY_PREVIEW_MAX_BYTES)
        }
        _ => return None,
    };
    (size_bytes <= limit).then_some(kind)
}

fn read_artifact_preview(file: std::fs::File, max_bytes: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read artifact preview: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "artifact exceeds the {} MB preview limit",
            max_bytes / (1024 * 1024)
        ));
    }
    Ok(bytes)
}

fn spreadsheet_preview_response(file: std::fs::File) -> Result<tauri::ipc::Response, String> {
    let bytes = read_artifact_preview(file, ARTIFACT_SPREADSHEET_PREVIEW_MAX_BYTES)?;
    let mut source = Cursor::new(bytes);
    validate_spreadsheet_archive(&mut source)?;
    let mut workbook =
        Xlsx::new(source).map_err(|error| format!("could not open Excel workbook: {error}"))?;
    let sheet_names = workbook
        .sheets_metadata()
        .iter()
        .filter(|sheet| sheet.typ == SheetType::WorkSheet)
        .map(|sheet| sheet.name.clone())
        .collect::<Vec<_>>();
    let mut remaining_text_bytes = ARTIFACT_SPREADSHEET_MAX_TEXT_BYTES;
    let mut sheets = Vec::new();

    for name in sheet_names.iter().take(ARTIFACT_SPREADSHEET_MAX_SHEETS) {
        let mut reader = workbook
            .worksheet_cells_reader(name)
            .map_err(|error| format!("could not read worksheet {name}: {error}"))?;
        let dimensions = reader.dimensions();
        let declared_rows = dimensions.end.0.saturating_sub(dimensions.start.0) as usize + 1;
        let declared_columns = dimensions.end.1.saturating_sub(dimensions.start.1) as usize + 1;
        let mut truncated = declared_rows > ARTIFACT_SPREADSHEET_MAX_ROWS
            || declared_columns > ARTIFACT_SPREADSHEET_MAX_COLUMNS;
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut cell_records = 0_usize;

        loop {
            if cell_records >= ARTIFACT_SPREADSHEET_MAX_CELL_RECORDS {
                truncated = true;
                break;
            }
            let Some(cell) = reader
                .next_cell()
                .map_err(|error| format!("could not read worksheet {name}: {error}"))?
            else {
                break;
            };
            cell_records += 1;
            if matches!(cell.get_value(), DataRef::Empty) {
                continue;
            }
            let (row, column) = cell.get_position();
            let Some(row) = row
                .checked_sub(dimensions.start.0)
                .map(|value| value as usize)
            else {
                truncated = true;
                continue;
            };
            let Some(column) = column
                .checked_sub(dimensions.start.1)
                .map(|value| value as usize)
            else {
                truncated = true;
                continue;
            };
            if row >= ARTIFACT_SPREADSHEET_MAX_ROWS || column >= ARTIFACT_SPREADSHEET_MAX_COLUMNS {
                truncated = true;
                continue;
            }
            if remaining_text_bytes == 0 {
                truncated = true;
                break;
            }

            let raw = spreadsheet_cell_text(cell.get_value());
            let cell_limit = ARTIFACT_SPREADSHEET_MAX_CELL_BYTES.min(remaining_text_bytes);
            let (text, cell_truncated) = truncate_utf8_bytes(&raw, cell_limit);
            truncated |= cell_truncated;
            remaining_text_bytes = remaining_text_bytes.saturating_sub(text.len());
            rows.resize_with(row + 1, Vec::new);
            rows[row].resize(column + 1, String::new());
            rows[row][column] = text;
        }

        sheets.push(SpreadsheetSheetPreview {
            name: name.clone(),
            rows,
            truncated,
        });
        if remaining_text_bytes == 0 {
            break;
        }
    }

    let payload = SpreadsheetPreview {
        truncated: sheets.len() < sheet_names.len(),
        sheets,
        sheet_count: sheet_names.len(),
    };
    serde_json::to_vec(&payload)
        .map(tauri::ipc::Response::new)
        .map_err(|error| format!("could not encode Excel preview: {error}"))
}

fn validate_spreadsheet_archive(file: &mut Cursor<Vec<u8>>) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(&mut *file)
        .map_err(|error| format!("invalid Excel workbook: {error}"))?;
    if archive.len() > ARTIFACT_SPREADSHEET_MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "Excel workbook exceeds the {} entry preview limit",
            ARTIFACT_SPREADSHEET_MAX_ARCHIVE_ENTRIES
        ));
    }
    let mut uncompressed_bytes = 0_u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("could not inspect Excel workbook: {error}"))?;
        uncompressed_bytes = uncompressed_bytes
            .checked_add(entry.size())
            .ok_or_else(|| "Excel workbook uncompressed size overflowed".to_string())?;
        if uncompressed_bytes > ARTIFACT_SPREADSHEET_MAX_UNCOMPRESSED_BYTES {
            return Err(format!(
                "Excel workbook exceeds the {} MB expanded preview limit",
                ARTIFACT_SPREADSHEET_MAX_UNCOMPRESSED_BYTES / (1024 * 1024)
            ));
        }
    }
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("could not inspect Excel workbook: {error}"))?;
        let is_xml = entry
            .name()
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("xml"));
        if is_xml {
            validate_spreadsheet_xml(entry)?;
        }
    }
    drop(archive);
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("could not reset Excel workbook: {error}"))?;
    Ok(())
}

fn validate_spreadsheet_xml(reader: impl Read) -> Result<(), String> {
    let mut xml = XmlReader::from_reader(BufReader::new(reader));
    let mut buffer = Vec::new();
    let mut shared_strings = false;
    let mut worksheet = false;
    let mut shared_string_count = 0_usize;
    let mut cell_count = 0_usize;

    loop {
        buffer.clear();
        match xml.read_event_into(&mut buffer) {
            Ok(XmlEvent::Start(event)) | Ok(XmlEvent::Empty(event)) => {
                let name = event.local_name();
                if name.as_ref() == b"sst" {
                    shared_strings = true;
                    for attribute in event.attributes().with_checks(false) {
                        let attribute = attribute
                            .map_err(|error| format!("invalid Excel shared strings: {error}"))?;
                        if attribute.key.local_name().as_ref() != b"uniqueCount" {
                            continue;
                        }
                        let count = std::str::from_utf8(attribute.value.as_ref())
                            .ok()
                            .and_then(|value| value.parse::<usize>().ok())
                            .ok_or_else(|| {
                                "invalid Excel shared string count metadata".to_string()
                            })?;
                        if count > ARTIFACT_SPREADSHEET_MAX_SHARED_STRINGS {
                            return Err(format!(
                                "Excel workbook exceeds the {} shared string preview limit",
                                ARTIFACT_SPREADSHEET_MAX_SHARED_STRINGS
                            ));
                        }
                    }
                } else if name.as_ref() == b"worksheet" {
                    worksheet = true;
                } else if shared_strings && name.as_ref() == b"si" {
                    shared_string_count += 1;
                    if shared_string_count > ARTIFACT_SPREADSHEET_MAX_SHARED_STRINGS {
                        return Err(format!(
                            "Excel workbook exceeds the {} shared string preview limit",
                            ARTIFACT_SPREADSHEET_MAX_SHARED_STRINGS
                        ));
                    }
                } else if worksheet && name.as_ref() == b"c" {
                    cell_count += 1;
                    if cell_count > ARTIFACT_SPREADSHEET_MAX_CELL_RECORDS {
                        return Ok(());
                    }
                }
            }
            Ok(XmlEvent::Eof) => return Ok(()),
            Err(error) => return Err(format!("invalid Excel XML: {error}")),
            _ => {}
        }
    }
}

fn spreadsheet_cell_text(value: &DataRef<'_>) -> String {
    let value: Data = value.clone().into();
    match value {
        Data::DateTime(value) if value.is_datetime() => {
            let (year, month, day, hour, minute, second, millis) = value.to_ymd_hms_milli();
            if hour == 0 && minute == 0 && second == 0 && millis == 0 {
                format!("{year:04}-{month:02}-{day:02}")
            } else if millis == 0 {
                format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
            } else {
                format!(
                    "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{millis:03}"
                )
            }
        }
        value => value.to_string(),
    }
}

fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    if max_bytes < '…'.len_utf8() {
        return (String::new(), true);
    }
    let mut end = max_bytes - '…'.len_utf8();
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = text[..end].to_string();
    truncated.push('…');
    (truncated, true)
}

fn artifact_version(identity: &same_file::Handle, metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "{}:{}:{modified}",
        hash_artifact_identity(identity),
        metadata.len()
    )
}

fn validate_artifact_image_preview(source: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let expected = match source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => image::ImageFormat::Png,
        "jpg" | "jpeg" => image::ImageFormat::Jpeg,
        "gif" => image::ImageFormat::Gif,
        "webp" => image::ImageFormat::WebP,
        "bmp" => image::ImageFormat::Bmp,
        _ => return Err("unsupported image preview format".to_string()),
    };
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("could not inspect image preview: {error}"))?;
    if reader.format() != Some(expected) {
        return Err(format!(
            "image contents do not match the file extension: {}",
            source.display()
        ));
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| format!("could not read image dimensions: {error}"))?;
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels > ARTIFACT_IMAGE_PREVIEW_MAX_PIXELS {
        return Err(format!(
            "image exceeds the {} megapixel preview limit",
            ARTIFACT_IMAGE_PREVIEW_MAX_PIXELS / 1_000_000
        ));
    }
    Ok(())
}

fn open_artifact_source(
    source: &std::path::Path,
    expected_identity: &same_file::Handle,
) -> Result<std::fs::File, String> {
    let file = open_read_only_no_follow(source)
        .map_err(|error| format!("could not open {}: {error}", source.display()))?;
    let opened = file.metadata().map_err(|error| {
        format!(
            "could not inspect open artifact {}: {error}",
            source.display()
        )
    })?;
    if !opened.is_file() {
        return Err(format!(
            "artifact is no longer a regular file: {}",
            source.display()
        ));
    }
    let opened_identity = same_file::Handle::from_file(
        file.try_clone()
            .map_err(|error| format!("could not inspect {}: {error}", source.display()))?,
    )
    .map_err(|error| format!("could not identify {}: {error}", source.display()))?;
    if &opened_identity != expected_identity {
        return Err(format!(
            "artifact changed while opening download: {}",
            source.display()
        ));
    }
    let current = source
        .canonicalize()
        .map_err(|error| format!("could not resolve {}: {error}", source.display()))?;
    if current != source {
        return Err(format!(
            "artifact path changed before download: {}",
            source.display()
        ));
    }
    Ok(file)
}

fn open_read_only_no_follow(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Keep a final-component reparse point from being followed between the
        // metadata check above and opening the handle.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn create_unique_download(
    downloads: &std::path::Path,
    source: &std::path::Path,
) -> Result<(PathBuf, std::fs::File), String> {
    let file_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("artifact has no valid filename: {}", source.display()))?;
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("artifact has no valid filename stem: {}", source.display()))?;
    let extension = source.extension().and_then(|value| value.to_str());
    for index in 0..=9999 {
        let candidate_name = match (index, extension) {
            (0, _) => file_name.to_string(),
            (_, Some(extension)) => format!("{stem} ({index}).{extension}"),
            (_, None) => format!("{stem} ({index})"),
        };
        let candidate = downloads.join(candidate_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("could not create {}: {error}", candidate.display()));
            }
        }
    }
    Err(format!(
        "could not allocate a download name for {file_name}"
    ))
}

fn authorize_artifact_download(
    snapshot: &AgentTaskSnapshot,
    source_path: &str,
    download_path: &str,
) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let artifact = task_artifacts(snapshot)
        .into_iter()
        .find(|artifact| artifact.path == source_path)
        .ok_or_else(|| "artifact is not part of this task".to_string())?;
    let downloads = dirs::download_dir()
        .ok_or_else(|| "could not resolve the Downloads directory".to_string())?;
    let destination = PathBuf::from(download_path);
    if !destination.is_absolute() {
        return Err("artifact download path must be absolute".into());
    }
    let candidate_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "artifact download path has no valid filename".to_string())?;
    if !is_artifact_download_name(&artifact.name, candidate_name) {
        return Err("artifact download filename does not match the task artifact".into());
    }
    let destination_parent = destination
        .parent()
        .ok_or_else(|| "artifact download path has no parent directory".to_string())?;
    if destination_parent != downloads {
        return Err("artifact download is outside the Downloads directory".into());
    }
    let downloads_root = match downloads.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not resolve {}: {error}",
                downloads.display()
            ));
        }
    };
    Ok(Some((downloads_root, destination)))
}

fn is_artifact_download_name(source_name: &str, candidate_name: &str) -> bool {
    if source_name == candidate_name {
        return true;
    }
    let source = std::path::Path::new(source_name);
    let Some(stem) = source.file_stem().and_then(|value| value.to_str()) else {
        return false;
    };
    let prefix = format!("{stem} (");
    let suffix = source
        .extension()
        .and_then(|value| value.to_str())
        .map(|extension| format!(").{extension}"))
        .unwrap_or_else(|| ")".to_string());
    let Some(index) = candidate_name
        .strip_prefix(&prefix)
        .and_then(|name| name.strip_suffix(&suffix))
    else {
        return false;
    };
    let Ok(index_value) = index.parse::<u16>() else {
        return false;
    };
    (1..=9999).contains(&index_value) && index_value.to_string() == index
}

fn downloaded_artifact_file(
    downloads_root: &std::path::Path,
    destination: &std::path::Path,
    expected_identity: &str,
) -> Result<Option<std::fs::File>, String> {
    let metadata = match std::fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not inspect downloaded artifact {}: {error}",
                destination.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(None);
    }
    let file = match open_read_only_no_follow(destination) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not open downloaded artifact {}: {error}",
                destination.display()
            ));
        }
    };
    let opened = file.metadata().map_err(|error| {
        format!(
            "could not inspect open downloaded artifact {}: {error}",
            destination.display()
        )
    })?;
    if !opened.is_file() {
        return Ok(None);
    }
    let canonical = match destination.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not resolve downloaded artifact {}: {error}",
                destination.display()
            ));
        }
    };
    if canonical.parent() != Some(downloads_root) {
        return Ok(None);
    }
    let opened_identity = same_file::Handle::from_file(file.try_clone().map_err(|error| {
        format!(
            "could not inspect downloaded artifact {}: {error}",
            destination.display()
        )
    })?)
    .map_err(|error| {
        format!(
            "could not identify downloaded artifact {}: {error}",
            destination.display()
        )
    })?;
    let current_identity = same_file::Handle::from_path(&canonical).map_err(|error| {
        format!(
            "could not identify current downloaded artifact {}: {error}",
            destination.display()
        )
    })?;
    if opened_identity != current_identity {
        return Ok(None);
    }
    if hash_artifact_identity(&opened_identity) != expected_identity {
        return Ok(None);
    }
    Ok(Some(file))
}

fn artifact_file_identity(file: &std::fs::File) -> Result<String, String> {
    let handle = same_file::Handle::from_file(
        file.try_clone()
            .map_err(|error| format!("could not inspect downloaded artifact: {error}"))?,
    )
    .map_err(|error| format!("could not identify downloaded artifact: {error}"))?;
    Ok(hash_artifact_identity(&handle))
}

fn hash_artifact_identity(handle: &same_file::Handle) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    handle.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn reveal_artifact_in_file_manager(
    path: &std::path::Path,
    source_file: std::fs::File,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open")
        .arg("-R")
        .arg(path)
        .status()
        .map_err(|error| format!("failed to reveal {}: {error}", path.display()))?;

    #[cfg(target_os = "windows")]
    let status = Command::new("explorer.exe")
        .arg(format!("/select,{}", dunce::simplified(path).display()))
        .status()
        .map_err(|error| format!("failed to reveal {}: {error}", path.display()))?;

    #[cfg(target_os = "linux")]
    let status = {
        let parent = path
            .parent()
            .ok_or_else(|| format!("artifact has no parent directory: {}", path.display()))?;
        Command::new("xdg-open")
            .arg(parent)
            .status()
            .map_err(|error| format!("failed to reveal {}: {error}", path.display()))?
    };

    if !status.success() {
        return Err(format!(
            "file manager could not reveal {} (status {status})",
            path.display()
        ));
    }
    drop(source_file);
    Ok(())
}

/// Rewrite a note's run-dir-relative media paths (`media[].src`/`poster`,
/// resolved via `media_dir`) to absolute paths, so notes from different runs
/// can share one frontend registry.
fn absolutize_note_media(note: &mut Value, run_dir: &std::path::Path) {
    let media_dir = note
        .get("media_dir")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let base = if media_dir.is_empty() {
        run_dir.to_path_buf()
    } else {
        run_dir.join(&media_dir)
    };
    let Some(media) = note.get_mut("media").and_then(Value::as_array_mut) else {
        return;
    };
    for item in media {
        for key in ["src", "poster"] {
            let Some(value) = item.get(key).and_then(Value::as_str) else {
                continue;
            };
            if value.is_empty() || std::path::Path::new(value).is_absolute() {
                continue;
            }
            item[key] = json!(base.join(value).to_string_lossy());
        }
    }
}

#[tauri::command]
pub async fn agent_task_cancel(
    app: AppHandle,
    runtime: State<'_, SocaiRuntime>,
    tasks: State<'_, AgentTaskRegistry>,
    telemetry: State<'_, DesktopTelemetry>,
    task_id: String,
) -> Result<AgentTaskSnapshot, String> {
    let (mut snapshot, abort_handle, target_id, changed) = tasks
        .cancel(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    if changed {
        if let Some(run_dir) = snapshot.run_dir.as_deref() {
            socai_core::media::cancel_background_media_for_run(run_dir);
        }
    }
    if let Some(handle) = abort_handle {
        handle.abort();
    }
    if let Some(target_id) = target_id {
        let _ = runtime.close_target(&target_id).await;
    }
    if changed {
        if socai_core::cloud::pro_activated() {
            if let Some(settlement) = settle_hosted_task_with_retry(&task_id, "cancelled").await {
                if let Some(updated) = tasks
                    .update(&task_id, |task| {
                        task.points_used =
                            visible_billed_points(task.provider.as_deref(), &settlement);
                    })
                    .await
                {
                    snapshot = updated;
                }
            }
        }
        persist_run_points_used(snapshot.run_dir.as_deref(), snapshot.points_used);
        let usage = snapshot.run_dir.as_deref().and_then(read_run_usage);
        if let Some(run_dir) = snapshot.run_dir.as_deref() {
            let _ = mark_agent_run_status(run_dir, "cancelled", None);
            let run_dir = PathBuf::from(run_dir);
            let telemetry = telemetry.inner().clone();
            tauri::async_runtime::spawn(async move {
                upload_terminal_run_trace(run_dir, "cancelled", &telemetry).await;
            });
        }
        record_desktop_session(&snapshot, "[cancelled by user]", "cancelled");
        telemetry.capture(
            "socai_agent_task_end",
            with_usage_telemetry(
                json!({
                    "task_id": task_id.clone(),
                    "provider": snapshot.provider.clone(),
                    "run_id": snapshot.run_id.clone(),
                    "model": snapshot.model.clone(),
                    "outcome": "cancelled",
                    "steps": snapshot.steps,
                    "points_used": snapshot.points_used,
                    "duration_ms": duration_ms(snapshot.started_at, snapshot.finished_at),
                }),
                usage.as_ref(),
            ),
        );
        emit_task_event(
            &app,
            tasks.inner(),
            &task_id,
            "cancelled",
            "task cancelled".into(),
            Some(snapshot.clone()),
        )
        .await;
    }
    Ok(snapshot)
}

pub(crate) async fn settle_hosted_task_with_retry(
    task_id: &str,
    final_status: &str,
) -> Option<socai_core::cloud::LlmSettlement> {
    let mut delay_ms = 200;
    for attempt in 0..3 {
        match socai_core::cloud::settle_llm_task(task_id, final_status).await {
            Ok(settlement) => return Some(settlement),
            Err(err) if attempt < 2 => {
                eprintln!(
                    "hosted LLM settlement retry {} for task {} ({}): {err:#}",
                    attempt + 1,
                    task_id,
                    final_status,
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                delay_ms *= 4;
            }
            Err(err) => {
                eprintln!(
                    "hosted LLM settlement failed for task {} ({}); server stale-task settlement will recover it: {err:#}",
                    task_id,
                    final_status,
                );
            }
        }
    }
    None
}

pub(crate) fn visible_billed_points(
    provider: Option<&str>,
    settlement: &socai_core::cloud::LlmSettlement,
) -> Option<i64> {
    if settlement.status == "settlement_pending" {
        return None;
    }
    if settlement.billed_points > 0 || provider == Some(Provider::Socai.as_str()) {
        Some(settlement.billed_points)
    } else {
        Some(0)
    }
}

/// Remove a task from history and delete its on-disk artifacts: every run dir
/// the conversation recorded (run.json, report.md, notes.json, media), the
/// latest run dir, and the conversation session dir. With the nested layout,
/// removing the session dir covers the turns inside it; the explicit run-dir
/// list also cleans up conversations whose turns predate nesting. Active
/// tasks must be cancelled first; the registry enforces that.
#[tauri::command]
pub async fn agent_task_delete(
    runtime: State<'_, SocaiRuntime>,
    tasks: State<'_, AgentTaskRegistry>,
    telemetry: State<'_, DesktopTelemetry>,
    task_id: String,
) -> Result<(), String> {
    let snapshot = tasks.delete(&task_id).await?;
    if let Some(target_id) = snapshot.target_id.as_deref() {
        let _ = runtime.close_target(target_id).await;
    }
    if let Some(run_dir) = snapshot.run_dir.as_deref() {
        socai_core::media::cancel_background_media_for_run(run_dir);
    }
    let mut dirs: Vec<String> = Vec::new();
    if let Some(session_dir) = &snapshot.session_dir {
        if let Ok(conversation) = Conversation::load(session_dir) {
            dirs.extend(conversation.runs.iter().map(|run| run.run_dir.clone()));
        }
        dirs.push(session_dir.clone());
    }
    if let Some(run_dir) = &snapshot.run_dir {
        dirs.push(run_dir.clone());
    }
    dirs.sort();
    dirs.dedup();
    let terminal_status = snapshot.status.clone();
    let trace_run_dir = snapshot.run_dir.as_deref().map(PathBuf::from);
    telemetry.capture(
        "socai_agent_task_delete",
        json!({
            "task_id": task_id.clone(),
            "provider": snapshot.provider.clone(),
            "model": snapshot.model.clone(),
            "status": terminal_status.clone(),
        }),
    );

    // Removing a history row should feel immediate. The filesystem cleanup is
    // background work, and cancelled/interrupted traces get a chance to finish
    // durable staging before their source run directory disappears.
    let telemetry = telemetry.inner().clone();
    tauri::async_runtime::spawn(async move {
        if matches!(terminal_status.as_str(), "cancelled" | "interrupted") {
            if let Some(run_dir) = trace_run_dir.as_deref() {
                wait_for_trace_staging(run_dir).await;
                upload_terminal_run_trace(run_dir, &terminal_status, &telemetry).await;
            }
        }
        let _ = tokio::task::spawn_blocking(move || {
            for dir in &dirs {
                if let Err(err) = std::fs::remove_dir_all(dir) {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        eprintln!("failed to delete task artifacts at {dir}: {err}");
                    }
                }
            }
            // XHS history caches absolute media paths into these run dirs; scrub
            // them so cross-run dedupe re-downloads instead of resurrecting dead paths.
            let run_dirs: Vec<PathBuf> = dirs.iter().map(PathBuf::from).collect();
            let scrubbed = XhsHistoryStore::open_default().scrub_media_under(&run_dirs);
            if scrubbed > 0 {
                eprintln!("forgot cached media for {scrubbed} notes under deleted task {task_id}");
            }
        })
        .await;
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_task_background(
    app: AppHandle,
    registry: AgentTaskRegistry,
    runtime: SocaiRuntime,
    task_id: String,
    site_id: String,
    task: String,
    provider: Option<String>,
    model: Option<String>,
    run_dir: PathBuf,
    background_media_generation: u64,
    telemetry: DesktopTelemetry,
) {
    let site = match find_site(&site_id) {
        Some(site) => site,
        None => {
            fail_task_before_run(
                &app,
                &registry,
                &telemetry,
                &task_id,
                provider.as_deref(),
                model.as_deref(),
                task_preflight_error("preflight_site", format!("unknown site: {site_id}")),
            )
            .await;
            return;
        }
    };
    let Some(session_id) = task_session_id(&registry, &task_id).await else {
        fail_task_before_run(
            &app,
            &registry,
            &telemetry,
            &task_id,
            provider.as_deref(),
            model.as_deref(),
            task_preflight_error("preflight_site", "task has no conversation session"),
        )
        .await;
        return;
    };

    // Admission is local to this app process: each run holds one task permit
    // and one browser lease for its entire lifetime.
    let mut connect_attempts = 0u32;
    let (_permit, _lease, _activity, page) = loop {
        let Some(permit) = registry.acquire_run_permit().await else {
            let error = "task runner queue closed".to_string();
            fail_task_before_run(
                &app,
                &registry,
                &telemetry,
                &task_id,
                provider.as_deref(),
                model.as_deref(),
                error,
            )
            .await;
            return;
        };
        let lease = match runtime.try_acquire_browser_lease() {
            Ok(lease) => lease,
            Err(busy) => {
                drop(permit);
                if wait_out_browser_busy(&busy, &mut connect_attempts).await {
                    continue;
                }
                fail_task_before_run(
                    &app,
                    &registry,
                    &telemetry,
                    &task_id,
                    provider.as_deref(),
                    model.as_deref(),
                    browser_preflight_error(busy.reason),
                )
                .await;
                return;
            }
        };
        // Held for the whole task — LLM thinking pauses between tool calls
        // included — so the remote idle reaper only fires between tasks.
        let activity = runtime.begin_activity().await;
        let (page, mut page_guard) =
            match acquire_session_page(&runtime, &lease, &session_id, site).await {
                Ok(admitted) => admitted,
                Err(PageAdmission::Busy(busy)) => {
                    drop(activity);
                    drop(lease);
                    drop(permit);
                    if wait_out_browser_busy(&busy, &mut connect_attempts).await {
                        continue;
                    }
                    fail_task_before_run(
                        &app,
                        &registry,
                        &telemetry,
                        &task_id,
                        provider.as_deref(),
                        model.as_deref(),
                        browser_preflight_error(busy.reason),
                    )
                    .await;
                    return;
                }
                Err(PageAdmission::Failed(error)) => {
                    drop(activity);
                    drop(lease);
                    drop(permit);
                    fail_task_before_run(
                        &app,
                        &registry,
                        &telemetry,
                        &task_id,
                        provider.as_deref(),
                        model.as_deref(),
                        error,
                    )
                    .await;
                    return;
                }
            };
        if !bind_task_page(
            &app,
            &registry,
            &task_id,
            &page,
            &format!("task · {}", title_safe(&task)),
        )
        .await
        {
            return;
        }
        page_guard.disarm();
        break (permit, lease, activity, page);
    };

    if let Some(snapshot) = registry
        .update(&task_id, |snapshot| {
            snapshot.status = "running".into();
            snapshot.started_at = Some(now_ms());
        })
        .await
    {
        emit_task_event(
            &app,
            &registry,
            &task_id,
            "running",
            "task started".into(),
            Some(snapshot),
        )
        .await;
    }

    telemetry.capture(
        "socai_agent_task_start",
        json!({
            "task_id": task_id.clone(),
            "provider": provider.clone(),
            "model": model.clone(),
            "task_len": task.chars().count(),
            "task_text": socai_core::telemetry::redact_secrets(&task),
        }),
    );

    let result = run_agent_task_on_session_page(
        app.clone(),
        task_id.clone(),
        page,
        site,
        &task,
        provider.as_deref(),
        model.as_deref(),
        Some(run_dir),
        Some(background_media_generation),
        Some(registry.clone()),
        telemetry.clone(),
    )
    .await;

    let settlement = if socai_core::cloud::pro_activated() {
        let final_status = if result.is_ok() {
            "completed"
        } else {
            "failed"
        };
        settle_hosted_task_with_retry(&task_id, final_status).await
    } else {
        None
    };

    let _ = registry.remove_abort_handle(&task_id).await;

    match result {
        Ok(outcome) => {
            if let Some(snapshot) = registry
                .finalize_if_active(&task_id, |snapshot| {
                    snapshot.status = "completed".into();
                    snapshot.finished_at = Some(now_ms());
                    snapshot.run_id = Some(outcome.run_id.clone());
                    snapshot.run_dir = Some(outcome.run_dir.clone());
                    // Final answer is hydrated from run_dir/report.md; tasks.json stays an index.
                    snapshot.final_text = None;
                    snapshot.error = None;
                    snapshot.steps = Some(outcome.steps);
                    snapshot.input_tokens = Some(outcome.input_tokens);
                    snapshot.output_tokens = Some(outcome.output_tokens);
                    snapshot.cached_input_tokens = Some(outcome.cached_input_tokens);
                    snapshot.cache_creation_input_tokens =
                        Some(outcome.cache_creation_input_tokens);
                    snapshot.estimated_cost = outcome.estimated_cost;
                    snapshot.cost_currency = outcome.cost_currency.clone();
                    snapshot.points_used = settlement.as_ref().and_then(|settlement| {
                        visible_billed_points(provider.as_deref(), settlement)
                    });
                })
                .await
            {
                persist_run_points_used(snapshot.run_dir.as_deref(), snapshot.points_used);
                record_desktop_session(&snapshot, &outcome.final_text, "completed");
                telemetry.capture(
                    "socai_agent_task_end",
                    with_usage_telemetry(
                        json!({
                            "task_id": task_id.clone(),
                            "run_id": outcome.run_id.clone(),
                            "provider": provider.clone(),
                            "model": model.clone(),
                            "outcome": "completed",
                            "steps": outcome.steps,
                            "points_used": snapshot.points_used,
                            "duration_ms": duration_ms(snapshot.started_at, snapshot.finished_at),
                        }),
                        Some(&outcome.usage),
                    ),
                );
                emit_task_event(
                    &app,
                    &registry,
                    &task_id,
                    "completed",
                    "task completed".into(),
                    Some(snapshot),
                )
                .await;
            }
        }
        Err(err) => {
            let error = format!("{err:#}");
            if let Some(snapshot) = registry
                .finalize_if_active(&task_id, |snapshot| {
                    snapshot.status = "failed".into();
                    snapshot.finished_at = Some(now_ms());
                    snapshot.error = Some(error.clone());
                    snapshot.points_used = settlement.as_ref().and_then(|settlement| {
                        visible_billed_points(provider.as_deref(), settlement)
                    });
                })
                .await
            {
                persist_run_points_used(snapshot.run_dir.as_deref(), snapshot.points_used);
                let usage = snapshot.run_dir.as_deref().and_then(read_run_usage);
                if let Some(run_dir) = snapshot.run_dir.as_deref() {
                    let _ = mark_agent_run_status(run_dir, "failed", Some(&error));
                    let _ = mark_run_trace_status(run_dir, "failed");
                    telemetry.upload_run_trace(run_dir);
                }
                record_desktop_session(&snapshot, &format!("[task failed: {error}]"), "failed");
                telemetry.capture(
                    "socai_agent_task_end",
                    with_usage_telemetry(
                        json!({
                            "task_id": task_id.clone(),
                            "provider": provider.clone(),
                            "model": model.clone(),
                            "outcome": "failed",
                            "error": short_error(&error),
                            "points_used": snapshot.points_used,
                            "duration_ms": duration_ms(snapshot.started_at, snapshot.finished_at),
                        }),
                        usage.as_ref(),
                    ),
                );
                emit_task_event(&app, &registry, &task_id, "failed", error, Some(snapshot)).await;
            }
        }
    }
}

async fn fail_task_before_run(
    app: &AppHandle,
    registry: &AgentTaskRegistry,
    telemetry: &DesktopTelemetry,
    task_id: &str,
    provider: Option<&str>,
    model: Option<&str>,
    error: String,
) {
    let _ = registry.remove_abort_handle(task_id).await;
    if let Some(snapshot) = registry
        .finalize_if_active(task_id, |snapshot| {
            snapshot.status = "failed".into();
            snapshot.finished_at = Some(now_ms());
            snapshot.error = Some(error.clone());
        })
        .await
    {
        record_desktop_session(&snapshot, &format!("[task failed: {error}]"), "failed");
        telemetry.capture(
            "socai_agent_task_end",
            json!({
                "task_id": task_id,
                "provider": provider,
                "model": model,
                "outcome": "failed",
                "error": short_error(&error),
                "points_used": snapshot.points_used,
                "duration_ms": duration_ms(snapshot.started_at, snapshot.finished_at),
            }),
        );
        emit_task_event(app, registry, task_id, "failed", error, Some(snapshot)).await;
    }
}

pub(crate) fn record_interrupted_run(snapshot: &AgentTaskSnapshot, message: &str) {
    if let Some(run_dir) = snapshot.run_dir.as_deref() {
        let _ = mark_agent_run_status(run_dir, "interrupted", Some(message));
    }
    record_desktop_session(
        snapshot,
        &format!("[task interrupted: {message}]"),
        "interrupted",
    );
}

/// Wait for an aborted agent future's trace drop guard, patch the precise
/// terminal state, then stage the trace durably before the task can be deleted.
/// AbortHandle::abort() only schedules cancellation; without this wait the
/// telemetry worker can race `trace.json` creation and silently miss the run.
pub(crate) async fn upload_terminal_run_trace(
    run_dir: impl AsRef<std::path::Path>,
    status: &str,
    telemetry: &DesktopTelemetry,
) -> bool {
    const READY_RETRIES: usize = 40;
    const READY_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

    let run_dir = run_dir.as_ref();
    let staged_marker = run_dir.join(".trace-staged");
    if staged_marker.is_file() {
        return true;
    }
    for attempt in 0..READY_RETRIES {
        match mark_run_trace_status(run_dir, status) {
            Ok(()) => {
                if telemetry.upload_run_trace(run_dir) {
                    let _ = std::fs::write(staged_marker, b"");
                    return true;
                }
                return false;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && attempt + 1 < READY_RETRIES =>
            {
                tokio::time::sleep(READY_DELAY).await;
            }
            Err(_) => return false,
        }
    }
    false
}

async fn wait_for_trace_staging(run_dir: &std::path::Path) {
    const WAIT_RETRIES: usize = 50;
    const WAIT_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

    let marker = run_dir.join(".trace-staged");
    for attempt in 0..WAIT_RETRIES {
        if marker.is_file() {
            return;
        }
        if attempt + 1 < WAIT_RETRIES {
            tokio::time::sleep(WAIT_DELAY).await;
        }
    }
}

fn record_desktop_session(snapshot: &AgentTaskSnapshot, assistant: &str, status: &str) {
    let Some(session_dir) = snapshot.session_dir.as_deref() else {
        return;
    };
    let Some(run_dir) = snapshot.run_dir.as_deref() else {
        return;
    };
    let Ok(mut conversation) = Conversation::load(session_dir) else {
        return;
    };
    let user_text = snapshot
        .current_message
        .as_deref()
        .unwrap_or(&snapshot.task);
    conversation.record_run(user_text, assistant, &PathBuf::from(run_dir), status);
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_task_on_session_page(
    app: AppHandle,
    task_id: String,
    page: Arc<RuntimePageSession>,
    site: &'static SiteSpec,
    task: &str,
    provider: Option<&str>,
    model: Option<&str>,
    run_dir: Option<PathBuf>,
    background_media_generation: Option<u64>,
    registry: Option<AgentTaskRegistry>,
    telemetry: DesktopTelemetry,
) -> Result<AgentRunOutcome> {
    let task = task.trim();
    if task.is_empty() {
        anyhow::bail!("task is empty");
    }
    let session_dir = if let Some(registry) = &registry {
        registry
            .get(&task_id)
            .await
            .and_then(|snapshot| snapshot.session_dir)
    } else {
        None
    };
    // Prior runs in this conversation, so a reply can continue it — empty for
    // a brand-new conversation's first run.
    let conversation = session_dir
        .as_deref()
        .and_then(|dir| Conversation::load(dir).ok());
    let seed_messages = conversation
        .as_ref()
        .map(|c| c.chat_messages())
        .unwrap_or_default();
    let session_id = conversation.as_ref().map(|c| c.id.clone());
    let context_note = conversation
        .as_ref()
        .map(|c| c.context_note())
        .unwrap_or_default();

    ensure_llm_provider_configured_for(provider, model)?;
    let llm_provider = create_llm_provider_for_task(provider, model, &task_id)?;
    let session_id =
        session_id.ok_or_else(|| anyhow::anyhow!("task has no conversation session"))?;
    let outcome = async {
        let agent_tools = site.default_agent_tools.unwrap_or(site.agent_tools);
        let mut tools = agent_tools(page.clone(), llm_provider.clone()).await?;
        tools.extend(desktop_agent_tools());
        tools.push(Arc::new(PublishArtifactTool::new(
            session_dir.as_deref().map(PathBuf::from),
        )));
        let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(256);
        let pump = pump_agent_task_events(
            app,
            registry.clone(),
            task_id.clone(),
            telemetry.clone(),
            rx,
        );

        let agent_instructions = site
            .default_agent_instructions
            .unwrap_or(site.agent_instructions);
        let preamble = format!("{TAURI_AGENT_PREAMBLE}\n\n{context_note}");
        let config = AgentRunConfig {
            extra_instructions: format!(
                "{}{}{}",
                agent_instructions(&preamble),
                tauri_citation_rules(site.id),
                TAURI_ARTIFACT_RULES
            ),
            enabled_sites: vec![site.id.to_string()],
            seed_messages,
            run_dir,
            session_id: Some(session_id),
            background_media_generation,
            billing_task_id: Some(task_id.clone()),
            ..AgentRunConfig::default()
        };
        let outcome = run_agent_with_tools(task, llm_provider, tools, config, tx).await;
        let _ = pump.await;
        let outcome = outcome?;
        if let Some(error) = outcome.error {
            // The run ended on a terminal error (unretryable API failure,
            // repeated truncation, or a failed forced summary); run.json and
            // the trace already say "failed" and final_text is best-effort.
            // Bail into the caller's failed arm — which marks the task failed
            // and uploads the trace — instead of reporting a completed task.
            anyhow::bail!(error);
        }
        telemetry.upload_run_trace(&outcome.run_dir);

        let usage = outcome.usage;
        let estimated_cost = usage.cost.as_ref().map(|cost| cost.total);
        let cost_currency = usage.cost.as_ref().map(|cost| cost.currency.clone());
        Ok::<AgentRunOutcome, anyhow::Error>(AgentRunOutcome {
            run_id: outcome.run_id,
            run_dir: outcome.run_dir.display().to_string(),
            steps: outcome.steps,
            final_text: outcome.final_text,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cache_read_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            estimated_cost,
            cost_currency,
            usage,
        })
    }
    .await;
    outcome
}

fn pump_agent_task_events(
    app: AppHandle,
    registry: Option<AgentTaskRegistry>,
    task_id: String,
    telemetry: DesktopTelemetry,
    mut rx: tokio::sync::broadcast::Receiver<AgentEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // The core run_id only surfaces on Started; remember it so each tool-call
        // event can correlate back to the run that produced it.
        let mut run_id: Option<String> = None;
        while let Ok(event) = rx.recv().await {
            match &event {
                AgentEvent::Started { run_id: id, .. } => {
                    run_id = Some(id.clone());
                    // Persist the core run_id so cancelled/interrupted terminal
                    // events (which read the snapshot, not this local) can still
                    // correlate to their tool_call rows.
                    if let Some(registry) = &registry {
                        let _ = registry
                            .update(&task_id, |snapshot| {
                                snapshot.run_id = Some(id.clone());
                            })
                            .await;
                    }
                }
                AgentEvent::ToolResult {
                    name,
                    step,
                    sequence,
                    input,
                    content,
                    duration_ms: tool_duration_ms,
                    error,
                    ..
                } => {
                    let mut props = Map::new();
                    props.insert("task_id".into(), json!(task_id.clone()));
                    props.insert("run_id".into(), json!(run_id.clone()));
                    props.insert("tool_name".into(), json!(name));
                    props.insert("step".into(), json!(step));
                    props.insert("sequence".into(), json!(sequence));
                    props.insert("duration_ms".into(), json!(tool_duration_ms));
                    props.insert("ok".into(), json!(error.is_none()));
                    props.insert("error".into(), json!(error.as_deref().map(short_error)));
                    // Same shared summarizer the CLI daemon uses: the search
                    // query is lifted into query_text/query_len (gated by
                    // query_text_enabled) and every other arg folds into
                    // metadata; result counts and bounded unexpected-page OCR
                    // diagnostics are extracted from the output. Note bodies
                    // and comments are never included.
                    props.extend(summarize_tool_args(input, query_text_enabled()));
                    props.extend(summarize_tool_result(content));
                    telemetry.capture("socai_tool_call", Value::Object(props));
                }
                _ => {}
            }
            let payload = agent_event_to_timeline(&event);
            emit_timeline_payload(&app, registry.as_ref(), &task_id, payload, None).await;
        }
    })
}

pub(crate) async fn emit_task_event(
    app: &AppHandle,
    registry: &AgentTaskRegistry,
    task_id: &str,
    kind: &str,
    text: String,
    snapshot: Option<AgentTaskSnapshot>,
) {
    let payload = AgentTaskEventKind::from_kind_text(kind, text);
    emit_timeline_payload(app, Some(registry), task_id, payload, snapshot).await;
}

async fn emit_timeline_payload(
    app: &AppHandle,
    registry: Option<&AgentTaskRegistry>,
    task_id: &str,
    payload: AgentTaskEventKind,
    snapshot: Option<AgentTaskSnapshot>,
) {
    let event = if let Some(registry) = registry {
        registry
            .live_timeline_event(task_id, payload.clone(), snapshot.clone())
            .await
            .unwrap_or_else(|| AgentTaskEventPayload::ephemeral(task_id, payload, snapshot))
    } else {
        AgentTaskEventPayload::ephemeral(task_id, payload, snapshot)
    };
    let _ = app.emit("agent_task:event", event);
}

// ── App configuration (~/.socai/config.json) ───────────────────────────────
// The desktop settings menu reads and writes the same config file the CLI
// manages via `socai config set`. Only the keys the menu surfaces are exposed.
// Persistence, validation, and key parsing all live in `socai_core::config`;
// these commands are thin pass-throughs plus the resolved defaults the menu
// shows as placeholders.

#[derive(serde::Serialize)]
pub struct DesktopConfig {
    /// Chrome profile mode: "managed" | "existing" | "auto" | "remote". Falls
    /// back to the product default ("existing") when the config key is unset.
    chrome_source: String,
    /// Configured managed-profile directory, or "" when unset.
    chrome_profile_dir: String,
    /// Resolved default managed-profile directory (shown as the input placeholder).
    chrome_profile_dir_default: String,
    /// Configured run-artifact root, or "" when unset.
    output_dir: String,
    /// Resolved default run-artifact root (shown as the input placeholder).
    output_dir_default: String,
}

#[tauri::command]
pub fn config_get() -> Result<DesktopConfig, String> {
    let config = socai_core::config::load_config().map_err(|err| format!("{err:#}"))?;
    Ok(DesktopConfig {
        chrome_source: config
            .chrome
            .profile
            .unwrap_or_default()
            .as_str()
            .to_string(),
        chrome_profile_dir: config.chrome.profile_dir.unwrap_or_default(),
        chrome_profile_dir_default: default_managed_profile_dir(),
        output_dir: config.runs.dir.unwrap_or_default(),
        output_dir_default: default_runs_root_display(),
    })
}

#[tauri::command]
pub async fn pro_activate(
    telemetry: State<'_, DesktopTelemetry>,
    invite_code: String,
    label: String,
) -> Result<Value, String> {
    let _ = label;
    let result = socai_core::cloud::redeem_invite(&invite_code)
        .await
        .map(|redemption| serde_json::to_value(redemption).unwrap_or_else(|_| json!({})))
        .map_err(|err| format!("{err:#}"));
    match &result {
        Ok(redemption) => telemetry.capture(
            "socai_invite_redeemed",
            json!({
                "outcome": "completed",
                "added_points": redemption.get("added_points"),
                "balance_points": redemption.get("balance_points"),
                "duration_days": redemption.get("duration_days"),
                "pro_active_until": redemption.get("active_until"),
            }),
        ),
        Err(error) => telemetry.capture(
            "socai_invite_redeemed",
            json!({ "outcome": "failed", "error": short_error(error) }),
        ),
    }
    result
}

#[tauri::command]
pub async fn auth_session() -> Result<socai_core::cloud::AuthSession, String> {
    socai_core::cloud::auth_session().map_err(|err| format!("{err:#}"))
}

#[tauri::command]
pub async fn auth_sms_send(
    telemetry: State<'_, DesktopTelemetry>,
    phone: String,
) -> Result<socai_core::cloud::SmsChallengeResponse, String> {
    let result = socai_core::cloud::send_sms_code(&phone)
        .await
        .map_err(|err| format!("{err:#}"));
    telemetry.capture(
        "socai_auth_sms_requested",
        match &result {
            Ok(_) => json!({ "account_phone": phone.trim(), "outcome": "completed" }),
            Err(error) => json!({
                "account_phone": phone.trim(),
                "outcome": "failed",
                "error": short_error(error),
            }),
        },
    );
    result
}

#[tauri::command]
pub async fn auth_sms_verify(
    telemetry: State<'_, DesktopTelemetry>,
    challenge_id: String,
    phone: String,
    code: String,
) -> Result<socai_core::cloud::AuthSession, String> {
    let result = socai_core::cloud::verify_sms_code(&challenge_id, &phone, &code, "desktop")
        .await
        .map_err(|err| format!("{err:#}"));
    telemetry.capture(
        "socai_auth_login",
        match &result {
            Ok(session) => json!({
                "account_phone": session.phone,
                "account_device_id": session.device_id,
                "outcome": "completed",
            }),
            Err(error) => json!({
                "account_phone": phone.trim(),
                "outcome": "failed",
                "error": short_error(error),
            }),
        },
    );
    result
}

#[tauri::command]
pub async fn auth_logout(telemetry: State<'_, DesktopTelemetry>) -> Result<(), String> {
    let session = socai_core::cloud::auth_session().ok();
    let result = socai_core::cloud::logout()
        .await
        .map_err(|err| format!("{err:#}"));
    telemetry.capture(
        "socai_auth_logout",
        json!({
            "account_phone": session.as_ref().map(|value| value.phone.as_str()),
            "outcome": if result.is_ok() { "completed" } else { "failed" },
            "error": result.as_ref().err().map(|error| short_error(error)),
        }),
    );
    result
}

#[tauri::command]
pub async fn billing_wallet(
    telemetry: State<'_, DesktopTelemetry>,
) -> Result<socai_core::cloud::WalletBalance, String> {
    let result = socai_core::cloud::wallet_balance()
        .await
        .map_err(|err| format!("{err:#}"));
    if let Ok(wallet) = &result {
        telemetry.capture(
            "socai_wallet_snapshot",
            json!({
                "balance_points": wallet.balance_points,
                "starter_points": wallet.starter_points,
                "points_per_cny": wallet.points_per_cny,
                "pro_active_until": wallet.active_until,
            }),
        );
    }
    result
}

#[tauri::command]
pub async fn billing_plan() -> Result<socai_core::cloud::PaymentPlan, String> {
    socai_core::cloud::payment_plan()
        .await
        .map_err(|err| format!("{err:#}"))
}

#[tauri::command]
pub async fn billing_create_wechat_order(
    telemetry: State<'_, DesktopTelemetry>,
    plan_id: String,
    request_id: String,
) -> Result<socai_core::cloud::PaymentOrder, String> {
    let result = socai_core::cloud::create_wechat_order(&plan_id, &request_id)
        .await
        .map_err(|err| format!("{err:#}"));
    capture_subscription_checkout(&telemetry, "wechatpay", &plan_id, &result);
    result
}

#[tauri::command]
pub async fn billing_create_alipay_order(
    telemetry: State<'_, DesktopTelemetry>,
    plan_id: String,
    request_id: String,
) -> Result<socai_core::cloud::PaymentOrder, String> {
    let result = socai_core::cloud::create_alipay_order(&plan_id, &request_id)
        .await
        .map_err(|err| format!("{err:#}"));
    capture_subscription_checkout(&telemetry, "alipay", &plan_id, &result);
    result
}

#[tauri::command]
pub async fn billing_order_status(
    telemetry: State<'_, DesktopTelemetry>,
    order_id: String,
) -> Result<socai_core::cloud::PaymentOrder, String> {
    let result = socai_core::cloud::payment_order(&order_id)
        .await
        .map_err(|err| format!("{err:#}"));
    if let Ok(order) = &result {
        if order.status == "paid" {
            telemetry.capture(
                "socai_subscription_paid",
                json!({
                    "order_id": order.order_id,
                    "amount_fen": order.amount_fen,
                    "added_points": order.points,
                    "duration_days": order.duration_days,
                    "pro_active_until": order.active_until,
                }),
            );
        }
    }
    result
}

#[tauri::command]
pub async fn billing_mock_recharge(
    telemetry: State<'_, DesktopTelemetry>,
    points: i64,
    request_id: String,
) -> Result<socai_core::cloud::RechargeReceipt, String> {
    let result = socai_core::cloud::mock_recharge(points, &request_id)
        .await
        .map_err(|err| format!("{err:#}"));
    if let Ok(receipt) = &result {
        telemetry.capture(
            "socai_wallet_mock_recharge",
            json!({
                "added_points": receipt.added_points,
                "balance_points": receipt.balance_points,
                "amount_fen": receipt.amount_fen,
            }),
        );
    }
    result
}

fn capture_subscription_checkout(
    telemetry: &DesktopTelemetry,
    provider: &str,
    plan_id: &str,
    result: &Result<socai_core::cloud::PaymentOrder, String>,
) {
    telemetry.capture(
        "socai_subscription_checkout",
        match result {
            Ok(order) => json!({
                "provider": provider,
                "plan_id": plan_id,
                "outcome": "created",
                "order_id": order.order_id,
                "amount_fen": order.amount_fen,
                "points": order.points,
                "duration_days": order.duration_days,
            }),
            Err(error) => json!({
                "provider": provider,
                "plan_id": plan_id,
                "outcome": "failed",
                "error": short_error(error),
            }),
        },
    );
}

#[tauri::command]
pub fn config_set(key: String, value: String) -> Result<(), String> {
    socai_core::config::set_config_key(&key, &value)
        .map(|_| ())
        .map_err(|err| format!("{err:#}"))
}

#[tauri::command]
pub fn config_unset(key: String) -> Result<(), String> {
    socai_core::config::unset_config_key(&key)
        .map(|_| ())
        .map_err(|err| format!("{err:#}"))
}

/// Default run-artifact root when `runs.dir` is unset. Mirrors
/// `socai_core::agent::default_runs_root` for the no-config case:
/// `SOCAI_RUNS_DIR`, then `~/.socai/runs`.
fn default_runs_root_display() -> String {
    if let Some(dir) = non_empty_env("SOCAI_RUNS_DIR") {
        return dir;
    }
    join_home(".socai/runs")
}

/// Default managed chrome user-data-dir when `chrome.profile_dir` is unset.
/// Delegates to the core resolver (`SOCAI_HOME/chrome-profile`, then
/// `~/.socai/chrome-profile`) so the placeholder matches what chrome would
/// actually launch with — including `~` expansion of a tilde-prefixed
/// `SOCAI_HOME`, which a local reimplementation tends to drift on.
fn default_managed_profile_dir() -> String {
    socai_core::cdp::managed_chrome_user_data_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var_os(key)
        .map(|value| value.to_string_lossy().trim().to_string())
        .filter(|value| !value.is_empty())
}

fn join_home(relative: &str) -> String {
    // `dirs::home_dir()` resolves `%USERPROFILE%` on Windows and `$HOME` on
    // unix, so the settings-UI placeholder shows the real `~/.socai/...` path on
    // every platform instead of a bare relative string on Windows.
    match dirs::home_dir() {
        Some(home) => home.join(relative).to_string_lossy().into_owned(),
        None => relative.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        previous: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
            let previous = vars
                .iter()
                .map(|(key, _)| (*key, std::env::var(key).ok()))
                .collect();
            for (key, value) in vars {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.previous {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }

    #[tokio::test]
    async fn agent_list_models_honors_socai_model_as_selected_row() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::set(&[("SOCAI_LLM_PROVIDER", None), ("SOCAI_MODEL", Some("o3"))]);

        let models = agent_list_models().await.unwrap();
        let selected: Vec<&Value> = models
            .iter()
            .filter(|model| model.get("is_default").and_then(Value::as_bool) == Some(true))
            .collect();

        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected[0].get("provider").and_then(Value::as_str),
            Some("openai")
        );
        assert_eq!(
            selected[0].get("model_id").and_then(Value::as_str),
            Some("o3")
        );
        assert_eq!(
            selected[0].get("selected_model").and_then(Value::as_str),
            Some("o3")
        );
    }

    #[test]
    fn preflight_rejects_empty_hosted_balance() {
        assert!(validate_preflight_balance(100).is_ok());
        let error = validate_preflight_balance(0).expect_err("zero balance must fail");
        let payload: Value = serde_json::from_str(&error).expect("preflight error must be JSON");
        assert_eq!(payload["code"], "preflight_balance");
        assert_eq!(
            payload["detail"],
            "insufficient Socai points; recharge or switch provider"
        );
        assert!(validate_preflight_balance(-1).is_err());
    }

    #[test]
    fn desktop_routes_explicit_platform_intent_without_preloading_a_site() {
        assert_eq!(app_site_id_for_intent("在抖音搜索 OpenAI", None), "dy");
        assert_eq!(
            app_site_id_for_intent("https://www.douyin.com/video/123", None),
            "dy"
        );
        assert_eq!(app_site_id_for_intent("搜索小红书露营帖子", None), "xhs");
        assert_eq!(
            app_site_id_for_intent("https://www.xiaohongshu.com/explore/abc", None),
            "xhs"
        );
        assert_eq!(app_site_id_for_intent("继续读取评论", Some("dy")), "dy");
        assert_eq!(app_site_id_for_intent("继续读取评论", None), "xhs");

        assert_eq!(app_site_start_url(), "");
    }
}
