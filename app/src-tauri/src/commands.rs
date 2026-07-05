use crate::tasks::{now_ms, AgentTaskRegistry, AgentTaskSnapshot};
use crate::telemetry::{duration_ms, short_error, DesktopTelemetry};
use crate::timeline::{agent_event_to_timeline, AgentTaskEventKind, AgentTaskEventPayload};
use anyhow::Result;
use serde_json::{json, Map, Value};
use socai_core::agent::{
    catalog_models_for, configured_default_model_for, configured_default_provider, make_run_dir,
    mark_agent_run_status, provider_credential_kind, resolve_provider, save_default_model,
    AgentEvent, CredentialKind, ModelCatalogEntry, Provider, Session,
};
use socai_core::runtime::{
    create_llm_provider_for, ensure_llm_provider_configured_for,
    run_agent_task as run_agent_with_tools, AgentRunConfig, BrowserStatus, RuntimePageSession,
    SocaiRuntime,
};
use socai_core::sites::{find_site, SiteSpec};
use socai_core::telemetry::query_text_enabled;
use socai_core::telemetry::tool_call::{summarize_tool_args, summarize_tool_result};
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

const TAURI_AGENT_PREAMBLE: &str = "You are running inside the socai desktop app.";

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

/// Site the desktop agent runner drives. Becomes a runtime choice once the
/// app grows a site switcher.
const APP_SITE_ID: &str = "xhs";

fn app_site() -> Result<&'static SiteSpec> {
    find_site(APP_SITE_ID)
        .ok_or_else(|| anyhow::anyhow!("app default site {APP_SITE_ID} is not registered"))
}

// ── CDP connect tests (existing) ───────────────────────────────────────────

#[tauri::command]
pub async fn cdp_connect(
    runtime: State<'_, SocaiRuntime>,
    telemetry: State<'_, DesktopTelemetry>,
) -> Result<(), String> {
    runtime.connect_browser();
    telemetry.capture("socai_browser_connect", json!({}));
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
pub async fn cdp_refresh(_runtime: State<'_, SocaiRuntime>) -> Result<(), String> {
    Ok(())
}

async fn require_connected(runtime: &SocaiRuntime) -> Result<(), String> {
    match runtime.browser_status().await {
        BrowserStatus::Connected { .. } => Ok(()),
        _ => Err("chrome not connected — click connect first".into()),
    }
}

async fn close_page(page: Arc<RuntimePageSession>) {
    if let Ok(page) = Arc::try_unwrap(page) {
        let _ = page.close().await;
    }
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

// ── Agent tasks ────────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct AgentRunOutcome {
    run_id: String,
    run_dir: String,
    turns: u32,
    final_text: String,
    input_tokens: u64,
    output_tokens: u64,
}

#[tauri::command]
pub async fn agent_save_api_key(provider: String, api_key: String) -> Result<(), String> {
    let provider_enum = Provider::from_name(provider.trim())
        .ok_or_else(|| format!("unknown provider: {provider}"))?;
    socai_core::agent::save_api_key(provider_enum, api_key.trim())
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn agent_list_models() -> Result<Vec<Value>, String> {
    use socai_core::agent::PROVIDERS;
    // The provider/model that would be used right now. Environment overrides
    // win when present; otherwise the desktop restores persisted defaults even
    // if the selected provider still needs a key. The frontend uses
    // `is_default` to restore this choice across relaunches.
    let default_provider = if provider_env_override_present() {
        resolve_provider(None, None)
            .ok()
            .or_else(configured_default_provider)
    } else {
        configured_default_provider().or_else(|| resolve_provider(None, None).ok())
    };
    let env_model = model_env_override();
    let mut out = Vec::new();
    for cfg in PROVIDERS {
        let credential_kind = provider_credential_kind(cfg.provider);
        let credential_kind_label = match credential_kind {
            Some(CredentialKind::ApiKey) => Some("api_key"),
            Some(CredentialKind::CodexOAuth) => Some("codex_oauth"),
            None => None,
        };
        let selected_model = if Some(cfg.provider) == default_provider {
            env_model
                .clone()
                .unwrap_or_else(|| configured_default_model_for(cfg.provider))
        } else {
            configured_default_model_for(cfg.provider)
        };
        let mut models = catalog_models_for(cfg.provider);
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

/// Persist the user's model choice so it survives a relaunch. Writes
/// `defaults.provider` + `defaults.{provider}_model` to `~/.socai/auth.json`.
#[tauri::command]
pub async fn agent_set_default_model(provider: String, model: String) -> Result<(), String> {
    let provider_enum = Provider::from_name(provider.trim())
        .ok_or_else(|| format!("unknown provider: {provider}"))?;
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
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", &url]);
        c
    };

    command
        .status()
        .map_err(|e| format!("failed to open {url}: {e}"))?;
    Ok(())
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
    require_connected(&runtime).await?;
    let task_text = task.trim().to_string();
    if task_text.is_empty() {
        return Err("task is empty".into());
    }
    ensure_llm_provider_configured_for(provider.as_deref(), model.as_deref())
        .map_err(|e| format!("{e:#}"))?;

    let site_id = app_site().map(|site| site.id).unwrap_or("agent");
    let run_dir = make_run_dir(&format!("{site_id} {task_text}"));
    let _ = std::fs::create_dir_all(&run_dir);
    let session = Session::new(model.clone())
        .map_err(|err| format!("failed to create desktop conversation session for task: {err}"))?;
    let session_dir = session.dir.display().to_string();
    let registry = tasks.inner().clone();
    let snapshot = registry
        .create(
            task_text.clone(),
            provider.clone(),
            model.clone(),
            run_dir.display().to_string(),
            session_dir,
        )
        .await;
    let task_id = snapshot.task_id.clone();
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
            task_text,
            provider,
            model,
            run_dir,
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

#[tauri::command]
pub async fn agent_task_list(
    tasks: State<'_, AgentTaskRegistry>,
) -> Result<Vec<AgentTaskSnapshot>, String> {
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

/// Notes the agent saw during a run — full content + resolved local media,
/// read from `<run_dir>/notes.json`. Empty when the run recorded none (or has
/// not scanned yet). Powers the desktop app's embedded rich-note cards; works
/// live (run_dir is set at task creation) and on history reload.
#[tauri::command]
pub async fn agent_task_notes(
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
) -> Result<Vec<Value>, String> {
    let snapshot = tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    let Some(run_dir) = snapshot.run_dir else {
        return Ok(Vec::new());
    };
    Ok(socai_core::agent::note_store::load_notes(
        std::path::Path::new(&run_dir),
    ))
}

#[tauri::command]
pub async fn agent_task_cancel(
    app: AppHandle,
    runtime: State<'_, SocaiRuntime>,
    tasks: State<'_, AgentTaskRegistry>,
    telemetry: State<'_, DesktopTelemetry>,
    task_id: String,
) -> Result<AgentTaskSnapshot, String> {
    let (snapshot, abort_handle, target_id, changed) = tasks
        .cancel(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    if let Some(handle) = abort_handle {
        handle.abort();
    }
    if let Some(target_id) = target_id {
        let _ = runtime.close_target(&target_id).await;
    }
    if changed {
        if let Some(run_dir) = snapshot.run_dir.as_deref() {
            let _ = mark_agent_run_status(run_dir, "cancelled", None);
        }
        record_desktop_session(&snapshot, "[cancelled by user]", "cancelled");
        telemetry.capture(
            "socai_agent_task_end",
            json!({
                "task_id": task_id.clone(),
                "provider": snapshot.provider.clone(),
                "run_id": snapshot.run_id.clone(),
                "model": snapshot.model.clone(),
                "outcome": "cancelled",
                "turns": snapshot.turns,
                "input_tokens": snapshot.input_tokens,
                "output_tokens": snapshot.output_tokens,
                "duration_ms": duration_ms(snapshot.started_at, snapshot.finished_at),
            }),
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

/// Remove a task from history and delete its on-disk artifacts: the run dir
/// (run.json, report.md, notes.json, media) and the conversation session dir.
/// Active tasks must be cancelled first; the registry enforces that.
#[tauri::command]
pub async fn agent_task_delete(
    tasks: State<'_, AgentTaskRegistry>,
    telemetry: State<'_, DesktopTelemetry>,
    task_id: String,
) -> Result<(), String> {
    let snapshot = tasks.delete(&task_id).await?;
    for dir in [&snapshot.run_dir, &snapshot.session_dir].into_iter().flatten() {
        if let Err(err) = std::fs::remove_dir_all(dir) {
            if err.kind() != std::io::ErrorKind::NotFound {
                eprintln!("failed to delete task artifacts at {dir}: {err}");
            }
        }
    }
    telemetry.capture(
        "socai_agent_task_delete",
        json!({
            "task_id": task_id,
            "provider": snapshot.provider,
            "model": snapshot.model,
            "status": snapshot.status,
        }),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_task_background(
    app: AppHandle,
    registry: AgentTaskRegistry,
    runtime: SocaiRuntime,
    task_id: String,
    task: String,
    provider: Option<String>,
    model: Option<String>,
    run_dir: PathBuf,
    telemetry: DesktopTelemetry,
) {
    let Some(_permit) = registry.acquire_run_permit().await else {
        let error = "task runner queue closed".to_string();
        if let Some(snapshot) = registry
            .finalize_if_active(&task_id, |snapshot| {
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
                    "task_id": task_id.clone(),
                    "provider": provider.clone(),
                    "model": model.clone(),
                    "outcome": "failed",
                    "error": short_error(&error),
                    "duration_ms": duration_ms(snapshot.started_at, snapshot.finished_at),
                }),
            );
            emit_task_event(&app, &registry, &task_id, "failed", error, Some(snapshot)).await;
        }
        return;
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
            "task_text": task.clone(),
        }),
    );

    let result = run_agent_task_on_fresh_page(
        app.clone(),
        task_id.clone(),
        runtime,
        &task,
        provider.as_deref(),
        model.as_deref(),
        Some(run_dir),
        Some(registry.clone()),
        telemetry.clone(),
        format!("task · {}", title_safe(&task)),
    )
    .await;

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
                    snapshot.turns = Some(outcome.turns);
                    snapshot.input_tokens = Some(outcome.input_tokens);
                    snapshot.output_tokens = Some(outcome.output_tokens);
                })
                .await
            {
                record_desktop_session(&snapshot, &outcome.final_text, "completed");
                telemetry.capture(
                    "socai_agent_task_end",
                    json!({
                        "task_id": task_id.clone(),
                        "run_id": outcome.run_id.clone(),
                        "provider": provider.clone(),
                        "model": model.clone(),
                        "outcome": "completed",
                        "turns": outcome.turns,
                        "input_tokens": outcome.input_tokens,
                        "output_tokens": outcome.output_tokens,
                        "duration_ms": duration_ms(snapshot.started_at, snapshot.finished_at),
                    }),
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
                })
                .await
            {
                if let Some(run_dir) = snapshot.run_dir.as_deref() {
                    let _ = mark_agent_run_status(run_dir, "failed", Some(&error));
                }
                record_desktop_session(&snapshot, &format!("[task failed: {error}]"), "failed");
                telemetry.capture(
                    "socai_agent_task_end",
                    json!({
                        "task_id": task_id.clone(),
                        "provider": provider.clone(),
                        "model": model.clone(),
                        "outcome": "failed",
                        "error": short_error(&error),
                        "duration_ms": duration_ms(snapshot.started_at, snapshot.finished_at),
                    }),
                );
                emit_task_event(&app, &registry, &task_id, "failed", error, Some(snapshot)).await;
            }
        }
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

fn record_desktop_session(snapshot: &AgentTaskSnapshot, assistant: &str, status: &str) {
    let Some(session_dir) = snapshot.session_dir.as_deref() else {
        return;
    };
    let Some(run_dir) = snapshot.run_dir.as_deref() else {
        return;
    };
    let Ok(mut session) = Session::load(session_dir) else {
        return;
    };
    session.record_run(&snapshot.task, assistant, &PathBuf::from(run_dir), status);
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_task_on_fresh_page(
    app: AppHandle,
    task_id: String,
    runtime: SocaiRuntime,
    task: &str,
    provider: Option<&str>,
    model: Option<&str>,
    run_dir: Option<PathBuf>,
    registry: Option<AgentTaskRegistry>,
    telemetry: DesktopTelemetry,
    title_label: String,
) -> Result<AgentRunOutcome> {
    let task = task.trim();
    if task.is_empty() {
        anyhow::bail!("task is empty");
    }
    let session_id = if let Some(registry) = &registry {
        registry
            .get(&task_id)
            .await
            .and_then(|snapshot| snapshot.session_dir)
            .and_then(|dir| {
                PathBuf::from(dir)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
    } else {
        None
    };

    ensure_llm_provider_configured_for(provider, model)?;
    let llm_provider = create_llm_provider_for(provider, model)?;
    let site = app_site()?;
    let page = Arc::new(runtime.create_page(site.home_url).await?);
    let target_id = page.target_id().to_string();
    label_controlled_page(&page, &title_label).await;
    if let Some(registry) = &registry {
        if let Some(snapshot) = registry
            .update(&task_id, |snapshot| {
                snapshot.target_id = Some(target_id.clone());
            })
            .await
        {
            emit_task_event(
                &app,
                registry,
                &task_id,
                "tab",
                "chrome tab marked as controlled by socai".into(),
                Some(snapshot),
            )
            .await;
        }
    }
    let outcome = async {
        let agent_tools = site.default_agent_tools.unwrap_or(site.agent_tools);
        let tools = agent_tools(page.clone(), llm_provider.clone()).await?;
        let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(256);
        let pump = pump_agent_task_events(app, registry.clone(), task_id.clone(), telemetry, rx);

        let agent_instructions = site
            .default_agent_instructions
            .unwrap_or(site.agent_instructions);
        let config = AgentRunConfig {
            extra_instructions: format!(
                "{}{}",
                agent_instructions(TAURI_AGENT_PREAMBLE),
                TAURI_CITATION_RULES
            ),
            enabled_sites: vec![site.id.to_string()],
            run_dir,
            session_id,
            ..AgentRunConfig::default()
        };
        let outcome = run_agent_with_tools(task, llm_provider, tools, config, tx).await;
        let _ = pump.await;
        let outcome = outcome?;

        Ok::<AgentRunOutcome, anyhow::Error>(AgentRunOutcome {
            run_id: outcome.run_id,
            run_dir: outcome.run_dir.display().to_string(),
            turns: outcome.turns,
            final_text: outcome.final_text,
            input_tokens: outcome.total_input_tokens,
            output_tokens: outcome.total_output_tokens,
        })
    }
    .await;
    if let Some(registry) = &registry {
        let _ = registry
            .update(&task_id, |snapshot| {
                snapshot.target_id = None;
            })
            .await;
    }
    close_page(page).await;
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
                    turn,
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
                    props.insert("turn".into(), json!(turn));
                    props.insert("sequence".into(), json!(sequence));
                    props.insert("duration_ms".into(), json!(tool_duration_ms));
                    props.insert("ok".into(), json!(error.is_none()));
                    props.insert("error".into(), json!(error.as_deref().map(short_error)));
                    // Same shared summarizer the CLI daemon uses: the search
                    // query is lifted into query_text/query_len (gated by
                    // query_text_enabled) and every other arg folds into
                    // metadata; result counts are extracted from the output.
                    // Tool OUTPUT text (note bodies) is never included.
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
    /// Chrome profile mode: "managed" | "existing" | "auto". Falls back to the
    /// product default ("existing") when the config key is unset.
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
    match std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        Some(home) => PathBuf::from(home)
            .join(relative)
            .to_string_lossy()
            .into_owned(),
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
}
