mod commands;
mod tasks;
mod telemetry;
mod timeline;

use std::collections::HashSet;

use serde_json::json;
use socai_core::runtime::{RuntimeBrowserEvent, SocaiRuntime};
use tasks::AgentTaskRegistry;
use tauri::{Emitter, Manager};
use telemetry::{duration_ms, DesktopTelemetry};

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
        .plugin(tauri_plugin_process::init())
        .manage(runtime)
        .manage(AgentTaskRegistry::default())
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
            tauri::async_runtime::spawn(async move {
                let mut rx = runtime.subscribe_browser_events();
                while let Ok(event) = rx.recv().await {
                    match event {
                        RuntimeBrowserEvent::StatusChanged(payload) => {
                            let _ = handle.emit("cdp:status_changed", payload);
                        }
                        RuntimeBrowserEvent::TargetsChanged(targets) => {
                            let active_targets: HashSet<String> =
                                targets.into_iter().map(|target| target.target_id).collect();
                            for (snapshot, abort_handle) in
                                tasks.interrupt_missing_targets(&active_targets).await
                            {
                                if let Some(handle) = abort_handle {
                                    handle.abort();
                                }
                                let task_id = snapshot.task_id.clone();
                                commands::record_interrupted_run(
                                    &snapshot,
                                    "chrome tab was closed",
                                );
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
                                        "duration_ms": duration_ms(
                                            snapshot.started_at,
                                            snapshot.finished_at,
                                        ),
                                    }),
                                );
                                commands::emit_task_event(
                                    &handle,
                                    &tasks,
                                    &task_id,
                                    "interrupted",
                                    "chrome tab was closed".into(),
                                    Some(snapshot),
                                )
                                .await;
                            }
                        }
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
            commands::cdp_refresh,
            commands::open_external,
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
            commands::agent_task_cancel,
            commands::agent_task_delete,
            commands::config_get,
            commands::config_set,
            commands::config_unset,
            commands::pro_activate,
        ])
        .run(tauri::generate_context!())
        .expect("error while running socai");
}
