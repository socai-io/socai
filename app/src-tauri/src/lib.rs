mod commands;
mod tasks;
mod telemetry;
mod timeline;

use std::collections::HashSet;

use serde_json::json;
use socai_core::runtime::{RuntimeBrowserEvent, SocaiRuntime};
use tasks::AgentTaskRegistry;
use telemetry::{duration_ms, DesktopTelemetry};
use tauri::{Emitter, Manager};

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
