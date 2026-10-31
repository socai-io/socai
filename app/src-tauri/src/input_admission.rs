//! Durable native submission intent. Local claim and remote binding are not
//! one transaction: uncertainty is recovered without automatically re-running.
use std::{collections::HashSet, fs::File, io::Write, path::PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use socai_core::cloud::input_files as api;

use crate::{
    input_files as staging,
    tasks::{AgentTaskRegistry, AgentTaskSnapshot, NativeTaskInput},
};

const MAX_JOURNAL: usize = 256 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u8,
    snapshot: AgentTaskSnapshot,
    uploads: Vec<String>,
    attachments: Vec<String>,
    claimed: bool,
    previous_run_dir: Option<String>,
    previous_context: Option<NativeTaskInput>,
    #[serde(default)]
    request: Option<serde_json::Value>,
    #[serde(default)]
    abandoned: bool,
    #[serde(default)]
    cancelled: bool,
}

pub(crate) enum Admission {
    Recovered(AgentTaskSnapshot),
    Ready(Prepared),
}

pub(crate) struct Prepared {
    intent: Intent,
    // Cancellation must not allow another process to dispatch the same turn.
    lease: File,
    _staging: staging::StagingGuard,
}

pub(crate) fn request_hash(value: &serde_json::Value) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

#[derive(Serialize)]
pub(crate) struct PendingSubmission {
    request_id: String,
    task_id: String,
    scope_id: String,
    request: Option<serde_json::Value>,
    uploads: Vec<String>,
    attachments: Vec<String>,
    claimed: bool,
    abandoned: bool,
}

fn pending_directory(owner: &str) -> Result<PathBuf> {
    let path = directory(owner)?.join("pending");
    staging::private_directory(&path)?;
    Ok(path)
}

fn cancellation_path(owner: &str, id: uuid::Uuid) -> Result<PathBuf> {
    let parent = directory(owner)?.join("cancelled-requests");
    staging::private_directory(&parent)?;
    Ok(parent.join(format!("{id}.json")))
}

fn cancel_not_received(owner: &str, request_id: &str) -> Result<()> {
    let path = cancellation_path(owner, staging::uuid(request_id)?)?;
    let parent = path.parent().context("Missing cancellation directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(b"1")?;
    file.as_file().sync_all()?;
    file.persist(&path).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    // No admission journal means no bind/dispatch was started. Keep unbound
    // drafts for explicit removal; the tombstone blocks a delayed IPC Send.
    Ok(())
}

fn pending_ids(owner: &str) -> Result<Vec<uuid::Uuid>> {
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(pending_directory(owner)?)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let id = staging::uuid(
            path.file_stem()
                .and_then(|value| value.to_str())
                .context("Invalid pending input reference")?,
        )?;
        if staging::read_private(&path, 16)? != b"1" {
            bail!("Invalid pending input reference");
        }
        ids.push(id);
        if ids.len() > 20 {
            bail!("More than 20 pending input submissions; inspect them before submitting again");
        }
    }
    ids.sort();
    Ok(ids)
}

fn ensure_pending(intent: &Intent) -> Result<()> {
    let context = intent
        .snapshot
        .input_context
        .as_ref()
        .context("Missing input context")?;
    let id = staging::uuid(&context.request_id)?;
    let existing = pending_ids(&context.owner)?;
    if existing.contains(&id) {
        return Ok(());
    }
    if existing.len() >= 20 {
        bail!("Review or cancel pending input submissions before sending another");
    }
    let parent = pending_directory(&context.owner)?;
    let mut file = tempfile::NamedTempFile::new_in(&parent)?;
    file.write_all(b"1")?;
    file.as_file().sync_all()?;
    file.persist(parent.join(format!("{id}.json")))
        .map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn read_intent(owner: &str, request_id: &str) -> Result<Intent> {
    let id = staging::uuid(request_id)?;
    if id.to_string() != request_id {
        bail!("Invalid submission ID");
    }
    let intent: Intent = serde_json::from_slice(&staging::read_private(
        &directory(owner)?.join(format!("{id}.json")),
        MAX_JOURNAL,
    )?)?;
    let context = intent
        .snapshot
        .input_context
        .as_ref()
        .context("Missing input context")?;
    validate_ids(&intent.uploads)?;
    validate_ids(&intent.attachments)?;
    staging::uuid(&context.scope_id)?;
    if intent.version != 1
        || context.owner != owner
        || context.request_id != request_id
        || intent.uploads.len() != intent.attachments.len()
        || (intent.claimed && intent.abandoned)
        || (intent.cancelled && !intent.abandoned)
        || intent
            .request
            .as_ref()
            .is_some_and(|value| request_hash(value).ok().as_ref() != Some(&context.request_hash))
    {
        bail!("Input intent does not match its private identity");
    }
    Ok(intent)
}

fn forget_pending(owner: &str, request_id: &str) -> Result<()> {
    let id = staging::uuid(request_id)?;
    let parent = pending_directory(owner)?;
    if let Err(error) = std::fs::remove_file(parent.join(format!("{id}.json"))) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error.into());
        }
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn input_submissions(
    owner: String,
) -> std::result::Result<Vec<PendingSubmission>, String> {
    let result = async {
        let _guard = staging::staging(&owner).await?;
        let mut result = Vec::new();
        // Only unacknowledged references are listed. No full history scan on
        // app startup; at most 20 bounded private journals are read here.
        for id in pending_ids(&owner)? {
            let intent = read_intent(&owner, &id.to_string())?;
            let context = intent
                .snapshot
                .input_context
                .as_ref()
                .context("Missing input context")?;
            result.push(PendingSubmission {
                request_id: id.to_string(),
                task_id: intent.snapshot.task_id.clone(),
                scope_id: context.scope_id.clone(),
                request: intent.request,
                uploads: intent.uploads,
                attachments: intent.attachments,
                claimed: intent.claimed,
                abandoned: intent.abandoned,
            });
        }
        staging::check_owner(&owner)?;
        Ok::<_, anyhow::Error>(result)
    }
    .await;
    result.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub(crate) async fn input_submission_ack(
    owner: String,
    request_id: String,
) -> std::result::Result<(), String> {
    let result = async {
        let _guard = staging::staging(&owner).await?;
        let intent = read_intent(&owner, &request_id)?;
        if !intent.claimed {
            bail!("An unsent input request must be explicitly cancelled, not acknowledged");
        }
        forget_pending(&owner, &request_id)
    }
    .await;
    result.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub(crate) async fn input_submission_cancel(
    owner: String,
    request_id: String,
    tasks: tauri::State<'_, AgentTaskRegistry>,
) -> std::result::Result<(), String> {
    let result = async {
        let _guard = staging::staging(&owner).await?;
        let mut intent = match read_intent(&owner, &request_id) {
            Ok(intent) => intent,
            Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {
                if pending_ids(&owner)?.contains(&staging::uuid(&request_id)?) {
                    bail!("Pending reference exists but its durable intent is missing; inspect the original task, do not discard this record");
                }
                cancel_not_received(&owner, &request_id)?;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if intent.claimed { bail!("This request already has a dispatch claim. Inspect/cancel the original task; it cannot be abandoned as unsent"); }
        let context = intent.snapshot.input_context.as_ref().context("Missing input context")?.clone();
        // try-lock (never await) keeps the staging/conversation lock order safe.
        let _lease = conversation_lease(&owner, &intent.snapshot.task_id)?;
        if let Some(current) = tasks.get(&intent.snapshot.task_id).await {
            if current.run_dir != intent.previous_run_dir || current.input_context != intent.previous_context
                || matches!(current.status.as_str(), "queued" | "running") {
                bail!("Conversation changed after this request. Inspect its inputs before removing anything");
            }
        } else if intent.previous_run_dir.is_some() && !intent.cancelled {
            bail!("Original conversation is missing; inspect the saved request before removing inputs");
        }
        // Persist the no-replay decision before any network deletion. An
        // interrupted cancellation remains listed and can only be cancelled.
        intent.abandoned = true;
        write(&intent)?;
        if !intent.cancelled {
            api::cancel_native_input_admission(&owner, staging::uuid(&context.scope_id)?,
                &intent.attachments, intent.previous_context.is_some()).await?;
            staging::check_owner(&owner)?;
            intent.cancelled = true;
            write(&intent)?;
        }
        for (upload, attachment) in intent.uploads.iter().zip(&intent.attachments) {
            let id = staging::uuid(upload)?;
            let path = staging::root(&owner)?.join(format!("{id}.json"));
            if !path.try_exists()? { continue; }
            let record = staging::read_record(&owner, id)?;
            if record.remote.as_ref().map(|remote| &remote.id) != Some(attachment)
                || record.bound_scope.as_ref().is_some_and(|scope| scope != &context.scope_id) {
                bail!("Cancelled input copy does not match the original intent");
            }
            staging::remove_local(&owner, id)?;
        }
        forget_pending(&owner, &request_id)
    }.await;
    result.map_err(|e| format!("{e:#}"))
}

fn directory(owner: &str) -> Result<PathBuf> {
    let path = staging::root(owner)?.join("admissions");
    staging::private_directory(&path)?;
    Ok(path)
}

// One lease per conversation, including subsequent turns and deletion. Unlike
// the short staging lock it is held by the spawned runner until it terminates.
pub(crate) fn conversation_lease(owner: &str, task_id: &str) -> Result<File> {
    staging::check_owner(owner)?;
    let name = hex::encode(Sha256::digest(task_id.as_bytes()));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(directory(owner)?.join(format!("{name}.lock")))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("Input dispatch lock is not a regular file");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            bail!("Input dispatch lock cannot be a reparse point");
        }
    }
    fs2::FileExt::try_lock_exclusive(&file).context(
        "This input conversation is still executing or changing in another Desktop operation; do not submit a new request to retry it")?;
    Ok(file)
}

fn write(intent: &Intent) -> Result<()> {
    let context = intent
        .snapshot
        .input_context
        .as_ref()
        .context("Missing input intent context")?;
    let id = staging::uuid(&context.request_id)?;
    let parent = directory(&context.owner)?;
    let bytes = serde_json::to_vec(intent)?;
    if bytes.len() > MAX_JOURNAL {
        bail!("Input submission metadata exceeds its limit");
    }
    let mut file = tempfile::NamedTempFile::new_in(&parent)?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    file.persist(parent.join(format!("{id}.json")))
        .map_err(|_| {
            anyhow::anyhow!("Cannot persist the input submission; retry the SAME request ID")
        })?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn validate_ids(values: &[String]) -> Result<()> {
    if values.len() > 10 {
        bail!("At most 10 input files can be submitted together");
    }
    let mut seen = HashSet::new();
    for value in values {
        let id = staging::uuid(value)?;
        if id.to_string() != *value || !seen.insert(id) {
            bail!("Input IDs must be unique canonical UUIDs");
        }
    }
    Ok(())
}

// Caller holds staging(). This function contains no task creation/spawn path.
async fn recover_claim(registry: &AgentTaskRegistry, intent: &Intent) -> Result<AgentTaskSnapshot> {
    if !intent.claimed || intent.abandoned {
        bail!("No executable claim exists; recovery will not send this request");
    }
    let context = intent
        .snapshot
        .input_context
        .as_ref()
        .context("Missing input context")?;
    api::native_input_files(&context.owner, staging::uuid(&context.scope_id)?).await?;
    cleanup_claimed_copies(intent);
    if let Some(snapshot) = registry.live_input_claim(&intent.snapshot).await {
        return Ok(snapshot);
    }
    let _lease = conversation_lease(&context.owner, &intent.snapshot.task_id)?;
    let recovered = registry
        .recover_input_claim(
            &intent.snapshot,
            &intent.previous_run_dir,
            &intent.previous_context,
        )
        .await
        .map_err(anyhow::Error::msg)?;
    if recovered.status == "interrupted" {
        if let (Some(session), Some(run)) = (&recovered.session_dir, &recovered.run_dir) {
            let mut conversation = socai_core::agent::Conversation::load(session)?;
            if !conversation.runs.iter().any(|entry| &entry.run_dir == run) {
                conversation.record_run(
                    recovered
                        .current_message
                        .as_deref()
                        .unwrap_or(&recovered.task),
                    "[input dispatch unconfirmed; not automatically replayed]",
                    &PathBuf::from(run),
                    "interrupted",
                );
            }
        }
    }
    Ok(recovered)
}

#[tauri::command]
pub(crate) async fn input_submission_recover(
    owner: String,
    request_id: String,
    tasks: tauri::State<'_, AgentTaskRegistry>,
) -> std::result::Result<AgentTaskSnapshot, String> {
    let result = async {
        let _guard = staging::staging(&owner).await?;
        recover_claim(tasks.inner(), &read_intent(&owner, &request_id)?).await
    }
    .await;
    result.map_err(|e| format!("{e:#}"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare(
    registry: &AgentTaskRegistry,
    owner: &str,
    request_id: &str,
    request_payload: serde_json::Value,
    uploads: Vec<String>,
    attachments: Vec<String>,
    previous: Option<&AgentTaskSnapshot>,
    create: impl FnOnce() -> Result<AgentTaskSnapshot>,
) -> Result<Admission> {
    staging::check_owner(owner)?;
    let hash = request_hash(&request_payload)?;
    let request = staging::uuid(request_id)?;
    if request.to_string() != request_id
        || hash.len() != 64
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        bail!("Invalid input submission identity");
    }
    validate_ids(&uploads)?;
    validate_ids(&attachments)?;
    if uploads.len() != attachments.len() {
        bail!("Every submitted input needs its confirmed upload ID");
    }
    let guard = staging::staging(owner).await?;
    if cancellation_path(owner, request)?.try_exists()? {
        bail!("This input submission was explicitly cancelled before admission; it cannot be replayed");
    }
    let path = directory(owner)?.join(format!("{request}.json"));
    let intent = match staging::read_private(&path, MAX_JOURNAL) {
        Ok(bytes) => {
            serde_json::from_slice::<Intent>(&bytes).context("Invalid input submission journal")?
        }
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            if previous.and_then(|p| p.input_context.as_ref()).is_none() && uploads.is_empty() {
                bail!("Select an input before starting an input conversation");
            }
            if previous.is_some_and(|p| matches!(p.status.as_str(), "queued" | "running")) {
                bail!("Wait for the current turn before replying");
            }
            let mut snapshot = create()?;
            let scope = if let Some(context) = previous.and_then(|p| p.input_context.as_ref()) {
                if context.owner != owner {
                    bail!("Return to the conversation's original account and service");
                }
                staging::uuid(&context.scope_id)?
            } else {
                uuid::Uuid::new_v4()
            };
            snapshot.input_context = Some(NativeTaskInput {
                owner: owner.into(),
                scope_id: scope.to_string(),
                request_id: request_id.into(),
                request_hash: hash.clone(),
            });
            Intent {
                version: 1,
                snapshot,
                uploads: uploads.clone(),
                attachments: attachments.clone(),
                claimed: false,
                previous_run_dir: previous.and_then(|p| p.run_dir.clone()),
                previous_context: previous.and_then(|p| p.input_context.clone()),
                request: Some(request_payload.clone()),
                abandoned: false,
                cancelled: false,
            }
        }
        Err(error) => return Err(error),
    };
    let context = intent
        .snapshot
        .input_context
        .as_ref()
        .context("Missing saved input context")?;
    let expected_task_id = previous
        .map(|p| p.task_id.clone())
        .unwrap_or_else(|| format!("task-input-{request}"));
    if intent.version != 1
        || context.owner != owner
        || context.request_id != request_id
        || context.request_hash != hash
        || intent.uploads != uploads
        || intent.attachments != attachments
        || intent.snapshot.task_id != expected_task_id
        || intent.snapshot.status != "queued"
        || intent
            .request
            .as_ref()
            .is_some_and(|value| value != &request_payload)
    {
        bail!("The original input request has different content; recover that request before changing it");
    }
    if intent.abandoned {
        bail!("This submission was explicitly abandoned; it cannot be sent or replayed");
    }
    let scope = staging::uuid(&context.scope_id)?;
    if intent.claimed {
        // A revoked/deleted remote scope must never resurrect a removed row.
        return Ok(Admission::Recovered(
            recover_claim(registry, &intent).await?,
        ));
    }
    if intent.previous_run_dir != previous.and_then(|p| p.run_dir.clone())
        || intent.previous_context != previous.and_then(|p| p.input_context.clone())
    {
        bail!("Conversation advanced after this input request was prepared; inspect its history before continuing");
    }
    let lease = conversation_lease(owner, &intent.snapshot.task_id)?;
    let mut records = Vec::new();
    for (upload, attachment) in uploads.iter().zip(&attachments) {
        let record = staging::read_record(owner, staging::uuid(upload)?)?;
        let remote = record
            .remote
            .as_ref()
            .context("Input upload has not been confirmed; retry its upload first")?;
        if remote.id != *attachment
            || record
                .bound_scope
                .as_ref()
                .is_some_and(|s| s != &scope.to_string())
        {
            bail!("Selected input belongs to a different upload or conversation");
        }
        records.push(record);
    }
    // Before any remote mutation: even if the response disappears, retries
    // bind the same owner/scope/files instead of creating a second task.
    write(&intent)?;
    ensure_pending(&intent)?;
    for record in &mut records {
        // Reserve the local copy for this intent before an ambiguous remote
        // bind. Removing or reusing it now must go through intent recovery.
        record.bound_scope = Some(scope.to_string());
        staging::write_record(record)?;
    }
    let bound = api::bind_native_input_files(owner, scope, &attachments).await?;
    staging::check_owner(owner)?;
    for record in &records {
        let remote = record
            .remote
            .as_ref()
            .context("Missing upload acknowledgement")?;
        if !bound.input_files.iter().any(|file| {
            file.id == remote.id
                && file.size == remote.size
                && file.sha256 == remote.sha256
                && file.name == remote.name
        }) {
            bail!("Input binding could not be confirmed; retry the SAME request");
        }
    }
    // Keep the planned snapshot immutable throughout any preflight retry.
    Ok(Admission::Ready(Prepared {
        intent,
        lease,
        _staging: guard,
    }))
}

impl Prepared {
    pub(crate) fn snapshot(&self) -> &AgentTaskSnapshot {
        &self.intent.snapshot
    }

    pub(crate) async fn claim(
        mut self,
        registry: &AgentTaskRegistry,
        previous: Option<&AgentTaskSnapshot>,
    ) -> Result<(AgentTaskSnapshot, File)> {
        let owner = self
            .intent
            .snapshot
            .input_context
            .as_ref()
            .context("Missing input owner")?
            .owner
            .clone();
        staging::check_owner(&owner)?;
        // Once this bit is durable, no automatic path may dispatch it again,
        // including index-write, event-delivery, process-exit or spawn failure.
        self.intent.claimed = true;
        write(&self.intent)?;
        registry
            .admit_input_snapshot(self.intent.snapshot.clone(), previous)
            .await
            .map_err(anyhow::Error::msg)?;
        cleanup_claimed_copies(&self.intent);
        Ok((self.intent.snapshot, self.lease))
    }
}

// Caller holds staging(). Only remove accepted copies for this exact scope.
// Errors retain local quota, never turn a claimed submission into a fresh run.
fn cleanup_claimed_copies(intent: &Intent) {
    let Some(context) = intent.snapshot.input_context.as_ref() else {
        return;
    };
    for upload in &intent.uploads {
        let cleanup = || -> Result<()> {
            let id = staging::uuid(upload)?;
            let path = staging::root(&context.owner)?.join(format!("{id}.json"));
            if !path.try_exists()? {
                return Ok(());
            }
            let record = staging::read_record(&context.owner, id)?;
            if record.bound_scope.as_ref() != Some(&context.scope_id) {
                bail!("Accepted input cleanup does not match its saved scope");
            }
            staging::remove_local(&context.owner, id)
        };
        if cleanup().is_err() {
            eprintln!("accepted input retry copy retained; cleanup is still pending");
        }
    }
}

// Called only after the remote scope deletion has been acknowledged. Journals
// remain private recovery records; this is not a claim of full physical erasure.
pub(crate) async fn cleanup_deleted_scope(owner: &str, scope: &str) -> Result<()> {
    staging::uuid(scope)?;
    let _guard = staging::staging(owner).await?;
    for record in staging::records(owner)? {
        if record.bound_scope.as_deref() == Some(scope) {
            staging::remove_local(owner, staging::uuid(&record.upload_id)?)?;
        }
    }
    staging::check_owner(owner)
}
