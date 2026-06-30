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

/// macOS traffic-light inset (x, y). With this math the close-button center
/// lands at `y - 2` px from the window top; the `.is-macos .topbar` header is
/// 52px tall (centerline 26px), so `y = 28` puts the lights on the same row as
/// the brand and status capsule. Keep `y` in sync with that header height.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_POS: (f64, f64) = (19.0, 28.0);

/// Reposition the native macOS traffic lights onto our header's centerline.
///
/// tao applies `trafficLightPosition` inside its content-view `drawRect`, but
/// wry swaps in its own webview parent view, so that path never fires for us.
/// We replicate tao's inset math here and call it on window show/resize/focus.
#[cfg(target_os = "macos")]
fn reposition_traffic_lights(ns_window_ptr: *mut std::ffi::c_void, x: f64, y: f64) {
    use objc2_app_kit::{NSWindow, NSWindowButton};

    if ns_window_ptr.is_null() {
        return;
    }
    // Safety: Tauri hands us a valid NSWindow pointer; we only message it on the
    // main thread (setup + window-event callbacks both run there).
    let window: &NSWindow = unsafe { &*(ns_window_ptr as *const NSWindow) };

    let (Some(close), Some(mini), Some(zoom)) = (
        window.standardWindowButton(NSWindowButton::CloseButton),
        window.standardWindowButton(NSWindowButton::MiniaturizeButton),
        window.standardWindowButton(NSWindowButton::ZoomButton),
    ) else {
        return;
    };

    let win_h = window.frame().size.height;
    let close_rect = close.frame();

    // Resize the private title-bar container so clicks register at the new
    // position, pinned to the window top (tao's approach).
    if let Some(container) = unsafe { close.superview().and_then(|sv| sv.superview()) } {
        let mut r = container.frame();
        r.size.height = close_rect.size.height + y;
        r.origin.y = win_h - r.size.height;
        container.setFrame(r);
    }

    // Evenly space the three buttons from x, keeping their vertical offset.
    let space = mini.frame().origin.x - close_rect.origin.x;
    for (i, button) in [&close, &mini, &zoom].into_iter().enumerate() {
        let mut origin = button.frame().origin;
        origin.x = x + (i as f64) * space;
        button.setFrameOrigin(origin);
    }
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
                                        "turns": snapshot.turns,
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

            // macOS: drop the native traffic lights onto our header centerline,
            // and re-apply on show/resize/focus (macOS resets them otherwise).
            #[cfg(target_os = "macos")]
            if let Some(win) = app.get_webview_window("main") {
                let (x, y) = TRAFFIC_LIGHT_POS;
                if let Ok(ptr) = win.ns_window() {
                    reposition_traffic_lights(ptr, x, y);
                }
                // Cold-launch backstop: on first launch (Gatekeeper scan, busy
                // system) the setup-time placement can run before AppKit finishes
                // laying out the title bar, and no focus/resize event is guaranteed
                // to follow to correct it. Re-apply on the main thread shortly after
                // the window settles. Idempotent — a no-op when already correct, so
                // it never flickers. Runs on the shared async runtime (no dedicated
                // OS threads); the two waits land the retries ~250ms and ~750ms in.
                let win_retry = win.clone();
                tauri::async_runtime::spawn(async move {
                    for gap_ms in [250u64, 500] {
                        tokio::time::sleep(std::time::Duration::from_millis(gap_ms)).await;
                        let win_main = win_retry.clone();
                        let _ = win_retry.run_on_main_thread(move || {
                            if let Ok(ptr) = win_main.ns_window() {
                                reposition_traffic_lights(ptr, x, y);
                            }
                        });
                    }
                });
                let win_for_events = win.clone();
                win.on_window_event(move |event| {
                    if matches!(
                        event,
                        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Focused(true)
                    ) {
                        if let Ok(ptr) = win_for_events.ns_window() {
                            reposition_traffic_lights(ptr, x, y);
                        }
                    }
                });
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
            commands::agent_task_list,
            commands::agent_task_get,
            commands::agent_task_events,
            commands::agent_task_cancel,
            commands::config_get,
            commands::config_set,
            commands::config_unset,
        ])
        .run(tauri::generate_context!())
        .expect("error while running socai");
}
