use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use socai_core::agent::AgentEvent;

use crate::tasks::{now_ms, AgentTaskSnapshot};

const TIMELINE_FILE: &str = "timeline.jsonl";
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
    Turn {
        turn: u32,
        text: String,
    },
    Assistant {
        turn: u32,
        text: String,
    },
    Reasoning {
        turn: u32,
        text: String,
    },
    ToolCall {
        id: String,
        turn: u32,
        sequence_in_turn: u32,
        name: String,
        label: String,
        args: Value,
        repeat_count: u32,
        text: String,
    },
    ToolResult {
        id: String,
        turn: u32,
        sequence_in_turn: u32,
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
        turn: u32,
        sequence_in_turn: u32,
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
        turn: u32,
        message: String,
        text: String,
    },
    Done {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        turns: u32,
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
            Self::Turn { .. } => "turn",
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
            "turn" => Self::Turn {
                turn: first_number(&text).unwrap_or_default(),
                text,
            },
            "assistant" => Self::Assistant { turn: 0, text },
            "reasoning" => Self::Reasoning { turn: 0, text },
            "tool_call" => Self::ToolCall {
                id: String::new(),
                turn: 0,
                sequence_in_turn: 0,
                name: "tool".into(),
                label: "tool".into(),
                args: Value::Null,
                repeat_count: 0,
                text,
            },
            "tool_result" => Self::ToolResult {
                id: String::new(),
                turn: 0,
                sequence_in_turn: 0,
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
                turn: 0,
                sequence_in_turn: 0,
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
                turn: 0,
                message: text.clone(),
                text,
            },
            "done" => Self::Done {
                run_id: None,
                turns: first_number(&text).unwrap_or_default(),
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
            _ => Self::Assistant { turn: 0, text },
        }
    }
}

pub(crate) fn append_timeline_event(
    snapshot: &AgentTaskSnapshot,
    payload: AgentTaskEventKind,
    snapshot_for_emit: Option<AgentTaskSnapshot>,
    sequence: u64,
) -> anyhow::Result<AgentTaskEventPayload> {
    let path = timeline_path(snapshot).context("task has no run_dir for timeline")?;
    let mut event = AgentTaskEventPayload {
        task_id: snapshot.task_id.clone(),
        payload,
        snapshot: None,
        sequence,
        created_at: now_ms(),
    };
    append_jsonl(&path, &event).with_context(|| format!("write timeline {}", path.display()))?;
    event.snapshot = snapshot_for_emit;
    Ok(event)
}

pub(crate) fn load_task_events(snapshot: &AgentTaskSnapshot) -> Vec<AgentTaskEventPayload> {
    let mut events = read_timeline_events(snapshot);
    if events.is_empty() {
        events = replay_task_events(snapshot);
        if !events.is_empty() {
            ensure_started_event(snapshot, &mut events);
            reindex_replay_events(snapshot, &mut events);
        }
    }
    append_terminal_snapshot_events(snapshot, &mut events);
    events.sort_by(compare_events);
    events
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
        AgentEvent::Turn { turn } => AgentTaskEventKind::Turn {
            turn: *turn,
            text: format!("turn {turn}"),
        },
        AgentEvent::AssistantText { turn, text } => AgentTaskEventKind::Assistant {
            turn: *turn,
            text: truncate_event_text(text),
        },
        AgentEvent::Reasoning { turn, text } => AgentTaskEventKind::Reasoning {
            turn: *turn,
            text: truncate_event_text(text),
        },
        AgentEvent::ToolCall {
            id,
            turn,
            sequence,
            name,
            input,
            repeat_count,
        } => tool_call_event(id, *turn, *sequence, name, input, *repeat_count),
        AgentEvent::ToolResult {
            id,
            turn,
            sequence,
            name,
            input,
            content,
            summary,
            duration_ms,
            error,
        } => tool_result_event(
            id,
            *turn,
            *sequence,
            name,
            input,
            summary,
            *duration_ms,
            error.as_deref(),
            content,
            None,
        ),
        AgentEvent::ApiError { turn, message } => AgentTaskEventKind::ApiError {
            turn: *turn,
            message: message.clone(),
            text: format!("turn {turn}: {message}"),
        },
        AgentEvent::Done { run_id, turns, .. } => AgentTaskEventKind::Done {
            run_id: Some(run_id.clone()),
            turns: *turns,
            text: format!("done in {turns} turns"),
        },
    }
}

/// Human-readable label for a tool name. Computed when an event is written and
/// baked into timeline.jsonl; the read path never recomputes it. So only current
/// tool names ever reach here — renamed tools need no legacy aliases, and old
/// runs keep the labels they were written with.
pub(crate) fn user_label(name: &str) -> String {
    match name {
        "search" => "searched xiaohongshu",
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
        "page_state" => "checked page state",
        _ => return name.replace('_', " "),
    }
    .into()
}

fn timeline_path(snapshot: &AgentTaskSnapshot) -> Option<PathBuf> {
    let run_dir = snapshot
        .run_dir
        .as_ref()
        .map(String::as_str)
        .map(str::trim)
        .filter(|dir| !dir.is_empty())?;
    Some(PathBuf::from(run_dir).join(TIMELINE_FILE))
}

fn append_jsonl(path: &Path, event: &AgentTaskEventPayload) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(event).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())
}

fn read_timeline_events(snapshot: &AgentTaskSnapshot) -> Vec<AgentTaskEventPayload> {
    let Some(path) = timeline_path(snapshot) else {
        return Vec::new();
    };
    let mut events: Vec<AgentTaskEventPayload> = read_jsonl_values(&path)
        .into_iter()
        .filter_map(|value| serde_json::from_value::<AgentTaskEventPayload>(value).ok())
        .map(|mut event| {
            if event.task_id.is_empty() {
                event.task_id = snapshot.task_id.clone();
            }
            event.snapshot = None;
            event
        })
        .collect();
    events.sort_by(compare_events);
    events
}

pub(crate) fn next_timeline_sequence(snapshot: &AgentTaskSnapshot) -> u64 {
    timeline_path(snapshot)
        .map(|path| next_sequence(&path))
        .unwrap_or(1)
}

fn next_sequence(path: &Path) -> u64 {
    read_jsonl_values(path)
        .into_iter()
        .filter_map(|value| value.get("sequence").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
        + 1
}

fn replay_task_events(snapshot: &AgentTaskSnapshot) -> Vec<AgentTaskEventPayload> {
    let Some(run_dir) = snapshot
        .run_dir
        .as_ref()
        .filter(|dir| !dir.trim().is_empty())
    else {
        return Vec::new();
    };
    let run_dir = PathBuf::from(run_dir);
    let mut events = replay_reasoning_log(snapshot, &run_dir, &run_dir.join("reasoning_log.jsonl"));
    if events.is_empty() {
        events = replay_run_state_events(snapshot, &run_dir.join("run_state/events.jsonl"));
    }
    events
}

fn replay_reasoning_log(
    snapshot: &AgentTaskSnapshot,
    run_dir: &Path,
    path: &Path,
) -> Vec<AgentTaskEventPayload> {
    let mut events = Vec::new();
    let mut last_turn = None;
    for value in read_jsonl_values(path) {
        let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        match event_type {
            "task_start" => {
                let task = value.get("task").and_then(Value::as_str);
                let model = value.get("model").and_then(Value::as_str);
                events.push(replay_event(
                    snapshot,
                    AgentTaskEventKind::Started {
                        run_id: snapshot.run_id.clone().unwrap_or_default(),
                        task: task.unwrap_or(&snapshot.task).to_string(),
                        model: model
                            .or(snapshot.model.as_deref())
                            .unwrap_or("unknown model")
                            .to_string(),
                        text: started_event_text(snapshot, task, model),
                    },
                ));
            }
            "llm_response" => {
                let turn = number_field(&value, "turn") as u32;
                push_turn_event(snapshot, &mut events, &mut last_turn, turn);
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if !text.is_empty() {
                    events.push(replay_event(
                        snapshot,
                        AgentTaskEventKind::Assistant {
                            turn,
                            text: truncate_event_text(text),
                        },
                    ));
                }
            }
            "reasoning" => {
                let turn = number_field(&value, "turn") as u32;
                push_turn_event(snapshot, &mut events, &mut last_turn, turn);
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if !text.is_empty() {
                    events.push(replay_event(
                        snapshot,
                        AgentTaskEventKind::Reasoning {
                            turn,
                            text: truncate_event_text(text),
                        },
                    ));
                }
            }
            "tool_call_start" => {
                let turn = number_field(&value, "turn") as u32;
                push_turn_event(snapshot, &mut events, &mut last_turn, turn);
                let sequence = number_field(&value, "sequence") as u32;
                let tool = value.get("tool").and_then(Value::as_str).unwrap_or("tool");
                let input = value.get("input").unwrap_or(&Value::Null);
                let repeat_count = number_field(&value, "repeat_count") as u32;
                let id = tool_use_id(&value, turn, sequence, tool);
                events.push(replay_event(
                    snapshot,
                    tool_call_event(&id, turn, sequence, tool, input, repeat_count),
                ));
            }
            "tool_result" => {
                let turn = number_field(&value, "turn") as u32;
                push_turn_event(snapshot, &mut events, &mut last_turn, turn);
                let sequence = number_field(&value, "sequence") as u32;
                let tool = value.get("tool").and_then(Value::as_str).unwrap_or("tool");
                let input = value.get("input").unwrap_or(&Value::Null);
                let summary = value
                    .get("result_summary")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let error = value.get("error").and_then(Value::as_str).unwrap_or("");
                let duration_ms = duration_ms(&value);
                let result_file = value
                    .get("result_file")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let content = value
                    .get("content")
                    .cloned()
                    .or_else(|| {
                        result_file
                            .as_deref()
                            .and_then(|file| load_result_content(run_dir, file))
                    })
                    .unwrap_or(Value::Null);
                let id = tool_use_id(&value, turn, sequence, tool);
                events.push(replay_event(
                    snapshot,
                    tool_result_event(
                        &id,
                        turn,
                        sequence,
                        tool,
                        input,
                        summary,
                        duration_ms,
                        if error.trim().is_empty() {
                            None
                        } else {
                            Some(error)
                        },
                        &content,
                        result_file,
                    ),
                ));
            }
            "api_error" => {
                let turn = number_field(&value, "turn") as u32;
                push_turn_event(snapshot, &mut events, &mut last_turn, turn);
                let message = value
                    .get("error")
                    .or_else(|| value.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("api error");
                events.push(replay_event(
                    snapshot,
                    AgentTaskEventKind::ApiError {
                        turn,
                        message: message.to_string(),
                        text: format!("turn {turn}: {message}"),
                    },
                ));
            }
            "task_end" => {
                let turn = number_field(&value, "turn") as u32;
                events.push(replay_event(
                    snapshot,
                    AgentTaskEventKind::Done {
                        run_id: snapshot.run_id.clone(),
                        turns: turn,
                        text: if turn > 0 {
                            format!("done in {turn} turns")
                        } else {
                            "done".into()
                        },
                    },
                ));
            }
            _ => {}
        }
    }
    events
}

fn replay_run_state_events(
    snapshot: &AgentTaskSnapshot,
    path: &Path,
) -> Vec<AgentTaskEventPayload> {
    let mut events = Vec::new();
    let mut last_turn = None;
    for value in read_jsonl_values(path) {
        let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        match event_type {
            "assistant_turn" => {
                let turn = number_field(&value, "turn") as u32;
                push_turn_event(snapshot, &mut events, &mut last_turn, turn);
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if !text.is_empty() {
                    events.push(replay_event(
                        snapshot,
                        AgentTaskEventKind::Assistant {
                            turn,
                            text: truncate_event_text(text),
                        },
                    ));
                }
            }
            "tool_call" => {
                let turn = number_field(&value, "turn") as u32;
                push_turn_event(snapshot, &mut events, &mut last_turn, turn);
                let tool = value.get("tool").and_then(Value::as_str).unwrap_or("tool");
                let input = value.get("input").unwrap_or(&Value::Null);
                let id = tool_use_id(&value, turn, 0, tool);
                events.push(replay_event(
                    snapshot,
                    tool_call_event(&id, turn, 0, tool, input, 0),
                ));
            }
            "tool_result" => {
                let turn = number_field(&value, "turn") as u32;
                push_turn_event(snapshot, &mut events, &mut last_turn, turn);
                let tool = value.get("tool").and_then(Value::as_str).unwrap_or("tool");
                let input = value.get("input").unwrap_or(&Value::Null);
                let summary = value
                    .get("result_summary")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let duration_ms = duration_ms(&value);
                let id = tool_use_id(&value, turn, 0, tool);
                events.push(replay_event(
                    snapshot,
                    tool_result_event(
                        &id,
                        turn,
                        0,
                        tool,
                        input,
                        summary,
                        duration_ms,
                        None,
                        &Value::Null,
                        None,
                    ),
                ));
            }
            _ => {}
        }
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
                let text = snapshot
                    .turns
                    .map(|turns| format!("done in {turns} turns"))
                    .unwrap_or_else(|| "done".into());
                push_snapshot_event(
                    snapshot,
                    events,
                    AgentTaskEventKind::Done {
                        run_id: snapshot.run_id.clone(),
                        turns: snapshot.turns.unwrap_or_default(),
                        text,
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

fn push_turn_event(
    snapshot: &AgentTaskSnapshot,
    events: &mut Vec<AgentTaskEventPayload>,
    last_turn: &mut Option<u32>,
    turn: u32,
) {
    if turn == 0 || *last_turn == Some(turn) {
        return;
    }
    *last_turn = Some(turn);
    events.push(replay_event(
        snapshot,
        AgentTaskEventKind::Turn {
            turn,
            text: format!("turn {turn}"),
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
    turn: u32,
    sequence: u32,
    name: &str,
    input: &Value,
    repeat_count: u32,
) -> AgentTaskEventKind {
    AgentTaskEventKind::ToolCall {
        id: id.to_string(),
        turn,
        sequence_in_turn: sequence,
        name: name.to_string(),
        label: user_label(name),
        args: input.clone(),
        repeat_count,
        text: format_tool_call_text(name, input, repeat_count.into()),
    }
}

fn tool_result_event(
    id: &str,
    turn: u32,
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
    let (_kind, text) = format_tool_result_text(name, summary, duration_ms, error.unwrap_or(""));
    if let Some(error) = error {
        AgentTaskEventKind::ToolError {
            id: id.to_string(),
            turn,
            sequence_in_turn: sequence,
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
            turn,
            sequence_in_turn: sequence,
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
    summary: &str,
    duration_ms: u64,
    error: &str,
) -> (&'static str, String) {
    if !error.trim().is_empty() {
        return ("tool_error", format!("{tool} ({duration_ms}ms): {error}"));
    }
    let first = summary.lines().next().unwrap_or("");
    ("tool_result", format!("{tool} ({duration_ms}ms): {first}"))
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
    // Like user_label, this runs at write time, so only the current tool name
    // reaches here. `--preview` yields a `cards` array (-> card grid); the
    // default full scan yields the aggregated bundle (-> xhs_search).
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

fn load_result_content(run_dir: &Path, result_file: &str) -> Option<Value> {
    let path = run_dir.join(result_file);
    let text = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    value.get("content").cloned()
}

fn tool_use_id(value: &Value, turn: u32, sequence: u32, tool: &str) -> String {
    value
        .get("tool_use_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("turn:{turn}:sequence:{sequence}:tool:{tool}"))
}

fn duration_ms(value: &Value) -> u64 {
    if let Some(ms) = value.get("duration_ms").and_then(Value::as_u64) {
        return ms;
    }
    value
        .get("duration_s")
        .and_then(Value::as_f64)
        .map(|seconds| (seconds * 1000.0).round().max(0.0) as u64)
        .unwrap_or_default()
}

fn number_field(value: &Value, field: &str) -> u64 {
    value
        .get(field)
        .and_then(|number| {
            number
                .as_u64()
                .or_else(|| number.as_f64().map(|n| n as u64))
        })
        .unwrap_or_default()
}

fn read_jsonl_values(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
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
