mod artifact_tool;
mod commands;
mod connectors;
mod tasks;
mod telemetry;
mod timeline;

use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use serde_json::json;
use socai_core::runtime::{RuntimeBrowserEvent, SocaiRuntime};
use tasks::AgentTaskRegistry;
use tauri::{Emitter, Manager};
use telemetry::{duration_ms, DesktopTelemetry};

const BROWSER_RECOVERY_GRACE: Duration = Duration::from_secs(15);

async fn interrupt_missing_browser_targets(
    active_targets: HashSet<String>,
    interruption_reason: String,
    tasks: AgentTaskRegistry,
    telemetry: DesktopTelemetry,
    handle: tauri::AppHandle,
) {
    for (mut snapshot, abort_handle) in tasks
        .interrupt_missing_targets(&active_targets, &interruption_reason)
        .await
    {
        let task_id = snapshot.task_id.clone();
        if snapshot.provider.as_deref() == Some("socai") && socai_core::cloud::pro_activated() {
            if let Some(settlement) =
                commands::settle_hosted_task_with_retry(&task_id, "interrupted").await
            {
                if let Some(updated) = tasks
                    .update(&task_id, |task| {
                        task.points_used =
                            commands::visible_billed_points(task.provider.as_deref(), &settlement);
                    })
                    .await
                {
                    snapshot = updated;
                }
            }
        }
        if let Some(handle) = abort_handle {
            handle.abort();
        }
        commands::persist_run_points_used(snapshot.run_dir.as_deref(), snapshot.points_used);
        commands::record_interrupted_run(&snapshot, &interruption_reason);
        if let Some(run_dir) = snapshot.run_dir.as_deref() {
            commands::upload_terminal_run_trace(run_dir, "interrupted", &telemetry).await;
        }
        telemetry.capture(
            "socai_agent_task_end",
            json!({
                "task_id": task_id.clone(),
                "provider": snapshot.provider.clone(),
                "run_id": snapshot.run_id.clone(),
                "model": snapshot.model.clone(),
                "outcome": "interrupted",
                "steps": snapshot.steps,
                "input_tokens": snapshot.input_tokens,
                "output_tokens": snapshot.output_tokens,
                "points_used": snapshot.points_used,
                "duration_ms": duration_ms(snapshot.started_at, snapshot.finished_at),
                "error": crate::telemetry::short_error(&interruption_reason),
            }),
        );
        commands::emit_task_event(
            &handle,
            &tasks,
            &task_id,
            "interrupted",
            interruption_reason.clone(),
            Some(snapshot),
        )
        .await;
    }
}

/// macOS: attach an empty unified toolbar so AppKit itself lays the native
/// traffic lights out on a tall (~52px) titlebar, vertically centered — the
/// same centerline as the 52px `.is-macos .topbar` header in styles.css.
///
/// We previously repositioned the buttons by hand (tao's inset math) on every
/// show/resize/focus event, but AppKit re-lays-out the titlebar on each live-
/// resize frame *before* our handler runs, so the lights flickered between the
/// default and custom spots — and the private view-hierarchy math drifted
/// across macOS 26 point releases. With a unified toolbar the placement is
/// AppKit's own layout, applied atomically with every frame: nothing to
/// re-apply, nothing to flicker. The window's `titleBarStyle: Overlay`
/// (transparent titlebar + full-size content view) means the empty toolbar
/// draws no chrome of its own; only the lights are visible over our header.
#[cfg(target_os = "macos")]
fn install_unified_titlebar(ns_window_ptr: *mut std::ffi::c_void) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSTitlebarSeparatorStyle, NSToolbar, NSWindow, NSWindowToolbarStyle};

    if ns_window_ptr.is_null() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // Safety: Tauri hands us a valid NSWindow pointer; setup runs on the main
    // thread (proven by the marker above).
    let window: &NSWindow = unsafe { &*(ns_window_ptr as *const NSWindow) };

    let toolbar = NSToolbar::new(mtm);
    window.setToolbar(Some(&toolbar));
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    // styles.css draws the header's bottom hairline; keep AppKit's out.
    window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
}

pub fn run() {
    // Tauri owns its own in-process runtime.
    let runtime = SocaiRuntime::new();
    let telemetry = DesktopTelemetry::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .manage(runtime)
        .manage(AgentTaskRegistry::default())
        .manage(connectors::feishu::FeishuState::default())
        .manage(telemetry)
        .setup(|app| {
            // Let the webview load locally-downloaded note media (images/video)
            // from the run-artifact root via the asset protocol (convertFileSrc).
            // The static scope in tauri.conf.json can only name the default
            // ~/.socai/runs — build-time config can't know the user's setting —
            // so the runs dir stays fully relocatable: whatever root
            // SOCAI_RUNS_DIR / `runs.dir` resolves to is granted here at
            // startup. allow_directory tolerates a not-yet-created dir.
            let runs_root = socai_core::agent::run_logging::default_runs_root();
            if let Err(err) = app.asset_protocol_scope().allow_directory(&runs_root, true) {
                eprintln!(
                    "failed to allow runs dir {} in asset scope: {err:#}",
                    runs_root.display()
                );
            }

            let runtime = app.state::<SocaiRuntime>().inner().clone();
            let tasks = app.state::<AgentTaskRegistry>().inner().clone();
            let telemetry = app.state::<DesktopTelemetry>().inner().clone();
            let handle = app.handle().clone();
            let media_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                // A graceful app shutdown marks active tasks interrupted in
                // tasks.json before this process starts. If its trace drop
                // guard finished during shutdown, recover and upload it now.
                for snapshot in tasks.list().await {
                    if snapshot.status == "interrupted" {
                        if let Some(run_dir) = snapshot.run_dir.as_deref() {
                            commands::upload_terminal_run_trace(run_dir, "interrupted", &telemetry)
                                .await;
                        }
                    }
                }

                let mut rx = runtime.subscribe_browser_events();
                let mut latest_disconnect_reason: Option<String> = None;
                let mut recoverable_disconnect = false;
                let browser_event_generation = Arc::new(AtomicU64::new(0));
                while let Ok(event) = rx.recv().await {
                    match event {
                        RuntimeBrowserEvent::StatusChanged(payload) => {
                            match &payload {
                                socai_core::cdp::StatusPayload::Connected {
                                    managed,
                                    remote,
                                    source,
                                    remote_timeout_seconds,
                                    remote_remaining_seconds,
                                    ..
                                } => {
                                    browser_event_generation.fetch_add(1, Ordering::SeqCst);
                                    latest_disconnect_reason = None;
                                    recoverable_disconnect = false;
                                    let profile = if *remote {
                                        "remote"
                                    } else if *managed {
                                        "managed"
                                    } else {
                                        "existing"
                                    };
                                    telemetry.capture(
                                        "socai_browser_connect",
                                        json!({
                                            "outcome": "completed",
                                            "browser_profile": profile,
                                            "browser_source": source,
                                            "remote_timeout_seconds": remote_timeout_seconds,
                                            "remote_remaining_seconds": remote_remaining_seconds,
                                        }),
                                    );
                                }
                                socai_core::cdp::StatusPayload::Disconnected { reason } => {
                                    if reason != "not_yet_connected" {
                                        latest_disconnect_reason = Some(reason.clone());
                                        telemetry.capture(
                                            "socai_browser_connect",
                                            json!({
                                                "outcome": if reason == "user_disconnected" {
                                                    "disconnected"
                                                } else {
                                                    "failed"
                                                },
                                                "error": crate::telemetry::short_error(reason),
                                            }),
                                        );
                                    }
                                }
                                _ => {}
                            }
                            let _ = handle.emit("cdp:status_changed", payload);
                        }
                        RuntimeBrowserEvent::Interruption {
                            interruption_kind,
                            reason,
                            ..
                        } => {
                            recoverable_disconnect = matches!(
                                interruption_kind,
                                socai_core::cdp::BrowserInterruptionKind::TransportDisconnected
                                    | socai_core::cdp::BrowserInterruptionKind::RemoteLeaseExpired
                            );
                            latest_disconnect_reason = Some(reason.clone());
                            let _ = handle.emit(
                                "cdp:recovery_changed",
                                json!({
                                    "status": if recoverable_disconnect { "recovering" } else { "terminal" },
                                    "kind": interruption_kind,
                                    "reason": reason,
                                    "grace_seconds": if recoverable_disconnect {
                                        Some(BROWSER_RECOVERY_GRACE.as_secs())
                                    } else {
                                        None
                                    },
                                }),
                            );
                        }
                        RuntimeBrowserEvent::TargetsChanged(targets) => {
                            let generation =
                                browser_event_generation.fetch_add(1, Ordering::SeqCst) + 1;
                            for snapshot in tasks.rebind_missing_targets(&targets).await {
                                let task_id = snapshot.task_id.clone();
                                commands::emit_task_event(
                                    &handle,
                                    &tasks,
                                    &task_id,
                                    "tab_rebound",
                                    "chrome target rebound after reconnect".into(),
                                    Some(snapshot),
                                )
                                .await;
                            }
                            let active_targets: HashSet<String> =
                                targets.into_iter().map(|target| target.target_id).collect();
                            let interruption_reason = latest_disconnect_reason
                                .clone()
                                .unwrap_or_else(|| "chrome tab was closed".into());
                            if active_targets.is_empty() && recoverable_disconnect {
                                let generation_counter = browser_event_generation.clone();
                                let delayed_tasks = tasks.clone();
                                let delayed_telemetry = telemetry.clone();
                                let delayed_handle = handle.clone();
                                tauri::async_runtime::spawn(async move {
                                    tokio::time::sleep(BROWSER_RECOVERY_GRACE).await;
                                    if generation_counter.load(Ordering::SeqCst) != generation {
                                        return;
                                    }
                                    interrupt_missing_browser_targets(
                                        active_targets,
                                        interruption_reason,
                                        delayed_tasks,
                                        delayed_telemetry,
                                        delayed_handle,
                                    )
                                    .await;
                                });
                            } else {
                                interrupt_missing_browser_targets(
                                    active_targets,
                                    interruption_reason,
                                    tasks.clone(),
                                    telemetry.clone(),
                                    handle.clone(),
                                )
                                .await;
                            }
                        }
                    }
                }
            });
            tauri::async_runtime::spawn(async move {
                let mut media_events = socai_core::media::subscribe_background_media_events();
                loop {
                    match media_events.recv().await {
                        Ok(event) => {
                            let _ = media_handle.emit("agent_task:notes_updated", event);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            // macOS: native unified-toolbar titlebar — one-shot; AppKit owns
            // the traffic-light layout from here on (see install_unified_titlebar).
            #[cfg(target_os = "macos")]
            if let Some(win) = app.get_webview_window("main") {
                if let Ok(ptr) = win.ns_window() {
                    install_unified_titlebar(ptr);
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::cdp_connect,
            commands::cdp_disconnect,
            commands::cdp_status,
            commands::cdp_remote_debugging_ready,
            commands::cdp_refresh,
            commands::app_relaunch,
            commands::open_external,
            commands::open_chrome_remote_debugging,
            commands::agent_list_models,
            commands::agent_set_default_model,
            commands::agent_open_codex_login,
            commands::agent_save_api_key,
            commands::agent_task_start,
            commands::agent_task_reply,
            commands::agent_task_list,
            commands::agent_task_get,
            commands::agent_task_events,
            commands::agent_task_notes,
            commands::agent_task_artifacts,
            commands::agent_task_artifact_preview,
            commands::agent_task_artifact_download,
            commands::agent_task_artifact_download_exists,
            commands::agent_task_artifact_open,
            commands::agent_task_cancel,
            commands::agent_task_delete,
            commands::config_get,
            commands::config_set,
            commands::config_unset,
            commands::pro_activate,
            commands::auth_session,
            commands::auth_sms_send,
            commands::auth_sms_verify,
            commands::auth_logout,
            commands::billing_wallet,
            commands::billing_plan,
            commands::billing_create_wechat_order,
            commands::billing_create_alipay_order,
            commands::billing_order_status,
            commands::billing_mock_recharge,
            connectors::feishu::feishu_status,
            connectors::feishu::feishu_accounts,
            connectors::feishu::feishu_account_identity,
            connectors::feishu::feishu_report_failure,
            connectors::feishu::feishu_select_account,
            connectors::feishu::feishu_disconnect_account,
            connectors::feishu::feishu_connect,
            connectors::feishu::feishu_cancel_connect,
            connectors::feishu::feishu_export_task,
            connectors::feishu::feishu_list_chats,
            connectors::feishu::feishu_send_task_to_chat,
        ])
        .build(tauri::generate_context!())
        .expect("error while building socai")
        .run(|app_handle, event| {
            // On quit, close socai-owned chrome tabs the same way the explicit
            // disconnect button does. Tauri fires ExitRequested before the
            // process tears down; block on the async cleanup so the
            // Target.closeTarget calls actually reach chrome before we exit —
            // otherwise the tabs socai opened linger after the app is gone.
            // (Managed chrome is killed on drop regardless; this matters for the
            // attach-to-existing-browser case, where only socai's own tabs
            // should be closed, not the whole browser.) For a remote hosted
            // browser, disconnect() also awaits the session release (bounded)
            // — a fire-and-forget release would be aborted by the exit and the
            // session would bill until its server-side timeout.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let runtime = app_handle.state::<SocaiRuntime>().inner().clone();
                tauri::async_runtime::block_on(runtime.disconnect_browser());
            }
        });
}
