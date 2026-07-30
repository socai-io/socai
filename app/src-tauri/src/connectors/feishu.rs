use std::{
    future::pending,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::{write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shell::{process::CommandEvent, ShellExt};
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

use crate::{
    commands,
    tasks::{AgentTaskRegistry, AgentTaskSnapshot},
    telemetry::{short_error, DesktopTelemetry},
};

const DEFAULT_PROFILE: &str = "socai";
const USER_SCOPES: &str = "docx:document:create im:chat:read im:message.send_as_user im:message";
const DOCUMENT_SCOPES: &str = "docx:document:create";
const CHAT_LIST_SCOPES: &str = "im:chat:read";
const SEND_SCOPES: &str = "im:message.send_as_user im:message";
const REGISTRATION_FEISHU_URL: &str = "https://accounts.feishu.cn/oauth/v1/app/registration";
const REGISTRATION_LARK_URL: &str = "https://accounts.larksuite.com/oauth/v1/app/registration";
const CONNECT_CANCELLED: &str = "已取消连接飞书";
const APP_NAME: &str = "{user}的实习生socai";
const APP_DESCRIPTION: &str = "由 socai 创建，用于将研究总结导出为飞书文档并发送到群聊。";

#[derive(Default)]
pub struct FeishuState {
    connect_lock: Mutex<()>,
    connect_generation: AtomicU64,
}

#[derive(Default, Deserialize)]
struct AppRegistrationResponse {
    device_code: Option<String>,
    verification_uri_complete: Option<String>,
    interval: Option<u64>,
    #[serde(alias = "expire_in")]
    expires_in: Option<u64>,
    client_id: Option<String>,
    client_secret: Option<String>,
    user_info: Option<AppRegistrationUserInfo>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Default, Deserialize)]
struct AppRegistrationUserInfo {
    tenant_brand: Option<String>,
}

struct RegisteredApp {
    app_id: String,
    app_secret: String,
    brand: String,
}

#[derive(Serialize)]
pub struct FeishuStatus {
    configured: bool,
    connected: bool,
    identity: String,
    profile: String,
    user_name: Option<String>,
    message: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct FeishuAccount {
    profile: String,
    user_name: Option<String>,
    avatar_url: Option<String>,
    tenant_key: Option<String>,
    connected: bool,
    active: bool,
}

#[derive(Serialize)]
pub struct FeishuAccountIdentity {
    profile: String,
    avatar_url: Option<String>,
    tenant_key: Option<String>,
}

#[derive(Clone, Serialize)]
struct FeishuConnectEvent {
    stage: &'static str,
    state: &'static str,
    url: Option<String>,
}

#[derive(Serialize)]
pub struct FeishuDocument {
    document_id: String,
    url: String,
    title: String,
}

#[derive(Serialize)]
pub struct FeishuChat {
    chat_id: String,
    name: String,
    description: Option<String>,
}

#[derive(Serialize)]
pub struct FeishuMessage {
    message_id: String,
    chat_id: String,
}

struct CliOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

#[tauri::command]
pub async fn feishu_status(
    app: AppHandle,
    profile: Option<String>,
) -> Result<FeishuStatus, String> {
    let profile = resolve_profile(&app, profile.as_deref()).await?;
    Ok(read_status(&app, &profile).await)
}

#[tauri::command]
pub async fn feishu_accounts(app: AppHandle) -> Result<Vec<FeishuAccount>, String> {
    list_accounts(&app).await
}

#[tauri::command]
pub async fn feishu_account_identity(
    app: AppHandle,
    profile: String,
) -> Result<FeishuAccountIdentity, String> {
    let (avatar_url, tenant_key) = read_account_identity(&app, &profile).await?;
    Ok(FeishuAccountIdentity {
        profile,
        avatar_url,
        tenant_key,
    })
}

#[tauri::command]
pub async fn feishu_report_failure(
    tasks: State<'_, AgentTaskRegistry>,
    telemetry: State<'_, DesktopTelemetry>,
    task_id: String,
    destination: String,
    stage: String,
    duration_ms: u64,
    error: String,
) -> Result<(), String> {
    if !matches!(destination.as_str(), "setup" | "document" | "chat") {
        return Err("unknown Feishu failure destination".into());
    }
    if !matches!(
        stage.as_str(),
        "load_accounts"
            | "select_account"
            | "connect_account"
            | "reconnect_account"
            | "disconnect_account"
            | "prepare_document"
            | "open_document"
            | "load_chats"
    ) {
        return Err("unknown Feishu failure stage".into());
    }
    let run_id = tasks.get(&task_id).await.and_then(|task| task.run_id);
    let error = short_error(&format!("[{stage}] {}", short_error(&error)));
    telemetry.capture(
        "socai_feishu_export",
        json!({
            "task_id": task_id,
            "run_id": run_id,
            "destination": destination,
            "stage": stage,
            "outcome": "failed",
            "duration_ms": duration_ms,
            "error": error,
        }),
    );
    Ok(())
}

#[tauri::command]
pub async fn feishu_select_account(
    app: AppHandle,
    profile: String,
) -> Result<FeishuStatus, String> {
    let accounts = list_accounts(&app).await?;
    if !accounts.iter().any(|account| account.profile == profile) {
        return Err("这个飞书账户配置不存在，请重新连接".into());
    }
    set_active_profile(&app, &profile).await?;
    Ok(read_status(&app, &profile).await)
}

#[tauri::command]
pub async fn feishu_disconnect_account(
    app: AppHandle,
    state: State<'_, FeishuState>,
    profile: String,
) -> Result<(), String> {
    let _guard = state.connect_lock.lock().await;
    let accounts = list_accounts(&app).await?;
    if !accounts.iter().any(|account| account.profile == profile) {
        return Err("这个飞书账户配置不存在".into());
    }
    let args = if accounts.len() == 1 {
        vec!["config".into(), "remove".into()]
    } else {
        vec!["profile".into(), "remove".into(), profile]
    };
    let output = run_cli(&app, args, None).await?;
    if output.success {
        Ok(())
    } else {
        Err(cli_error_message(&output))
    }
}

#[tauri::command]
pub async fn feishu_connect(
    app: AppHandle,
    state: State<'_, FeishuState>,
    profile: Option<String>,
    new_account: bool,
) -> Result<FeishuStatus, String> {
    let run_id = state.connect_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let _guard = state.connect_lock.lock().await;
    if state.connect_generation.load(Ordering::SeqCst) != run_id {
        return Err(CONNECT_CANCELLED.into());
    }
    let accounts = list_accounts(&app).await.unwrap_or_default();
    let profile = if new_account {
        next_profile_name(&accounts)
    } else {
        profile
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                accounts
                    .iter()
                    .find(|account| account.active)
                    .map(|account| account.profile.clone())
            })
            .unwrap_or_else(|| DEFAULT_PROFILE.into())
    };
    let configured = accounts.iter().any(|account| account.profile == profile);
    let current = read_status(&app, &profile).await;
    if current.connected && !new_account {
        set_active_profile(&app, &profile).await?;
        return Ok(current);
    }

    if new_account || !configured {
        emit_connect(&app, "app", "starting", None);
        let registered = register_app(&app, &state, run_id).await?;
        save_registered_app(&app, &profile, registered).await?;
        emit_connect(&app, "app", "completed", None);
    }

    emit_connect(&app, "user", "starting", None);
    let auth_error = run_streaming_auth(
        &app,
        "user",
        vec![
            "--profile".into(),
            profile.clone(),
            "auth".into(),
            "login".into(),
            "--scope".into(),
            USER_SCOPES.into(),
            "--json".into(),
        ],
        Some((&state.connect_generation, run_id)),
    )
    .await
    .err();
    emit_connect(&app, "user", "completed", None);
    if auth_error.as_deref() == Some(CONNECT_CANCELLED) {
        return Err(CONNECT_CANCELLED.into());
    }

    // The CLI can finish the device flow and persist a valid token while its
    // process still exits without a usable status code. Trust the persisted
    // identity, with a short propagation window, instead of surfacing its last
    // diagnostic line and forcing the user to click "retry".
    let status = wait_for_user_identity(&app, &profile).await;
    if !status.connected {
        return Err(status
            .message
            .or(auth_error)
            .unwrap_or_else(|| "飞书授权已完成，但尚未检测到可用身份；请稍等几秒后重试".into()));
    }
    set_active_profile(&app, &profile).await?;
    Ok(status)
}

#[tauri::command]
pub fn feishu_cancel_connect(state: State<'_, FeishuState>) {
    state.connect_generation.fetch_add(1, Ordering::SeqCst);
}

#[tauri::command]
pub async fn feishu_export_task(
    app: AppHandle,
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
    profile: String,
    content: String,
) -> Result<FeishuDocument, String> {
    let started_at = Instant::now();
    let telemetry_task_id = task_id.clone();
    let run_id = tasks.get(&task_id).await.and_then(|task| task.run_id);
    let result = export_task(&app, &tasks, task_id, profile, content).await;
    capture_export_result(
        app.state::<DesktopTelemetry>().inner(),
        &telemetry_task_id,
        run_id.as_deref(),
        "document",
        started_at,
        &result,
    );
    result
}

async fn export_task(
    app: &AppHandle,
    tasks: &AgentTaskRegistry,
    task_id: String,
    profile: String,
    content: String,
) -> Result<FeishuDocument, String> {
    require_connected(app, &profile).await?;
    wait_for_scopes(app, &profile, DOCUMENT_SCOPES, "创建飞书文档").await?;
    let task = tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    if content.trim().is_empty() {
        return Err("这个答案没有可导出的内容".into());
    }
    let run_dir = task
        .run_dir
        .as_deref()
        .ok_or_else(|| "这个任务没有运行产物目录".to_string())?;
    let export_content = hydrate_note_links(&task, &content);
    let export_path = write_export_file(Path::new(run_dir), &export_content)?;
    let export_name = export_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "无法生成飞书导出文件名".to_string())?
        .to_string();

    let title = compact_title(&task.task);
    let output_result = run_cli(
        app,
        vec![
            "--profile".into(),
            profile,
            "docs".into(),
            "+create".into(),
            "--as".into(),
            "user".into(),
            "--doc-format".into(),
            "markdown".into(),
            "--title".into(),
            title.clone(),
            "--content".into(),
            format!("@./{export_name}"),
            "--format".into(),
            "json".into(),
        ],
        Some(Path::new(run_dir)),
    )
    .await;
    let _ = std::fs::remove_file(&export_path);
    let output = output_result?;
    let value = require_cli_success(output)?;
    let data = cli_data(&value);
    let document = data
        .get("document")
        .and_then(Value::as_object)
        .ok_or_else(|| "飞书已返回成功，但响应里没有 document".to_string())?;
    let document_id = document
        .get("document_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let url = document
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if document_id.is_empty() || url.is_empty() {
        return Err("飞书已创建文档，但响应里缺少文档 ID 或链接".into());
    }

    Ok(FeishuDocument {
        document_id,
        url,
        title,
    })
}

#[tauri::command]
pub async fn feishu_list_chats(app: AppHandle, profile: String) -> Result<Vec<FeishuChat>, String> {
    require_connected(&app, &profile).await?;
    wait_for_scopes(&app, &profile, CHAT_LIST_SCOPES, "读取群聊列表").await?;
    let output = run_cli(
        &app,
        vec![
            "--profile".into(),
            profile,
            "im".into(),
            "+chat-list".into(),
            "--as".into(),
            "user".into(),
            "--sort".into(),
            "active_time".into(),
            "--page-size".into(),
            "50".into(),
            "--format".into(),
            "json".into(),
        ],
        None,
    )
    .await?;
    let value = require_cli_success(output)?;
    let chats = cli_data(&value)
        .get("chats")
        .and_then(Value::as_array)
        .ok_or_else(|| "飞书响应里没有群聊列表".to_string())?;

    Ok(chats
        .iter()
        .filter_map(|chat| {
            let chat_id = chat.get("chat_id")?.as_str()?.to_string();
            let name = chat
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("未命名群聊")
                .to_string();
            let description = chat
                .get("description")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_string);
            Some(FeishuChat {
                chat_id,
                name,
                description,
            })
        })
        .collect())
}

#[tauri::command]
pub async fn feishu_send_task_to_chat(
    app: AppHandle,
    state: State<'_, FeishuState>,
    tasks: State<'_, AgentTaskRegistry>,
    task_id: String,
    profile: String,
    content: String,
    chat_id: String,
) -> Result<FeishuMessage, String> {
    let started_at = Instant::now();
    let telemetry_task_id = task_id.clone();
    let run_id = tasks.get(&task_id).await.and_then(|task| task.run_id);
    let result = send_task_to_chat(&app, &state, &tasks, task_id, profile, content, chat_id).await;
    capture_export_result(
        app.state::<DesktopTelemetry>().inner(),
        &telemetry_task_id,
        run_id.as_deref(),
        "chat",
        started_at,
        &result,
    );
    result
}

async fn send_task_to_chat(
    app: &AppHandle,
    state: &State<'_, FeishuState>,
    tasks: &AgentTaskRegistry,
    task_id: String,
    profile: String,
    content: String,
    chat_id: String,
) -> Result<FeishuMessage, String> {
    require_connected(app, &profile).await?;
    ensure_send_scopes(app, state, &profile).await?;
    if !chat_id.starts_with("oc_") {
        return Err("无效的飞书群聊 ID".into());
    }
    let task = tasks
        .get(&task_id)
        .await
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    if content.trim().is_empty() {
        return Err("这个答案没有可发送的内容".into());
    }
    let content = hydrate_note_links(&task, &content);
    let markdown = format!("**{}**\n\n{}", compact_title(&task.task), content.trim());
    let output = run_cli(
        app,
        vec![
            "--profile".into(),
            profile,
            "im".into(),
            "+messages-send".into(),
            "--as".into(),
            "user".into(),
            "--chat-id".into(),
            chat_id.clone(),
            "--markdown".into(),
            markdown,
            "--format".into(),
            "json".into(),
        ],
        None,
    )
    .await?;
    let value = require_cli_success(output)?;
    let data = cli_data(&value);
    let message_id = data
        .get("message_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let returned_chat = data
        .get("chat_id")
        .and_then(Value::as_str)
        .unwrap_or(&chat_id)
        .to_string();
    if message_id.is_empty() {
        return Err("飞书已返回成功，但响应里没有消息 ID".into());
    }
    Ok(FeishuMessage {
        message_id,
        chat_id: returned_chat,
    })
}

fn capture_export_result<T>(
    telemetry: &DesktopTelemetry,
    task_id: &str,
    run_id: Option<&str>,
    destination: &str,
    started_at: Instant,
    result: &Result<T, String>,
) {
    let (outcome, error) = match result {
        Ok(_) => ("completed", None),
        Err(error) => ("failed", Some(short_error(error))),
    };
    let stage = match destination {
        "document" => "export_document",
        "chat" => "send_chat",
        _ => "unknown",
    };
    telemetry.capture(
        "socai_feishu_export",
        json!({
            "task_id": task_id,
            "run_id": run_id,
            "destination": destination,
            "stage": stage,
            "outcome": outcome,
            "duration_ms": u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            "error": error,
        }),
    );
}

async fn require_connected(app: &AppHandle, profile: &str) -> Result<(), String> {
    let status = read_status(app, profile).await;
    if status.connected {
        Ok(())
    } else {
        Err(status.message.unwrap_or_else(|| "请先连接飞书".to_string()))
    }
}

async fn read_status(app: &AppHandle, profile: &str) -> FeishuStatus {
    let output = match run_cli(
        app,
        vec![
            "--profile".into(),
            profile.into(),
            "auth".into(),
            "status".into(),
            "--json".into(),
        ],
        None,
    )
    .await
    {
        Ok(output) => output,
        Err(message) => {
            return FeishuStatus {
                configured: false,
                connected: false,
                identity: "none".into(),
                profile: profile.into(),
                user_name: None,
                message: Some(message),
            }
        }
    };

    let text = format!("{}\n{}", output.stdout, output.stderr);
    let value = parse_json(&output.stdout);
    let identity = value
        .as_ref()
        .and_then(|root| find_string(root, "identity"))
        .unwrap_or_else(|| "none".into());
    let user_name = value
        .as_ref()
        .and_then(|root| find_string(root, "userName"));
    let connected = output.success && identity == "user";
    let missing = text.contains(&format!("profile \"{profile}\" not found"))
        || text.contains("not configured")
        || text.contains("no active profile")
        || text.contains("configuration file not found");
    FeishuStatus {
        configured: !missing,
        connected,
        identity,
        profile: profile.into(),
        user_name,
        message: if connected {
            None
        } else {
            Some(cli_error_message(&output))
        },
    }
}

async fn wait_for_user_identity(app: &AppHandle, profile: &str) -> FeishuStatus {
    let mut status = read_status(app, profile).await;
    for _ in 0..20 {
        if status.connected {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        status = read_status(app, profile).await;
    }
    status
}

async fn list_accounts(app: &AppHandle) -> Result<Vec<FeishuAccount>, String> {
    let output = run_cli_bounded(
        app,
        vec!["profile".into(), "list".into()],
        None,
        Duration::from_secs(5),
        "读取飞书账户超时，请重试",
    )
    .await?;
    if !output.success {
        let combined = format!("{}\n{}", output.stdout, output.stderr);
        if combined.contains("not configured") || combined.contains("no active profile") {
            return Ok(Vec::new());
        }
        return Err(cli_error_message(&output));
    }
    let value = parse_json(&output.stdout).unwrap_or_else(|| Value::Array(Vec::new()));
    let items = value
        .as_array()
        .ok_or_else(|| "无法解析飞书账户列表".to_string())?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let profile = item.get("name")?.as_str()?.to_string();
            let user_name = item
                .get("user")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string);
            // lark-cli reports `needs_refresh` when the short-lived access
            // token is due for refresh but the refresh token is still usable.
            // The next user API call refreshes it automatically, so this is
            // still a connected account rather than a reason to re-authorize.
            let connected = matches!(
                item.get("tokenStatus").and_then(Value::as_str),
                Some("valid" | "needs_refresh")
            );
            let active = item.get("active").and_then(Value::as_bool).unwrap_or(false);
            Some(FeishuAccount {
                profile,
                user_name,
                avatar_url: None,
                tenant_key: None,
                connected,
                active,
            })
        })
        .collect())
}

async fn resolve_profile(app: &AppHandle, requested: Option<&str>) -> Result<String, String> {
    if let Some(profile) = requested.filter(|profile| !profile.trim().is_empty()) {
        return Ok(profile.to_string());
    }
    let accounts = list_accounts(app).await?;
    Ok(accounts
        .iter()
        .find(|account| account.active)
        .or_else(|| accounts.first())
        .map(|account| account.profile.clone())
        .unwrap_or_else(|| DEFAULT_PROFILE.into()))
}

fn next_profile_name(accounts: &[FeishuAccount]) -> String {
    if !accounts
        .iter()
        .any(|account| account.profile == DEFAULT_PROFILE)
    {
        return DEFAULT_PROFILE.into();
    }
    for index in 2..10_000 {
        let candidate = format!("{DEFAULT_PROFILE}-{index}");
        if !accounts.iter().any(|account| account.profile == candidate) {
            return candidate;
        }
    }
    format!("{DEFAULT_PROFILE}-{}", now_millis())
}

async fn set_active_profile(app: &AppHandle, profile: &str) -> Result<(), String> {
    let output = run_cli(
        app,
        vec!["profile".into(), "use".into(), profile.into()],
        None,
    )
    .await?;
    if output.success {
        Ok(())
    } else {
        Err(cli_error_message(&output))
    }
}

async fn register_app(
    app: &AppHandle,
    state: &FeishuState,
    run_id: u64,
) -> Result<RegisteredApp, String> {
    let client = reqwest::Client::new();
    let begin = registration_request(
        &client,
        REGISTRATION_FEISHU_URL,
        &[
            ("action", "begin"),
            ("archetype", "PersonalAgent"),
            ("auth_method", "client_secret"),
            ("request_user_info", "open_id"),
        ],
        &state.connect_generation,
        run_id,
    )
    .await?;
    if let Some(error) = begin.error {
        return Err(registration_error(
            &error,
            begin.error_description.as_deref(),
        ));
    }
    let device_code = begin
        .device_code
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "飞书应用创建请求没有返回 device code".to_string())?;
    let verification_url = build_app_registration_url(
        begin
            .verification_uri_complete
            .as_deref()
            .ok_or_else(|| "飞书应用创建请求没有返回确认链接".to_string())?,
    )?;
    emit_connect(
        app,
        "app",
        "awaiting_authorization",
        Some(verification_url.clone()),
    );
    if let Err(error) = commands::open_external(verification_url) {
        emit_connect(app, "app", "open_failed", Some(error));
    }

    let mut endpoint = REGISTRATION_FEISHU_URL;
    let mut interval = begin.interval.unwrap_or(5).max(1);
    let expires_in = begin.expires_in.unwrap_or(600).max(1);
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    let mut switched_domain = false;
    let mut wait_before_poll = false;

    loop {
        if Instant::now() >= deadline {
            return Err("飞书应用创建确认已过期，请重试".into());
        }
        if wait_before_poll {
            cancellable_sleep(
                Duration::from_secs(interval),
                &state.connect_generation,
                run_id,
            )
            .await?;
        }
        wait_before_poll = true;

        let response = registration_request(
            &client,
            endpoint,
            &[("action", "poll"), ("device_code", device_code.as_str())],
            &state.connect_generation,
            run_id,
        )
        .await?;

        if !switched_domain
            && response
                .user_info
                .as_ref()
                .and_then(|info| info.tenant_brand.as_deref())
                == Some("lark")
        {
            endpoint = REGISTRATION_LARK_URL;
            switched_domain = true;
            wait_before_poll = false;
            continue;
        }

        if let (Some(app_id), Some(app_secret)) = (
            response.client_id.filter(|value| !value.is_empty()),
            response.client_secret.filter(|value| !value.is_empty()),
        ) {
            let brand = if endpoint == REGISTRATION_LARK_URL {
                "lark"
            } else {
                "feishu"
            };
            return Ok(RegisteredApp {
                app_id,
                app_secret,
                brand: brand.into(),
            });
        }

        match response.error.as_deref() {
            Some("authorization_pending") | None => {}
            Some("slow_down") => interval = (interval + 5).min(60),
            Some("access_denied") => return Err("已取消创建飞书应用".into()),
            Some("expired_token" | "invalid_grant") => {
                return Err("飞书应用创建确认已过期，请重试".into())
            }
            Some(error) => {
                return Err(registration_error(
                    error,
                    response.error_description.as_deref(),
                ))
            }
        }
    }
}

async fn registration_request(
    client: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, &str)],
    generation: &AtomicU64,
    run_id: u64,
) -> Result<AppRegistrationResponse, String> {
    let request = async {
        let response = client
            .post(endpoint)
            .form(form)
            .send()
            .await
            .map_err(|error| format!("无法连接飞书应用注册服务：{error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| format!("无法读取飞书应用注册响应：{error}"))?;
        serde_json::from_str::<AppRegistrationResponse>(&body).map_err(|error| {
            format!(
                "无法解析飞书应用注册响应（HTTP {}）：{}",
                status.as_u16(),
                error
            )
        })
    };
    tokio::select! {
        result = request => result,
        _ = wait_until_cancelled(generation, run_id) => Err(CONNECT_CANCELLED.into()),
    }
}

fn build_app_registration_url(candidate: &str) -> Result<String, String> {
    let mut parsed =
        url::Url::parse(candidate).map_err(|error| format!("飞书应用创建链接无效：{error}"))?;
    if !matches!(
        parsed.host_str(),
        Some("open.feishu.cn") | Some("open.larksuite.com")
    ) {
        return Err("飞书应用创建链接来自未知域名".into());
    }
    let addons = encoded_app_addons()?;
    let retained = parsed
        .query_pairs()
        .filter(|(key, _)| {
            !matches!(
                key.as_ref(),
                "from" | "source" | "tp" | "avatar" | "name" | "desc" | "addons" | "createOnly"
            )
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    parsed.set_query(None);
    let mut query = parsed.query_pairs_mut();
    for (key, value) in retained {
        query.append_pair(&key, &value);
    }
    query
        .append_pair("from", "sdk")
        .append_pair("source", "node-sdk/socai")
        .append_pair("tp", "sdk")
        .append_pair("name", APP_NAME)
        .append_pair("desc", APP_DESCRIPTION)
        .append_pair("addons", &addons)
        .append_pair("createOnly", "true");
    drop(query);
    Ok(parsed.into())
}

fn registration_error(error: &str, description: Option<&str>) -> String {
    let detail = description
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(error);
    format!("飞书应用创建失败：{detail}")
}

async fn save_registered_app(
    app: &AppHandle,
    profile: &str,
    registered: RegisteredApp,
) -> Result<(), String> {
    let output = run_cli_with_stdin(
        app,
        vec![
            "profile".into(),
            "add".into(),
            "--name".into(),
            profile.into(),
            "--app-id".into(),
            registered.app_id,
            "--app-secret-stdin".into(),
            "--brand".into(),
            registered.brand,
            "--lang".into(),
            "zh_cn".into(),
            "--use".into(),
        ],
        format!("{}\n", registered.app_secret),
    )
    .await?;
    if output.success {
        Ok(())
    } else {
        Err(cli_error_message(&output))
    }
}

async fn run_streaming_auth(
    app: &AppHandle,
    stage: &'static str,
    args: Vec<String>,
    cancel: Option<(&AtomicU64, u64)>,
) -> Result<(), String> {
    let command = app
        .shell()
        .sidecar("lark-cli")
        .map_err(|error| format!("无法启动飞书 CLI：{error}"))?
        .args(args);
    let (mut events, child) = command
        .spawn()
        .map_err(|error| format!("无法启动飞书 CLI：{error}"))?;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut opened_url = None;
    let mut exit_code = None;

    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = wait_for_optional_cancel(cancel) => {
                let _ = child.kill();
                return Err(CONNECT_CANCELLED.into());
            }
        };
        let Some(event) = event else {
            break;
        };
        match event {
            CommandEvent::Stdout(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                stdout.push_str(&text);
                stdout.push('\n');
                maybe_open_authorization_url(app, stage, &stdout, &mut opened_url);
            }
            CommandEvent::Stderr(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                stderr.push_str(&text);
                stderr.push('\n');
                maybe_open_authorization_url(app, stage, &stderr, &mut opened_url);
            }
            CommandEvent::Error(error) => stderr.push_str(&error),
            CommandEvent::Terminated(payload) => {
                exit_code = payload.code;
                break;
            }
            _ => {}
        }
    }

    let output = CliOutput {
        success: exit_code == Some(0),
        stdout,
        stderr,
    };
    if output.success {
        Ok(())
    } else {
        Err(cli_error_message(&output))
    }
}

fn maybe_open_authorization_url(
    app: &AppHandle,
    stage: &'static str,
    text: &str,
    opened_url: &mut Option<String>,
) {
    if opened_url.is_some() {
        return;
    }
    let Some(url) = extract_authorization_url(stage, text) else {
        return;
    };
    *opened_url = Some(url.clone());
    emit_connect(app, stage, "awaiting_authorization", Some(url.clone()));
    if let Err(error) = commands::open_external(url) {
        emit_connect(app, stage, "open_failed", Some(error));
    }
}

fn extract_authorization_url(stage: &str, text: &str) -> Option<String> {
    if stage != "user" {
        return None;
    }
    let value = parse_json(text)?;
    let candidate = find_string(&value, "verification_uri_complete")
        .or_else(|| find_string(&value, "verification_url"))?;
    validated_user_authorization_url(&candidate)
}

fn validated_user_authorization_url(candidate: &str) -> Option<String> {
    let parsed = url::Url::parse(candidate).ok()?;
    if !matches!(
        parsed.host_str(),
        Some("accounts.feishu.cn")
            | Some("accounts.larksuite.com")
            | Some("open.feishu.cn")
            | Some("open.larksuite.com")
    ) || parsed.path() == "/oauth/v1/app/registration"
    {
        return None;
    }
    Some(parsed.into())
}

async fn read_account_identity(
    app: &AppHandle,
    profile: &str,
) -> Result<(Option<String>, Option<String>), String> {
    let output = run_cli_bounded(
        app,
        vec![
            "--profile".into(),
            profile.into(),
            "api".into(),
            "GET".into(),
            "/open-apis/authen/v1/user_info".into(),
            "--as".into(),
            "user".into(),
        ],
        None,
        Duration::from_secs(8),
        "读取飞书账户信息超时",
    )
    .await?;
    if !output.success {
        return Err(cli_error_message(&output));
    }
    let value = parse_json(&output.stdout).ok_or_else(|| cli_error_message(&output))?;
    let data = cli_data(&value);
    let avatar_url = data
        .get("avatar_thumb")
        .or_else(|| data.get("avatar_url"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let tenant_key = data
        .get("tenant_key")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((avatar_url, tenant_key))
}

fn encoded_app_addons() -> Result<String, String> {
    let payload = format!(
        r#"{{"preset":false,"scopes":{{"user":{}}}}}"#,
        serde_json::to_string(&USER_SCOPES.split_whitespace().collect::<Vec<_>>())
            .map_err(|error| error.to_string())?
    );
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(payload.as_bytes())
        .map_err(|error| error.to_string())?;
    let compressed = encoder.finish().map_err(|error| error.to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(compressed))
}

fn emit_connect(app: &AppHandle, stage: &'static str, state: &'static str, url: Option<String>) {
    let _ = app.emit("feishu:connect", FeishuConnectEvent { stage, state, url });
}

async fn wait_until_cancelled(generation: &AtomicU64, run_id: u64) {
    loop {
        if generation.load(Ordering::SeqCst) != run_id {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_optional_cancel(cancel: Option<(&AtomicU64, u64)>) {
    match cancel {
        Some((generation, run_id)) => wait_until_cancelled(generation, run_id).await,
        None => pending::<()>().await,
    }
}

async fn cancellable_sleep(
    duration: Duration,
    generation: &AtomicU64,
    run_id: u64,
) -> Result<(), String> {
    tokio::select! {
        _ = tokio::time::sleep(duration) => Ok(()),
        _ = wait_until_cancelled(generation, run_id) => Err(CONNECT_CANCELLED.into()),
    }
}

async fn run_cli_with_stdin(
    app: &AppHandle,
    args: Vec<String>,
    stdin: String,
) -> Result<CliOutput, String> {
    let command = app
        .shell()
        .sidecar("lark-cli")
        .map_err(|error| format!("无法启动飞书 CLI：{error}"))?
        .args(args);
    let (mut events, mut child) = command
        .spawn()
        .map_err(|error| format!("无法启动飞书 CLI：{error}"))?;
    child
        .write(stdin.as_bytes())
        .map_err(|error| format!("无法安全保存飞书应用凭证：{error}"))?;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = None;
    while let Some(event) = events.recv().await {
        match event {
            CommandEvent::Stdout(bytes) => {
                stdout.push_str(&String::from_utf8_lossy(&bytes));
                stdout.push('\n');
            }
            CommandEvent::Stderr(bytes) => {
                stderr.push_str(&String::from_utf8_lossy(&bytes));
                stderr.push('\n');
            }
            CommandEvent::Error(error) => stderr.push_str(&error),
            CommandEvent::Terminated(payload) => {
                exit_code = payload.code;
                break;
            }
            _ => {}
        }
    }
    Ok(CliOutput {
        success: exit_code == Some(0),
        stdout,
        stderr,
    })
}

async fn run_cli(
    app: &AppHandle,
    args: Vec<String>,
    current_dir: Option<&Path>,
) -> Result<CliOutput, String> {
    run_cli_bounded(
        app,
        args,
        current_dir,
        Duration::from_secs(60),
        "飞书 CLI 执行超时，请重试",
    )
    .await
}

async fn run_cli_bounded(
    app: &AppHandle,
    args: Vec<String>,
    current_dir: Option<&Path>,
    timeout: Duration,
    timeout_message: &str,
) -> Result<CliOutput, String> {
    let mut command = app
        .shell()
        .sidecar("lark-cli")
        .map_err(|error| format!("无法启动飞书 CLI：{error}"))?
        .args(args);
    if let Some(dir) = current_dir {
        command = command.current_dir(dir);
    }
    let (mut events, child) = command
        .spawn()
        .map_err(|error| format!("无法启动飞书 CLI：{error}"))?;
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = None;

    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = &mut deadline => {
                let _ = child.kill();
                return Err(timeout_message.into());
            }
        };
        let Some(event) = event else {
            break;
        };
        match event {
            CommandEvent::Stdout(bytes) => {
                stdout.push_str(&String::from_utf8_lossy(&bytes));
                stdout.push('\n');
            }
            CommandEvent::Stderr(bytes) => {
                stderr.push_str(&String::from_utf8_lossy(&bytes));
                stderr.push('\n');
            }
            CommandEvent::Error(error) => stderr.push_str(&error),
            CommandEvent::Terminated(payload) => {
                exit_code = payload.code;
                break;
            }
            _ => {}
        }
    }

    Ok(CliOutput {
        success: exit_code == Some(0),
        stdout,
        stderr,
    })
}

async fn wait_for_scopes(
    app: &AppHandle,
    profile: &str,
    scopes: &str,
    action: &str,
) -> Result<(), String> {
    for attempt in 0..16 {
        if scope_check(app, profile, scopes).await? {
            return Ok(());
        }
        if attempt < 15 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
    Err(format!(
        "飞书已完成授权，但“{action}”权限仍在审批或尚未生效。请等待管理员审批后重试。"
    ))
}

async fn ensure_send_scopes(
    app: &AppHandle,
    state: &State<'_, FeishuState>,
    profile: &str,
) -> Result<(), String> {
    if scope_check(app, profile, SEND_SCOPES).await? {
        return Ok(());
    }
    if !app_has_scopes(app, profile, SEND_SCOPES).await? {
        return Err("“以用户身份发送消息”权限仍待企业管理员审批。审批通过后再次发送即可。".into());
    }

    // Admin approval updates the application's available scopes, while the
    // existing user token can still carry the older scope set. Refresh only
    // that user grant here; this never recreates or reconnects the app.
    let _guard = state.connect_lock.lock().await;
    if scope_check(app, profile, SEND_SCOPES).await? {
        return Ok(());
    }
    emit_connect(app, "user", "starting", None);
    let auth_error = run_streaming_auth(
        app,
        "user",
        vec![
            "--profile".into(),
            profile.into(),
            "auth".into(),
            "login".into(),
            "--scope".into(),
            SEND_SCOPES.into(),
            "--json".into(),
        ],
        None,
    )
    .await
    .err();
    emit_connect(app, "user", "completed", None);
    let status = wait_for_user_identity(app, profile).await;
    if scope_check(app, profile, SEND_SCOPES).await? {
        return Ok(());
    }
    Err(auth_error
        .or(status.message)
        .unwrap_or_else(|| "发送权限尚未生效，请稍后再试".into()))
}

async fn app_has_scopes(app: &AppHandle, profile: &str, scopes: &str) -> Result<bool, String> {
    let output = run_cli(
        app,
        vec![
            "--profile".into(),
            profile.into(),
            "auth".into(),
            "scopes".into(),
            "--json".into(),
        ],
        None,
    )
    .await?;
    if !output.success {
        return Err(cli_error_message(&output));
    }
    let value = parse_json(&output.stdout).ok_or_else(|| cli_error_message(&output))?;
    let available = value
        .get("userScopes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    Ok(scopes
        .split_whitespace()
        .all(|scope| available.contains(scope)))
}

async fn scope_check(app: &AppHandle, profile: &str, scopes: &str) -> Result<bool, String> {
    let output = run_cli(
        app,
        vec![
            "--profile".into(),
            profile.into(),
            "auth".into(),
            "check".into(),
            "--scope".into(),
            scopes.into(),
            "--json".into(),
        ],
        None,
    )
    .await?;
    let value = parse_json(&output.stdout).ok_or_else(|| cli_error_message(&output))?;
    Ok(value.get("ok").and_then(Value::as_bool).unwrap_or(false))
}

fn hydrate_note_links(task: &AgentTaskSnapshot, content: &str) -> String {
    let mut result = content.to_string();
    for (run_dir, _) in crate::timeline::conversation_run_dirs(task) {
        for note in socai_core::agent::note_store::load_notes(&run_dir) {
            let Some(note_id) = note.get("note_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(url) = note
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| url.contains("xsec_token="))
            else {
                continue;
            };
            let title = note
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(note_id);
            result = replace_note_reference(&result, note_id, title, url);
        }
    }
    result
}

fn replace_note_reference(content: &str, note_id: &str, title: &str, url: &str) -> String {
    let mut result = content.to_string();
    for scheme in ["note:", "#note:"] {
        let target = format!("]({scheme}{note_id})");
        while let Some(end) = result.find(&target) {
            let Some(start) = result[..end].rfind('[') else {
                break;
            };
            let safe_title = title.replace('[', "\\[").replace(']', "\\]");
            let replacement = format!("[{safe_title}]({url})");
            result.replace_range(start..end + target.len(), &replacement);
        }
    }
    result
}

fn write_export_file(run_dir: &Path, content: &str) -> Result<PathBuf, String> {
    for suffix in 0..100 {
        let path = run_dir.join(format!(".socai-feishu-export-{}-{suffix}.md", now_millis()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(content.as_bytes())
                    .map_err(|error| format!("无法准备飞书导出内容：{error}"))?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("无法准备飞书导出内容：{error}")),
        }
    }
    Err("无法生成唯一的飞书导出文件".into())
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn require_cli_success(output: CliOutput) -> Result<Value, String> {
    if !output.success {
        return Err(cli_error_message(&output));
    }
    parse_json(&output.stdout).ok_or_else(|| {
        let detail = output.stdout.trim();
        if detail.is_empty() {
            "飞书 CLI 没有返回 JSON".into()
        } else {
            format!("无法解析飞书 CLI 响应：{detail}")
        }
    })
}

fn parse_json(text: &str) -> Option<Value> {
    serde_json::from_str(text.trim()).ok().or_else(|| {
        text.char_indices()
            .filter(|(_, ch)| *ch == '{' || *ch == '[')
            .find_map(|(start, _)| {
                serde_json::Deserializer::from_str(&text[start..])
                    .into_iter::<Value>()
                    .next()
                    .and_then(Result::ok)
            })
    })
}

fn cli_data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn cli_error_message(output: &CliOutput) -> String {
    for text in [&output.stdout, &output.stderr] {
        if let Some(value) = parse_json(text) {
            for key in ["message", "error", "hint"] {
                if let Some(message) = find_string(&value, key) {
                    if !message.trim().is_empty() {
                        return friendly_cli_error(&message);
                    }
                }
            }
        }
    }
    let useful_lines = output
        .stderr
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !matches!(*line, "{" | "}" | "}," | "[" | "]")
                && !line.starts_with("https://")
        })
        .collect::<Vec<_>>();
    if !useful_lines.is_empty() {
        let message = useful_lines
            .into_iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return friendly_cli_error(&message);
    }
    "飞书 CLI 执行失败".into()
}

fn friendly_cli_error(message: &str) -> String {
    if message.contains("im:message.send_as_user")
        || message.contains("missing required scope(s): im:message")
    {
        return "“以用户身份发送消息”权限仍待企业管理员审批。审批通过后再次发送即可。".into();
    }
    if message.contains("device-flow: token response received") {
        return "飞书已经收到授权，正在等待账户和权限状态同步，请稍后重试。".into();
    }
    message.to_string()
}

fn find_string(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(text) = map.get(key).and_then(Value::as_str) {
                return Some(text.to_string());
            }
            map.values().find_map(|value| find_string(value, key))
        }
        Value::Array(items) => items.iter().find_map(|value| find_string(value, key)),
        _ => None,
    }
}

fn compact_title(title: &str) -> String {
    let title = title.trim();
    if title.chars().count() <= 80 {
        return title.to_string();
    }
    format!("{}…", title.chars().take(79).collect::<String>())
}
