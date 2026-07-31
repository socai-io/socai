use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use socai_core::agent::{AgentEvent, Conversation, Run};

use crate::tasks::{now_ms, AgentTaskSnapshot};

const MAX_EVENT_TEXT_CHARS: usize = 8_000;

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct AgentTaskEventPayload {
    pub(crate) task_id: String,
    #[serde(flatten)]
    pub(crate) payload: AgentTaskEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) snapshot: Option<AgentTaskSnapshot>,
    #[serde(default)]
    pub(crate) sequence: u64,
    #[serde(default)]
    pub(crate) created_at: u64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AgentTaskEventKind {
    Queued {
        text: String,
    },
    Running {
        text: String,
    },
    Started {
        run_id: String,
        task: String,
        model: String,
        text: String,
    },
    Tab {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
    },
    Step {
        step: u32,
        text: String,
    },
    Assistant {
        step: u32,
        text: String,
    },
    Reasoning {
        step: u32,
        text: String,
    },
    ToolCall {
        id: String,
        step: u32,
        sequence_in_step: u32,
        name: String,
        label: String,
        args: Value,
        repeat_count: u32,
        text: String,
    },
    ToolResult {
        id: String,
        step: u32,
        sequence_in_step: u32,
        name: String,
        label: String,
        args: Value,
        ok: bool,
        summary: String,
        duration_ms: u64,
        #[serde(default)]
        entities: Vec<TimelineEntity>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_file: Option<String>,
        text: String,
    },
    ToolError {
        id: String,
        step: u32,
        sequence_in_step: u32,
        name: String,
        label: String,
        args: Value,
        ok: bool,
        summary: String,
        duration_ms: u64,
        #[serde(default)]
        entities: Vec<TimelineEntity>,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_file: Option<String>,
        text: String,
    },
    ApiError {
        step: u32,
        message: String,
        text: String,
    },
    Done {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        steps: u32,
        text: String,
    },
    Completed {
        text: String,
    },
    Failed {
        text: String,
    },
    Cancelled {
        text: String,
    },
    Interrupted {
        text: String,
    },
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct TimelineEntity {
    #[serde(rename = "type")]
    pub(crate) entity_type: String,
    pub(crate) data: Value,
}

impl AgentTaskEventPayload {
    pub(crate) fn ephemeral(
        task_id: &str,
        payload: AgentTaskEventKind,
        snapshot: Option<AgentTaskSnapshot>,
    ) -> Self {
        Self {
            task_id: task_id.to_string(),
            payload,
            snapshot,
            sequence: 0,
            created_at: now_ms(),
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        self.payload.kind()
    }
}

impl AgentTaskEventKind {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Queued { .. } => "queued",
            Self::Running { .. } => "running",
            Self::Started { .. } => "started",
            Self::Tab { .. } => "tab",
            Self::Step { .. } => "step",
            Self::Assistant { .. } => "assistant",
            Self::Reasoning { .. } => "reasoning",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::ToolError { .. } => "tool_error",
            Self::ApiError { .. } => "api_error",
            Self::Done { .. } => "done",
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
            Self::Cancelled { .. } => "cancelled",
            Self::Interrupted { .. } => "interrupted",
        }
    }

    pub(crate) fn from_kind_text(kind: &str, text: String) -> Self {
        let text = truncate_event_text(&text);
        match kind {
            "queued" => Self::Queued { text },
            "running" => Self::Running { text },
            "tab" => Self::Tab {
                text,
                target_id: None,
            },
            "step" => Self::Step {
                step: first_number(&text).unwrap_or_default(),
                text,
            },
            "assistant" => Self::Assistant { step: 0, text },
            "reasoning" => Self::Reasoning { step: 0, text },
            "tool_call" => Self::ToolCall {
                id: String::new(),
                step: 0,
                sequence_in_step: 0,
                name: "tool".into(),
                label: "tool".into(),
                args: Value::Null,
                repeat_count: 0,
                text,
            },
            "tool_result" => Self::ToolResult {
                id: String::new(),
                step: 0,
                sequence_in_step: 0,
                name: "tool".into(),
                label: "tool".into(),
                args: Value::Null,
                ok: true,
                summary: text.clone(),
                duration_ms: 0,
                entities: Vec::new(),
                error: None,
                result_file: None,
                text,
            },
            "tool_error" => Self::ToolError {
                id: String::new(),
                step: 0,
                sequence_in_step: 0,
                name: "tool".into(),
                label: "tool".into(),
                args: Value::Null,
                ok: false,
                summary: String::new(),
                duration_ms: 0,
                entities: Vec::new(),
                error: text.clone(),
                result_file: None,
                text,
            },
            "api_error" => Self::ApiError {
                step: 0,
                message: text.clone(),
                text,
            },
            "done" => Self::Done {
                run_id: None,
                steps: first_number(&text).unwrap_or_default(),
                text,
            },
            "completed" => Self::Completed { text },
            "failed" => Self::Failed { text },
            "cancelled" => Self::Cancelled { text },
            "interrupted" => Self::Interrupted { text },
            "started" => Self::Started {
                run_id: String::new(),
                task: String::new(),
                model: String::new(),
                text,
            },
            _ => Self::Assistant { step: 0, text },
        }
    }
}

pub(crate) fn live_timeline_event(
    snapshot: &AgentTaskSnapshot,
    payload: AgentTaskEventKind,
    snapshot_for_emit: Option<AgentTaskSnapshot>,
    sequence: u64,
) -> AgentTaskEventPayload {
    AgentTaskEventPayload {
        task_id: snapshot.task_id.clone(),
        payload,
        snapshot: snapshot_for_emit,
        sequence,
        created_at: now_ms(),
    }
}

pub(crate) fn load_task_events(snapshot: &AgentTaskSnapshot) -> Vec<AgentTaskEventPayload> {
    // Replays the run.json/llm/tools layout only. Runs recorded before that
    // layout (#160, pre-2026-07) replay as just their terminal state —
    // legacy timeline.jsonl support was deliberately dropped.
    //
    // A task's conversation can span multiple runs (replies continue it), so
    // this concatenates every run's replay in order — each run's own
    // "started" event (task/model) is the boundary the frontend renders as a
    // new message in the thread.
    let mut events = Vec::new();
    let runs = conversation_run_dirs(snapshot);
    let last_index = runs.len().saturating_sub(1);
    for (index, (run_dir, record)) in runs.iter().enumerate() {
        let replayed = replay_run_events(snapshot, run_dir);
        if replayed.is_empty() {
            // A run cancelled before it wrote run.json still has a recorded
            // user message; synthesize its boundary so it doesn't vanish.
            if let Some(record) = record {
                events.push(replay_event(
                    snapshot,
                    AgentTaskEventKind::Started {
                        run_id: String::new(),
                        task: record.user.clone(),
                        model: snapshot
                            .model
                            .clone()
                            .unwrap_or_else(|| "unknown model".into()),
                        text: started_event_text_fields(&record.user, None, "unknown model"),
                    },
                ));
            }
        } else {
            events.extend(replayed);
        }
        // Non-final runs get their terminal marker from the conversation
        // record (the snapshot's status only describes the latest run).
        if index != last_index {
            if let Some(record) = record {
                if let Some(marker) = run_status_marker(&record.status) {
                    events.push(replay_event(snapshot, marker));
                }
            }
        }
    }
    if !events.is_empty() {
        ensure_started_event(snapshot, &mut events);
        reindex_replay_events(snapshot, &mut events);
    }
    append_terminal_snapshot_events(snapshot, &mut events);
    events.sort_by(compare_events);
    events
}

/// Every run dir belonging to this task's conversation, oldest first (with
/// its conversation record when it has one): the conversation's recorded
/// runs, plus the current run dir when it isn't in that list yet (still
/// in-flight, or finished just before this read — see the ordering note on
/// `record_desktop_session`'s callers). Shared with the notes aggregator.
pub(crate) fn conversation_run_dirs(snapshot: &AgentTaskSnapshot) -> Vec<(PathBuf, Option<Run>)> {
    let mut dirs: Vec<(PathBuf, Option<Run>)> = Vec::new();
    if let Some(session_dir) = snapshot.session_dir.as_deref() {
        if let Ok(conversation) = Conversation::load(session_dir) {
            dirs.extend(
                conversation
                    .runs
                    .iter()
                    .map(|run| (PathBuf::from(&run.run_dir), Some(run.clone()))),
            );
        }
    }
    if let Some(run_dir) = snapshot
        .run_dir
        .as_deref()
        .filter(|dir| !dir.trim().is_empty())
    {
        let run_dir = PathBuf::from(run_dir);
        if !dirs.iter().any(|(dir, _)| dir == &run_dir) {
            dirs.push((run_dir, None));
        }
    }
    dirs
}

/// Terminal timeline marker for a non-final run, from its conversation
/// record status. Completed runs already replay a `done` event.
fn run_status_marker(status: &str) -> Option<AgentTaskEventKind> {
    match status {
        "failed" => Some(AgentTaskEventKind::Failed {
            text: "task failed".into(),
        }),
        "cancelled" => Some(AgentTaskEventKind::Cancelled {
            text: "task cancelled".into(),
        }),
        "interrupted" => Some(AgentTaskEventKind::Interrupted {
            text: "task interrupted".into(),
        }),
        _ => None,
    }
}

pub(crate) fn agent_event_to_timeline(event: &AgentEvent) -> AgentTaskEventKind {
    match event {
        AgentEvent::Started {
            run_id,
            task,
            model,
        } => AgentTaskEventKind::Started {
            run_id: run_id.clone(),
            task: task.clone(),
            model: model.clone(),
            text: started_event_text_fields(task, Some(run_id), model),
        },
        AgentEvent::Step { step } => AgentTaskEventKind::Step {
            step: *step,
            text: String::new(),
        },
        AgentEvent::AssistantText { step, text } => AgentTaskEventKind::Assistant {
            step: *step,
            text: truncate_event_text(text),
        },
        AgentEvent::Reasoning { step, text } => AgentTaskEventKind::Reasoning {
            step: *step,
            text: truncate_event_text(text),
        },
        AgentEvent::ToolCall {
            id,
            step,
            sequence,
            name,
            input,
            repeat_count,
        } => tool_call_event(id, *step, *sequence, name, input, *repeat_count),
        AgentEvent::ToolResult {
            id,
            step,
            sequence,
            name,
            input,
            content,
            summary,
            duration_ms,
            error,
        } => tool_result_event(
            id,
            *step,
            *sequence,
            name,
            input,
            summary,
            *duration_ms,
            error.as_deref(),
            content,
            None,
        ),
        AgentEvent::ApiError { step, message } => AgentTaskEventKind::ApiError {
            step: *step,
            message: message.clone(),
            text: message.clone(),
        },
        AgentEvent::Done { run_id, steps, .. } => AgentTaskEventKind::Done {
            run_id: Some(run_id.clone()),
            steps: *steps,
            text: "done".to_string(),
        },
    }
}

/// Human-readable label for a current tool name.
pub(crate) fn user_label(name: &str) -> String {
    match name {
        "search" => "searched xiaohongshu",
        "get_notes" => "read xiaohongshu notes",
        "extract_search_cards" => "read search cards",
        "list_search_tabs" => "listed search tabs",
        "click_search_tab" => "switched search tab",
        "reset_search_filters" => "reset search filters",
        "apply_search_filters" => "applied search filters",
        "open_note" => "opened note",
        "close_note" => "closed note",
        "read_note" => "reading note",
        "extract_note" => "read current note",
        "extract_comments" => "read comments",
        "scroll_in_note" => "scrolled note",
        "collect_carousel_images" => "collected carousel images",
        "extract_profile" => "read author profile",
        "wait_for_rate_limit" => "waiting for xiaohongshu's rate limit",
        "page_state" => "checked page state",
        _ => return name.replace('_', " "),
    }
    .into()
}

fn replay_run_events(snapshot: &AgentTaskSnapshot, run_dir: &Path) -> Vec<AgentTaskEventPayload> {
    let Some(run) = read_json(&run_dir.join("run.json")) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    let task = run
        .get("task")
        .and_then(Value::as_str)
        .unwrap_or(&snapshot.task);
    let model = run
        .get("model")
        .and_then(Value::as_str)
        .or(snapshot.model.as_deref())
        .unwrap_or("unknown model");
    let run_id = run
        .get("id")
        .and_then(Value::as_str)
        .or(snapshot.run_id.as_deref())
        .unwrap_or("");
    events.push(replay_event(
        snapshot,
        AgentTaskEventKind::Started {
            run_id: run_id.to_string(),
            task: task.to_string(),
            model: model.to_string(),
            text: started_event_text_fields(task, Some(run_id), model),
        },
    ));

    let mut last_step = None;
    for (step, response) in llm_responses(run_dir) {
        push_step_event(snapshot, &mut events, &mut last_step, step);
        if let Some(error) = response.get("error").and_then(Value::as_str) {
            events.push(replay_event(
                snapshot,
                AgentTaskEventKind::ApiError {
                    step,
                    message: error.to_string(),
                    text: error.to_string(),
                },
            ));
            continue;
        }
        if let Some(reasoning) = response
            .get("reasoning_content")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
        {
            events.push(replay_event(
                snapshot,
                AgentTaskEventKind::Reasoning {
                    step,
                    text: truncate_event_text(reasoning),
                },
            ));
        }
        if let Some(texts) = response.get("text_blocks").and_then(Value::as_array) {
            for text in texts.iter().filter_map(Value::as_str) {
                if !text.trim().is_empty() {
                    events.push(replay_event(
                        snapshot,
                        AgentTaskEventKind::Assistant {
                            step,
                            text: truncate_event_text(text),
                        },
                    ));
                }
            }
        }
        let calls = response
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (index, call) in calls.iter().enumerate() {
            let sequence = index as u32 + 1;
            let name = call.get("name").and_then(Value::as_str).unwrap_or("tool");
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("tool-call");
            // Pre-rename runs (< #190) recorded tool dirs as `turn-…`.
            let tool_rel = ["step", "turn"]
                .iter()
                .map(|prefix| {
                    format!(
                        "tools/{prefix}-{step:03}-call-{sequence:02}-{}",
                        safe_component(name, "tool")
                    )
                })
                .find(|rel| run_dir.join(rel).is_dir())
                .unwrap_or_else(|| {
                    format!(
                        "tools/step-{step:03}-call-{sequence:02}-{}",
                        safe_component(name, "tool")
                    )
                });
            let tool_dir = run_dir.join(&tool_rel);
            let manifest = read_json(&tool_dir.join("tool.json")).unwrap_or(Value::Null);
            let input = manifest
                .get("input")
                .or_else(|| call.get("input"))
                .unwrap_or(&Value::Null);
            events.push(replay_event(
                snapshot,
                tool_call_event(id, step, sequence, name, input, 0),
            ));
            let output = read_json(&tool_dir.join("output.json")).unwrap_or(Value::Null);
            let error = manifest.get("error").and_then(Value::as_str);
            let duration = manifest
                .get("duration_ms")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let summary = tool_output_summary(&output);
            events.push(replay_event(
                snapshot,
                tool_result_event(
                    id,
                    step,
                    sequence,
                    name,
                    input,
                    &summary,
                    duration,
                    error,
                    &output,
                    Some(format!("{tool_rel}/output.json")),
                ),
            ));
        }
    }
    // Pre-rename runs (< #190) recorded the step count as "turns".
    let steps = run
        .get("steps")
        .or_else(|| run.get("turns"))
        .and_then(Value::as_u64)
        .unwrap_or_default() as u32;
    if run.get("status").and_then(Value::as_str) == Some("completed") {
        events.push(replay_event(
            snapshot,
            AgentTaskEventKind::Done {
                run_id: Some(run_id.to_string()),
                steps,
                text: "done".to_string(),
            },
        ));
    }
    events
}

fn append_terminal_snapshot_events(
    snapshot: &AgentTaskSnapshot,
    events: &mut Vec<AgentTaskEventPayload>,
) {
    match snapshot.status.as_str() {
        "completed" => {
            if !events.iter().any(|event| event.kind() == "done") {
                push_snapshot_event(
                    snapshot,
                    events,
                    AgentTaskEventKind::Done {
                        run_id: snapshot.run_id.clone(),
                        steps: snapshot.steps.unwrap_or_default(),
                        text: "done".into(),
                    },
                );
            }
            if !events.iter().any(|event| event.kind() == "completed") {
                push_snapshot_event(
                    snapshot,
                    events,
                    AgentTaskEventKind::Completed {
                        text: "task completed".into(),
                    },
                );
            }
        }
        "failed" => {
            if !events.iter().any(|event| event.kind() == "failed") {
                let text = snapshot
                    .error
                    .as_deref()
                    .filter(|error| !error.trim().is_empty())
                    .unwrap_or("task failed")
                    .to_string();
                push_snapshot_event(snapshot, events, AgentTaskEventKind::Failed { text });
            }
        }
        "cancelled" => {
            if !events.iter().any(|event| event.kind() == "cancelled") {
                push_snapshot_event(
                    snapshot,
                    events,
                    AgentTaskEventKind::Cancelled {
                        text: "task cancelled".into(),
                    },
                );
            }
        }
        "interrupted" => {
            if !events.iter().any(|event| event.kind() == "interrupted") {
                let text = snapshot
                    .error
                    .as_deref()
                    .filter(|error| !error.trim().is_empty())
                    .unwrap_or("task interrupted")
                    .to_string();
                push_snapshot_event(snapshot, events, AgentTaskEventKind::Interrupted { text });
            }
        }
        _ => {}
    }
}

fn push_snapshot_event(
    snapshot: &AgentTaskSnapshot,
    events: &mut Vec<AgentTaskEventPayload>,
    payload: AgentTaskEventKind,
) {
    let sequence = events.iter().map(|event| event.sequence).max().unwrap_or(0) + 1;
    events.push(AgentTaskEventPayload {
        task_id: snapshot.task_id.clone(),
        payload,
        snapshot: None,
        sequence,
        created_at: snapshot.finished_at.unwrap_or_else(now_ms),
    });
}

fn ensure_started_event(snapshot: &AgentTaskSnapshot, events: &mut Vec<AgentTaskEventPayload>) {
    if events.iter().any(|event| event.kind() == "started") {
        return;
    }
    events.insert(
        0,
        replay_event(
            snapshot,
            AgentTaskEventKind::Started {
                run_id: snapshot.run_id.clone().unwrap_or_default(),
                task: snapshot.task.clone(),
                model: snapshot
                    .model
                    .clone()
                    .unwrap_or_else(|| "unknown model".into()),
                text: started_event_text(snapshot, None, None),
            },
        ),
    );
}

fn reindex_replay_events(snapshot: &AgentTaskSnapshot, events: &mut [AgentTaskEventPayload]) {
    let base = snapshot.started_at.unwrap_or(snapshot.created_at);
    for (idx, event) in events.iter_mut().enumerate() {
        event.sequence = idx as u64 + 1;
        event.created_at = base.saturating_add(idx as u64 + 1);
    }
}

fn replay_event(
    snapshot: &AgentTaskSnapshot,
    payload: AgentTaskEventKind,
) -> AgentTaskEventPayload {
    AgentTaskEventPayload {
        task_id: snapshot.task_id.clone(),
        payload,
        snapshot: None,
        sequence: 0,
        created_at: 0,
    }
}

fn push_step_event(
    snapshot: &AgentTaskSnapshot,
    events: &mut Vec<AgentTaskEventPayload>,
    last_step: &mut Option<u32>,
    step: u32,
) {
    if step == 0 || *last_step == Some(step) {
        return;
    }
    *last_step = Some(step);
    events.push(replay_event(
        snapshot,
        AgentTaskEventKind::Step {
            step,
            text: String::new(),
        },
    ));
}

fn started_event_text(
    snapshot: &AgentTaskSnapshot,
    task: Option<&str>,
    model: Option<&str>,
) -> String {
    let task = task.unwrap_or(&snapshot.task);
    let model = model
        .or(snapshot.model.as_deref())
        .unwrap_or("unknown model");
    started_event_text_fields(task, snapshot.run_id.as_deref(), model)
}

fn started_event_text_fields(task: &str, run_id: Option<&str>, model: &str) -> String {
    let run = run_id
        .filter(|run_id| !run_id.trim().is_empty())
        .map(|run_id| format!("run {run_id} · "))
        .unwrap_or_default();
    format!("task: {task}\n{run}model {model}")
}

fn tool_call_event(
    id: &str,
    step: u32,
    sequence: u32,
    name: &str,
    input: &Value,
    repeat_count: u32,
) -> AgentTaskEventKind {
    AgentTaskEventKind::ToolCall {
        id: id.to_string(),
        step,
        sequence_in_step: sequence,
        name: name.to_string(),
        label: user_label(name),
        args: input.clone(),
        repeat_count,
        text: format_tool_call_text(name, input, repeat_count.into()),
    }
}

fn tool_result_event(
    id: &str,
    step: u32,
    sequence: u32,
    name: &str,
    input: &Value,
    summary: &str,
    duration_ms: u64,
    error: Option<&str>,
    content: &Value,
    result_file: Option<String>,
) -> AgentTaskEventKind {
    let raw = raw_tool_result_value(content);
    let ok = error.map(|err| err.trim().is_empty()).unwrap_or(true)
        && raw
            .as_ref()
            .and_then(|value| value.get("ok"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
    let entities = raw
        .as_ref()
        .map(|value| normalize_entities(name, value))
        .unwrap_or_default();
    let error = error.map(str::trim).filter(|err| !err.is_empty());
    // A tool that ran but reported failure (`ok: false`, e.g. a search that
    // never reached a result page) carries no dispatcher error — surface its
    // `reason` as the row's error so the timeline doesn't render it like a
    // quiet success.
    let error = error.map(str::to_string).or_else(|| {
        (!ok).then(|| {
            raw.as_ref()
                .and_then(|value| value.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("tool reported failure")
                .to_string()
        })
    });
    let (_kind, text) = format_tool_result_text(name, content, duration_ms, error.as_deref().unwrap_or(""));
    if let Some(error) = error {
        AgentTaskEventKind::ToolError {
            id: id.to_string(),
            step,
            sequence_in_step: sequence,
            name: name.to_string(),
            label: user_label(name),
            args: input.clone(),
            ok: false,
            summary: truncate_event_text(summary),
            duration_ms,
            entities,
            error: error.to_string(),
            result_file,
            text,
        }
    } else {
        AgentTaskEventKind::ToolResult {
            id: id.to_string(),
            step,
            sequence_in_step: sequence,
            name: name.to_string(),
            label: user_label(name),
            args: input.clone(),
            ok,
            summary: truncate_event_text(summary),
            duration_ms,
            entities,
            error: None,
            result_file,
            text,
        }
    }
}

fn format_tool_call_text(tool: &str, input: &Value, repeat_count: u64) -> String {
    let preview = serde_json::to_string(input).unwrap_or_else(|_| input.to_string());
    if repeat_count > 1 {
        format!("{tool}({preview}) repeat={repeat_count}")
    } else {
        format!("{tool}({preview})")
    }
}

fn format_tool_result_text(
    tool: &str,
    content: &Value,
    duration_ms: u64,
    error: &str,
) -> (&'static str, String) {
    let duration = format_tool_duration(duration_ms);
    if !error.trim().is_empty() {
        return ("tool_error", format!("{tool} ({duration}): {error}"));
    }
    let tokens = format_token_estimate(estimate_result_tokens(content));
    ("tool_result", format!("{tool} ({duration} · {tokens})"))
}

fn format_tool_duration(duration_ms: u64) -> String {
    let tenths = (duration_ms + 50) / 100;
    if tenths < 10 {
        return format!("0.{tenths}s");
    }
    let secs = (duration_ms + 500) / 1000;
    if secs < 60 {
        return format!("{secs}s");
    }
    format!("{}m {:02}s", secs / 60, secs % 60)
}

// No provider reports per-tool-result tokens, so estimate: ~4 ASCII chars per
// token, ~1 token per non-ASCII (CJK) char. Image blocks carry no text here.
fn estimate_result_tokens(content: &Value) -> u64 {
    let mut ascii = 0u64;
    let mut wide = 0u64;
    let mut count = |text: &str| {
        for ch in text.chars() {
            if ch.is_ascii() {
                ascii += 1;
            } else {
                wide += 1;
            }
        }
    };
    match content {
        Value::Array(items) => {
            for item in items {
                if item.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    count(text);
                }
            }
        }
        Value::String(text) => count(text),
        Value::Null => {}
        other => count(&other.to_string()),
    }
    ascii / 4 + wide
}

fn format_token_estimate(tokens: u64) -> String {
    // 9_950+ rounds to 10k under {:.1}, so switch to the integer-k format there.
    if tokens >= 9_950 {
        format!("~{}k tokens", (tokens + 500) / 1000)
    } else if tokens >= 1_000 {
        format!("~{:.1}k tokens", tokens as f64 / 1000.0)
    } else if tokens == 1 {
        "~1 token".into()
    } else {
        format!("~{tokens} tokens")
    }
}

fn raw_tool_result_value(content: &Value) -> Option<Value> {
    match content {
        Value::Array(items) => items.iter().find_map(|item| {
            if item.get("type").and_then(Value::as_str) != Some("text") {
                return None;
            }
            let text = item.get("text").and_then(Value::as_str)?;
            serde_json::from_str::<Value>(text)
                .ok()
                .or_else(|| Some(Value::String(text.to_string())))
        }),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .or_else(|| Some(Value::String(text.clone()))),
        Value::Null => None,
        other => Some(other.clone()),
    }
}

fn normalize_entities(tool: &str, value: &Value) -> Vec<TimelineEntity> {
    // `--preview` yields a `cards` array (-> card grid); the default full scan
    // yields the aggregated bundle (-> xhs_search).
    match tool {
        "search" => {
            if let Some(cards) = value.get("cards").and_then(Value::as_array) {
                if cards.is_empty() {
                    Vec::new()
                } else {
                    vec![entity("xhs_note_card_grid", Value::Array(cards.clone()))]
                }
            } else if value.is_object() {
                vec![entity("xhs_search", value.clone())]
            } else {
                Vec::new()
            }
        }
        "extract_search_cards" => value
            .as_array()
            .filter(|cards| !cards.is_empty())
            .map(|cards| vec![entity("xhs_note_card_grid", Value::Array(cards.clone()))])
            .unwrap_or_default(),
        "read_note" => value
            .get("entity")
            .filter(|entity| entity.is_object())
            .map(|entity_value| vec![entity("xhs_note", entity_value.clone())])
            .unwrap_or_default(),
        "get_notes" => value
            .get("notes")
            .and_then(Value::as_array)
            .map(|notes| {
                notes
                    .iter()
                    .filter_map(|note| note.get("entity"))
                    .filter(|entity| entity.is_object())
                    .cloned()
                    .map(|entity_value| entity("xhs_note", entity_value))
                    .collect()
            })
            .unwrap_or_default(),
        "extract_note" => {
            if value.is_object() {
                vec![entity("xhs_note", value.clone())]
            } else {
                Vec::new()
            }
        }
        "extract_comments" => value
            .as_array()
            .filter(|comments| !comments.is_empty())
            .map(|comments| vec![entity("xhs_comments", Value::Array(comments.clone()))])
            .unwrap_or_default(),
        "collect_carousel_images" => value
            .as_array()
            .filter(|images| !images.is_empty())
            .map(|images| vec![entity("xhs_image_strip", Value::Array(images.clone()))])
            .unwrap_or_default(),
        "extract_profile" => {
            if value.is_object() {
                vec![entity("xhs_author_profile", value.clone())]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn entity(entity_type: &str, data: Value) -> TimelineEntity {
    TimelineEntity {
        entity_type: entity_type.to_string(),
        data,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn llm_responses(run_dir: &Path) -> Vec<(u32, Value)> {
    let Ok(entries) = std::fs::read_dir(run_dir.join("llm")) else {
        return Vec::new();
    };
    let mut responses: Vec<(u32, Value)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let step = name.strip_suffix(".response.json")?.parse().ok()?;
            Some((step, read_json(&entry.path())?))
        })
        .collect();
    responses.sort_by_key(|(step, _)| *step);
    responses
}

fn tool_output_summary(output: &Value) -> String {
    let raw = raw_tool_result_value(output).unwrap_or(Value::Null);
    let text = match raw {
        Value::String(text) => text,
        Value::Null => String::new(),
        value => serde_json::to_string(&value).unwrap_or_default(),
    };
    truncate_event_text(&text.chars().take(240).collect::<String>())
}

fn safe_component(value: &str, fallback: &str) -> String {
    let cleaned: String = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

fn truncate_event_text(text: &str) -> String {
    if text.chars().count() <= MAX_EVENT_TEXT_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(MAX_EVENT_TEXT_CHARS).collect();
    format!("{kept}\n... [truncated]")
}

fn first_number(text: &str) -> Option<u32> {
    let digits: String = text
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn compare_events(a: &AgentTaskEventPayload, b: &AgentTaskEventPayload) -> std::cmp::Ordering {
    if a.sequence > 0 && b.sequence > 0 && a.sequence != b.sequence {
        return a.sequence.cmp(&b.sequence);
    }
    if a.created_at != b.created_at {
        return a.created_at.cmp(&b.created_at);
    }
    std::cmp::Ordering::Equal
}
