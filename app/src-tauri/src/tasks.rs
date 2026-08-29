use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use socai_core::agent::{mark_agent_run_status, Conversation};
use socai_core::runtime::BrowserTargetInfo;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::AbortHandle;

use crate::timeline::{self, AgentTaskEventKind, AgentTaskEventPayload};

const MAX_CONCURRENT_AGENT_TASKS: usize = 3;

#[derive(Clone)]
pub struct AgentTaskRegistry {
    inner: Arc<Mutex<AgentTaskRegistryInner>>,
    runner_permits: Arc<Semaphore>,
}

#[derive(Default)]
struct AgentTaskRegistryInner {
    next_seq: u64,
    tasks: Vec<AgentTaskSnapshot>,
    abort_handles: HashMap<String, AbortHandle>,
    timeline_next_seq: HashMap<String, u64>,
    timeline_locks: HashMap<String, Arc<Mutex<()>>>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct AgentTaskSnapshot {
    pub(crate) task_id: String,
    pub(crate) task: String,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) status: String,
    pub(crate) created_at: u64,
    pub(crate) started_at: Option<u64>,
    pub(crate) finished_at: Option<u64>,
    pub(crate) run_id: Option<String>,
    pub(crate) run_dir: Option<String>,
    pub(crate) session_dir: Option<String>,
    pub(crate) target_id: Option<String>,
    pub(crate) page_url: Option<String>,
    pub(crate) page_title_marker: Option<String>,
    // Hydrated from `<run_dir>/report.md` for API responses; not persisted in tasks.json.
    pub(crate) final_text: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) steps: Option<u32>,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) cached_input_tokens: Option<u64>,
    pub(crate) cache_creation_input_tokens: Option<u64>,
    pub(crate) estimated_cost: Option<f64>,
    pub(crate) cost_currency: Option<String>,
    /// Server-authoritative LLM + paid cloud-tool charge for the latest turn.
    pub(crate) points_used: Option<i64>,
    // The text driving the in-flight/most recent run — distinct from `task`,
    // which stays the thread's original title across replies. It is process-
    // local; interrupted startup recovery reads the canonical run.json task.
    pub(crate) current_message: Option<String>,
}

impl Default for AgentTaskRegistry {
    fn default() -> Self {
        let mut tasks = load_task_index();
        let interrupted_at = now_ms();
        for task in &mut tasks {
            if matches!(task.status.as_str(), "queued" | "running") {
                task.status = "interrupted".into();
                task.finished_at = Some(interrupted_at);
                task.error = Some("app was closed before this task finished".into());
                task.target_id = None;
                if let Some(run_dir) = task.run_dir.as_deref() {
                    let _ = mark_agent_run_status(
                        run_dir,
                        "interrupted",
                        Some("app was closed before this task finished"),
                    );
                }
                if let (Some(session_dir), Some(run_dir)) =
                    (task.session_dir.as_deref(), task.run_dir.as_deref())
                {
                    if let Ok(mut conversation) = Conversation::load(session_dir) {
                        let user_text = task
                            .current_message
                            .clone()
                            .or_else(|| persisted_run_task(run_dir))
                            .unwrap_or_else(|| task.task.clone());
                        conversation.record_run(
                            &user_text,
                            "[task interrupted: app was closed before this task finished]",
                            &PathBuf::from(run_dir),
                            "interrupted",
                        );
                    }
                }
            }
        }
        let next_seq = tasks.len() as u64;
        if !tasks.is_empty() {
            persist_task_index(&tasks);
        }
        Self {
            inner: Arc::new(Mutex::new(AgentTaskRegistryInner {
                next_seq,
                tasks,
                abort_handles: HashMap::new(),
                timeline_next_seq: HashMap::new(),
                timeline_locks: HashMap::new(),
            })),
            runner_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_AGENT_TASKS)),
        }
    }
}

fn persisted_run_task(run_dir: &str) -> Option<String> {
    let text = std::fs::read_to_string(PathBuf::from(run_dir).join("run.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .map(str::to_string)
}

impl AgentTaskRegistry {
    pub(crate) async fn create(
        &self,
        task: String,
        provider: Option<String>,
        model: Option<String>,
        run_dir: String,
        session_dir: String,
    ) -> AgentTaskSnapshot {
        let mut guard = self.inner.lock().await;
        guard.next_seq += 1;
        let task_id = format!("task-{}-{}", now_ms(), guard.next_seq);
        let snapshot = AgentTaskSnapshot {
            task_id,
            current_message: Some(task.clone()),
            task,
            provider,
            model,
            status: "queued".into(),
            created_at: now_ms(),
            started_at: None,
            finished_at: None,
            run_id: None,
            run_dir: Some(run_dir),
            session_dir: Some(session_dir),
            target_id: None,
            page_url: None,
            page_title_marker: None,
            final_text: None,
            error: None,
            steps: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            estimated_cost: None,
            cost_currency: None,
            points_used: None,
        };
        guard.tasks.push(snapshot.clone());
        persist_task_index(&guard.tasks);
        snapshot
    }

    pub(crate) async fn acquire_run_permit(&self) -> Option<OwnedSemaphorePermit> {
        self.runner_permits.clone().acquire_owned().await.ok()
    }

    pub(crate) async fn bind_target_if_active(
        &self,
        task_id: &str,
        target_id: String,
        page_url: Option<String>,
        page_title_marker: String,
    ) -> Option<(AgentTaskSnapshot, bool)> {
        let mut guard = self.inner.lock().await;
        let task = guard
            .tasks
            .iter_mut()
            .find(|task| task.task_id == task_id)?;
        if !matches!(task.status.as_str(), "queued" | "running") {
            return None;
        }
        let target_changed = task.target_id.as_deref() != Some(target_id.as_str());
        task.target_id = Some(target_id);
        if page_url.is_some() {
            task.page_url = page_url;
        }
        task.page_title_marker = Some(page_title_marker);
        let snapshot = hydrate_task_snapshot(task.clone());
        persist_task_index(&guard.tasks);
        Some((snapshot, target_changed))
    }

    /// Register the task abort handle. Returns the handle back to the caller
    /// if the task is already terminal (for example, cancelled by another
    /// window after task creation but before handle registration).
    pub(crate) async fn set_abort_handle(
        &self,
        task_id: &str,
        handle: AbortHandle,
    ) -> Option<AbortHandle> {
        let mut guard = self.inner.lock().await;
        let active = guard
            .tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .map(|task| matches!(task.status.as_str(), "queued" | "running"))
            .unwrap_or(false);
        if !active {
            return Some(handle);
        }
        if let Some(previous) = guard.abort_handles.insert(task_id.to_string(), handle) {
            previous.abort();
        }
        None
    }

    pub(crate) async fn remove_abort_handle(&self, task_id: &str) -> Option<AbortHandle> {
        self.inner.lock().await.abort_handles.remove(task_id)
    }

    pub(crate) async fn cancel(
        &self,
        task_id: &str,
    ) -> Option<(AgentTaskSnapshot, Option<AbortHandle>, Option<String>, bool)> {
        let mut guard = self.inner.lock().await;
        let pos = guard
            .tasks
            .iter()
            .position(|task| task.task_id == task_id)?;
        let changed = matches!(guard.tasks[pos].status.as_str(), "queued" | "running");
        let handle = if changed {
            guard.abort_handles.remove(task_id)
        } else {
            None
        };
        let target_id = guard.tasks[pos].target_id.clone();
        if changed {
            let task = &mut guard.tasks[pos];
            task.status = "cancelled".into();
            task.finished_at = Some(now_ms());
            task.target_id = None;
            task.error = None;
        }
        let snapshot = hydrate_task_snapshot(guard.tasks[pos].clone());
        persist_task_index(&guard.tasks);
        Some((snapshot, handle, target_id, changed))
    }

    /// Remove a task from the registry and persisted index. Refuses while the
    /// task is queued or running — the run loop still owns its run dir, so the
    /// caller must cancel first. Returns the removed snapshot so the caller
    /// can delete the on-disk artifacts it points at.
    pub(crate) async fn delete(&self, task_id: &str) -> Result<AgentTaskSnapshot, String> {
        let mut guard = self.inner.lock().await;
        let pos = guard
            .tasks
            .iter()
            .position(|task| task.task_id == task_id)
            .ok_or_else(|| format!("unknown task: {task_id}"))?;
        if matches!(guard.tasks[pos].status.as_str(), "queued" | "running") {
            return Err("task is still active; cancel it before deleting".into());
        }
        let snapshot = guard.tasks.remove(pos);
        guard.abort_handles.remove(task_id);
        guard.timeline_next_seq.remove(task_id);
        guard.timeline_locks.remove(task_id);
        persist_task_index(&guard.tasks);
        Ok(snapshot)
    }

    pub(crate) async fn interrupt_missing_targets(
        &self,
        active_targets: &HashSet<String>,
        reason: &str,
    ) -> Vec<(AgentTaskSnapshot, Option<AbortHandle>)> {
        let mut guard = self.inner.lock().await;
        let mut out = Vec::new();
        let mut task_ids = Vec::new();
        for task in &mut guard.tasks {
            if task.status != "running" {
                continue;
            }
            let Some(target_id) = task.target_id.as_ref() else {
                continue;
            };
            if active_targets.contains(target_id) {
                continue;
            }
            task.status = "interrupted".into();
            task.finished_at = Some(now_ms());
            task.error = Some(reason.into());
            task.target_id = None;
            task_ids.push(task.task_id.clone());
            out.push((task.clone(), None));
        }
        if !task_ids.is_empty() {
            for (idx, task_id) in task_ids.into_iter().enumerate() {
                out[idx].1 = guard.abort_handles.remove(&task_id);
            }
            persist_task_index(&guard.tasks);
        }
        out
    }

    pub(crate) async fn rebind_missing_targets(
        &self,
        targets: &[BrowserTargetInfo],
    ) -> Vec<AgentTaskSnapshot> {
        let active_targets: HashSet<&str> = targets
            .iter()
            .map(|target| target.target_id.as_str())
            .collect();
        let mut guard = self.inner.lock().await;
        let mut rebound = Vec::new();
        for task in &mut guard.tasks {
            if task.status != "running"
                || task
                    .target_id
                    .as_deref()
                    .is_some_and(|target_id| active_targets.contains(target_id))
            {
                continue;
            }
            let marker_match = task
                .page_title_marker
                .as_deref()
                .and_then(|marker| targets.iter().find(|target| target.title.contains(marker)));
            let site_matches: Vec<&BrowserTargetInfo> = task
                .page_url
                .as_deref()
                .map(site_key)
                .filter(|site| !site.is_empty())
                .map(|site| {
                    targets
                        .iter()
                        .filter(|target| site_key(&target.url) == site)
                        .collect()
                })
                .unwrap_or_default();
            let candidate = marker_match.or_else(|| {
                if site_matches.len() == 1 {
                    site_matches.first().copied()
                } else {
                    None
                }
            });
            let Some(candidate) = candidate else {
                continue;
            };
            task.target_id = Some(candidate.target_id.clone());
            task.page_url = Some(candidate.url.clone());
            rebound.push(task.clone());
        }
        if !rebound.is_empty() {
            persist_task_index(&guard.tasks);
        }
        rebound
    }

    pub(crate) async fn list(&self) -> Vec<AgentTaskSnapshot> {
        self.inner
            .lock()
            .await
            .tasks
            .clone()
            .into_iter()
            .map(hydrate_task_snapshot)
            .collect()
    }

    pub(crate) async fn get(&self, task_id: &str) -> Option<AgentTaskSnapshot> {
        self.inner
            .lock()
            .await
            .tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .cloned()
            .map(hydrate_task_snapshot)
    }

    pub(crate) async fn events(&self, task_id: &str) -> Option<Vec<AgentTaskEventPayload>> {
        let snapshot = self.get(task_id).await?;
        Some(timeline::load_task_events(&snapshot))
    }

    pub(crate) async fn live_timeline_event(
        &self,
        task_id: &str,
        payload: AgentTaskEventKind,
        snapshot_for_emit: Option<AgentTaskSnapshot>,
    ) -> Option<AgentTaskEventPayload> {
        let (snapshot, timeline_lock) = {
            let mut guard = self.inner.lock().await;
            let snapshot = guard
                .tasks
                .iter()
                .find(|task| task.task_id == task_id)?
                .clone();
            let timeline_lock = guard
                .timeline_locks
                .entry(task_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone();
            (snapshot, timeline_lock)
        };

        let _timeline_guard = timeline_lock.lock().await;
        let sequence = self.next_timeline_sequence(task_id, &snapshot).await;
        Some(timeline::live_timeline_event(
            &snapshot,
            payload,
            snapshot_for_emit,
            sequence,
        ))
    }

    /// Next live sequence for a task's timeline. The counter is in-memory
    /// only, while the frontend dedupes/merges events on `task_id:sequence` —
    /// so after an app restart a fresh counter would reuse the sequences the
    /// replayed history already occupies, and a follow-up's live rows would
    /// overwrite earlier turns' rows in place. Seed a missing counter past
    /// the current replay length instead. The disk read happens at most once
    /// per task per process, under this task's timeline lock.
    async fn next_timeline_sequence(&self, task_id: &str, snapshot: &AgentTaskSnapshot) -> u64 {
        let seeded = {
            let guard = self.inner.lock().await;
            guard.timeline_next_seq.contains_key(task_id)
        };
        let seed = if seeded {
            1
        } else {
            timeline::load_task_events(snapshot).len() as u64 + 1
        };
        let mut guard = self.inner.lock().await;
        let next = guard
            .timeline_next_seq
            .entry(task_id.to_string())
            .or_insert(seed);
        let sequence = *next;
        *next = sequence.saturating_add(1);
        sequence
    }

    pub(crate) async fn update<F>(&self, task_id: &str, f: F) -> Option<AgentTaskSnapshot>
    where
        F: FnOnce(&mut AgentTaskSnapshot),
    {
        let mut guard = self.inner.lock().await;
        let snapshot = {
            let task = guard
                .tasks
                .iter_mut()
                .find(|task| task.task_id == task_id)?;
            f(task);
            task.clone()
        };
        persist_task_index(&guard.tasks);
        Some(hydrate_task_snapshot(snapshot))
    }

    /// Apply a terminal transition only if the task is still active (queued or
    /// running). Returns the updated snapshot when THIS call won the
    /// running→terminal race, or `None` when another path (cancel/interrupt)
    /// already finalized it. This is the single guard that keeps one task from
    /// emitting two terminal events.
    pub(crate) async fn finalize_if_active<F>(
        &self,
        task_id: &str,
        f: F,
    ) -> Option<AgentTaskSnapshot>
    where
        F: FnOnce(&mut AgentTaskSnapshot),
    {
        let mut guard = self.inner.lock().await;
        let task = guard
            .tasks
            .iter_mut()
            .find(|task| task.task_id == task_id)?;
        if !matches!(task.status.as_str(), "queued" | "running") {
            return None;
        }
        f(task);
        let snapshot = task.clone();
        persist_task_index(&guard.tasks);
        Some(hydrate_task_snapshot(snapshot))
    }
}

fn site_key(url: &str) -> String {
    url.trim()
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or_default()
        .trim_start_matches("www.")
        .to_ascii_lowercase()
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn hydrate_task_snapshot(mut snapshot: AgentTaskSnapshot) -> AgentTaskSnapshot {
    if let Some(run_dir) = snapshot.run_dir.as_deref() {
        snapshot.final_text = final_text_from_run_dir(run_dir);
        if let Ok(text) = std::fs::read_to_string(PathBuf::from(run_dir).join("run.json")) {
            if let Ok(run) = serde_json::from_str::<Value>(&text) {
                snapshot.run_id = run.get("id").and_then(Value::as_str).map(str::to_string);
                snapshot.error = run.get("error").and_then(Value::as_str).map(str::to_string);
                // Pre-rename runs (< #190) recorded the step count as "turns".
                snapshot.steps = run
                    .get("steps")
                    .or_else(|| run.get("turns"))
                    .and_then(Value::as_u64)
                    .map(|value| value as u32);
                snapshot.input_tokens = run.pointer("/usage/input_tokens").and_then(Value::as_u64);
                snapshot.output_tokens =
                    run.pointer("/usage/output_tokens").and_then(Value::as_u64);
                snapshot.cached_input_tokens = run
                    .pointer("/usage/cache_read_input_tokens")
                    .and_then(Value::as_u64);
                snapshot.cache_creation_input_tokens = run
                    .pointer("/usage/cache_creation_input_tokens")
                    .and_then(Value::as_u64);
                snapshot.estimated_cost = run.pointer("/usage/cost/total").and_then(Value::as_f64);
                snapshot.cost_currency = run
                    .pointer("/usage/cost/currency")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                snapshot.points_used = run
                    .pointer("/billing/points_used")
                    .and_then(Value::as_i64)
                    .or(snapshot.points_used);
            }
        }
    }
    snapshot
}

fn final_text_from_run_dir(run_dir: &str) -> Option<String> {
    let text = std::fs::read_to_string(PathBuf::from(run_dir).join("report.md")).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(crate) fn app_data_dir() -> PathBuf {
    if let Ok(home) = std::env::var("SOCAI_HOME") {
        return PathBuf::from(home).join("app");
    }
    // `dirs::home_dir()` resolves `%USERPROFILE%` on Windows (where `HOME` is
    // usually unset) and `$HOME` on unix — keeping the `~/.socai/app` layout on
    // every platform. The bare relative path is a last resort that should never
    // be hit once a home dir resolves; falling back to it on Windows would
    // scatter `tasks.json` into the CWD and lose it across launches.
    if let Some(home) = dirs::home_dir() {
        return home.join(".socai/app");
    }
    PathBuf::from(".socai/app")
}

fn task_index_path() -> PathBuf {
    app_data_dir().join("tasks.json")
}

fn load_task_index() -> Vec<AgentTaskSnapshot> {
    let path = task_index_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut tasks = serde_json::from_str::<Vec<AgentTaskSnapshot>>(&text).unwrap_or_default();
    for task in &mut tasks {
        task.final_text = None;
    }
    tasks
}

fn persist_task_index(tasks: &[AgentTaskSnapshot]) {
    let path = task_index_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Keep tasks.json as an app index only. Run id, result, error, token usage,
    // and target state are hydrated from the run or are process-local. Hosted
    // points are persisted because only the server can authoritatively settle
    // them; they cannot be reconstructed from the local run's estimated cost.
    let records: Vec<Value> = tasks
        .iter()
        .map(|task| {
            serde_json::json!({
                "task_id": &task.task_id,
                "task": &task.task,
                "provider": &task.provider,
                "model": &task.model,
                "status": &task.status,
                "created_at": task.created_at,
                "started_at": task.started_at,
                "finished_at": task.finished_at,
                "run_dir": &task.run_dir,
                "session_dir": &task.session_dir,
                "points_used": task.points_used,
            })
        })
        .collect();
    if let Ok(text) = serde_json::to_string_pretty(&records) {
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn empty_registry() -> AgentTaskRegistry {
        AgentTaskRegistry {
            inner: Arc::new(Mutex::new(AgentTaskRegistryInner::default())),
            runner_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_AGENT_TASKS)),
        }
    }

    #[tokio::test]
    async fn three_tasks_run_while_the_fourth_waits_for_a_released_permit() {
        let registry = empty_registry();
        let first = registry.acquire_run_permit().await.unwrap();
        let second = registry.acquire_run_permit().await.unwrap();
        let third = registry.acquire_run_permit().await.unwrap();

        assert!(
            tokio::time::timeout(Duration::from_millis(25), registry.acquire_run_permit())
                .await
                .is_err()
        );

        drop(first);
        let fourth =
            tokio::time::timeout(Duration::from_millis(250), registry.acquire_run_permit())
                .await
                .expect("the fourth task should leave the queue")
                .expect("the runner queue should stay open");

        drop((second, third, fourth));
    }
}
