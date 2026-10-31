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
    #[serde(default)]
    keep_inputs: bool,
    #[serde(default)]
    inputs_kept: bool,
}

// A terminal receipt must reject late/replayed IPC without retaining the
// original message, model settings or full planned task snapshot.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u8,
    context: NativeTaskInput,
    task_id: String,
    run_hash: Option<String>,
    uploads: Vec<String>,
    attachments: Vec<String>,
    outcome: ReceiptOutcome,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReceiptOutcome {
    Accepted,
    Cancelled,
    KeptInputs,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Journal {
    Intent(Intent),
    Receipt(Receipt),
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
    keep_inputs: bool,
}

#[derive(Serialize)]
pub(crate) struct SubmissionIndex {
    pending: Vec<PendingSubmission>,
    closed: Vec<String>,
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

fn read_journal(owner: &str, request_id: &str) -> Result<Journal> {
    let id = staging::uuid(request_id)?;
    if id.to_string() != request_id {
        bail!("Invalid submission ID");
    }
    let journal: Journal = serde_json::from_slice(&staging::read_private(
        &directory(owner)?.join(format!("{id}.json")),
        MAX_JOURNAL,
    )?)?;
    if let Journal::Receipt(receipt) = &journal {
        validate_ids(&receipt.uploads)?;
        validate_ids(&receipt.attachments)?;
        staging::uuid(&receipt.context.scope_id)?;
        if receipt.version != 2
            || receipt.context.owner != owner
            || receipt.context.request_id != request_id
            || receipt.context.request_hash.len() != 64
            || !receipt
                .context
                .request_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || receipt.uploads.len() != receipt.attachments.len()
            || receipt.task_id.is_empty()
            || receipt.run_hash.as_ref().is_some_and(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            bail!("Input receipt does not match its private identity");
        }
        return Ok(journal);
    }
    let Journal::Intent(intent) = &journal else {
        unreachable!()
    };
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
        || (intent.keep_inputs && (!intent.abandoned || intent.claimed || intent.cancelled))
        || (intent.inputs_kept && !intent.keep_inputs)
        || intent
            .request
            .as_ref()
            .is_some_and(|value| request_hash(value).ok().as_ref() != Some(&context.request_hash))
    {
        bail!("Input intent does not match its private identity");
    }
    Ok(journal)
}

fn finish(intent: &Intent, outcome: ReceiptOutcome) -> Result<()> {
    let context = intent
        .snapshot
        .input_context
        .as_ref()
        .context("Missing input context")?;
    let receipt = Receipt {
        version: 2,
        context: context.clone(),
        task_id: intent.snapshot.task_id.clone(),
        run_hash: run_fingerprint(&intent.snapshot.run_dir),
        uploads: intent.uploads.clone(),
        attachments: intent.attachments.clone(),
        outcome,
    };
    // Replace the large journal first. A crash before forgetting its pending
    // reference leaves a terminal receipt, never a new executable request.
    write_journal(&context.owner, &context.request_id, &receipt)?;
    forget_pending(&context.owner, &context.request_id)
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
    known_request_ids: Option<Vec<String>>,
) -> std::result::Result<SubmissionIndex, String> {
    let result = async {
        let _guard = staging::staging(&owner).await?;
        let mut result = Vec::new();
        let mut closed = Vec::new();
        let mut ids = pending_ids(&owner)?;
        let references: HashSet<_> = ids.iter().copied().collect();
        let known = known_request_ids.unwrap_or_default();
        if known.len() > 20 {
            bail!("Too many cached input request IDs");
        }
        for request in known {
            let id = staging::uuid(&request)?;
            if id.to_string() != request {
                bail!("Invalid cached input request ID");
            }
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        // Only unacknowledged references are listed. No full history scan on
        // app startup. At most 20 references plus 20 cached IDs are inspected.
        for id in ids {
            let intent = match read_journal(&owner, &id.to_string()) {
                Ok(Journal::Intent(intent)) => intent,
                Ok(Journal::Receipt(_)) => {
                    forget_pending(&owner, &id.to_string())?;
                    closed.push(id.to_string());
                    continue;
                }
                Err(error)
                    if !references.contains(&id)
                        && error
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|value| value.kind() == std::io::ErrorKind::NotFound) =>
                {
                    if cancellation_path(&owner, id)?.try_exists()? {
                        closed.push(id.to_string());
                    }
                    // No received IPC yet: leave the renderer's original
                    // cached intent available for explicit Send/cancellation.
                    continue;
                }
                Err(error) => return Err(error),
            };
            if !references.contains(&id) {
                ensure_pending(&intent)?;
            }
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
                keep_inputs: intent.keep_inputs,
            });
        }
        staging::check_owner(&owner)?;
        if result.len() > 20 {
            bail!("Too many pending input submissions; inspect their original tasks");
        }
        Ok::<_, anyhow::Error>(SubmissionIndex {
            pending: result,
            closed,
        })
    }
    .await;
    result.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub(crate) async fn input_submission_ack(
    owner: String,
    request_id: String,
    tasks: tauri::State<'_, AgentTaskRegistry>,
) -> std::result::Result<(), String> {
    let result = async {
        let _guard = staging::staging(&owner).await?;
        let intent = match read_journal(&owner, &request_id)? {
            Journal::Receipt(receipt) if receipt.outcome == ReceiptOutcome::Accepted => {
                return forget_pending(&owner, &request_id);
            }
            Journal::Receipt(_) => {
                bail!("An abandoned submission cannot be acknowledged as dispatched")
            }
            Journal::Intent(intent) => intent,
        };
        if !intent.claimed {
            bail!("An unsent input request must be explicitly cancelled, not acknowledged");
        }
        // Do not discard the only recovery snapshot if claim was saved but
        // task-index admission failed. A successful Send or explicit recovery
        // must have durably installed this exact turn first.
        tasks
            .checkpoint_input_claim(&intent.snapshot)
            .await
            .map_err(anyhow::Error::msg)?;
        cleanup_copies(&intent)?;
        finish(&intent, ReceiptOutcome::Accepted)
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
        let mut intent = match read_journal(&owner, &request_id) {
            Ok(Journal::Intent(intent)) => intent,
            Ok(Journal::Receipt(receipt)) if receipt.outcome == ReceiptOutcome::Cancelled => {
                return forget_pending(&owner, &request_id);
            }
            Ok(Journal::Receipt(_)) => bail!("This request is already closed; cancellation cannot remove its conversation's inputs"),
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
        if intent.keep_inputs { bail!("This submission is being ended with inputs retained; continue that resolution rather than deleting its inputs"); }
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
        finish(&intent, ReceiptOutcome::Cancelled)
    }.await;
    result.map_err(|e| format!("{e:#}"))
}

/// Explicitly end only this unsent message while retaining its inputs in the
/// original existing conversation. This contains no claim, spawn or billing.
#[tauri::command]
pub(crate) async fn input_submission_keep_inputs(
    owner: String,
    request_id: String,
    tasks: tauri::State<'_, AgentTaskRegistry>,
) -> std::result::Result<String, String> {
    let result = async {
        let _guard = staging::staging(&owner).await?;
        let mut intent = match read_journal(&owner, &request_id) {
            Ok(Journal::Intent(intent)) => intent,
            Ok(Journal::Receipt(receipt)) if receipt.outcome == ReceiptOutcome::KeptInputs => {
                forget_pending(&owner, &request_id)?;
                return Ok("kept".to_string());
            }
            Ok(Journal::Receipt(_)) => bail!("This request is already closed with another outcome"),
            Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|value| value.kind() == std::io::ErrorKind::NotFound) => {
                let id = staging::uuid(&request_id)?;
                if pending_ids(&owner)?.contains(&id) { bail!("Original pending journal is missing; inspect the task before discarding any record"); }
                cancel_not_received(&owner, &request_id)?;
                return Ok("unreceived_drafts".to_string());
            }
            Err(error) => return Err(error),
        };
        if intent.claimed || intent.cancelled || (intent.abandoned && !intent.keep_inputs) {
            bail!("This request cannot be ended as an unsent message with inputs retained");
        }
        let context = intent.snapshot.input_context.as_ref().context("Missing input context")?.clone();
        let previous = intent.previous_context.as_ref().context(
            "Retaining inputs requires an existing input conversation; cancel a new unsent conversation instead")?;
        if previous.owner != owner || previous.scope_id != context.scope_id {
            bail!("Input conversation identity does not match");
        }
        let _lease = conversation_lease(&owner, &intent.snapshot.task_id)?;
        let current = tasks.get(&intent.snapshot.task_id).await.context(
            "Original conversation is missing; inspect it before resolving inputs")?;
        if current.input_context.as_ref().is_none_or(|value| value.owner != owner || value.scope_id != context.scope_id)
            || matches!(current.status.as_str(), "queued" | "running") {
            bail!("Return to the original idle input conversation before retaining its inputs");
        }
        // Save the no-replay decision BEFORE (possibly first) remote binding.
        // A failed/ambiguous bind leaves only this same retention action retryable.
        intent.abandoned = true;
        intent.keep_inputs = true;
        write(&intent)?;
        if !intent.inputs_kept {
        let mut records = Vec::new();
        for (upload, attachment) in intent.uploads.iter().zip(&intent.attachments) {
            let record = staging::read_record(&owner, staging::uuid(upload)?)?;
            if record.remote.as_ref().map(|value| &value.id) != Some(attachment)
                || record.bound_scope.as_ref().is_some_and(|value| value != &context.scope_id) {
                bail!("Retained input copy does not match its original request");
            }
            records.push(record);
        }
        for record in &mut records {
            record.bound_scope = Some(context.scope_id.clone());
            staging::write_record(record)?;
        }
        let accepted = api::bind_native_input_files(&owner, staging::uuid(&context.scope_id)?, &intent.attachments).await?;
        staging::check_owner(&owner)?;
        for record in &records {
            let remote = record.remote.as_ref().context("Missing original upload acknowledgement")?;
            if !accepted.input_files.iter().any(|value| value.id == remote.id && value.size == remote.size
                && value.sha256 == remote.sha256 && value.name == remote.name) {
                bail!("Input retention could not be confirmed; retry this same request");
            }
        }
        intent.inputs_kept = true;
        write(&intent)?;
        }
        cleanup_copies(&intent)?;
        finish(&intent, ReceiptOutcome::KeptInputs)?;
        Ok("kept".to_string())
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
    write_journal(&context.owner, &context.request_id, intent)
}

fn write_journal(owner: &str, request_id: &str, value: &impl Serialize) -> Result<()> {
    staging::check_owner(owner)?;
    let id = staging::uuid(request_id)?;
    let parent = directory(owner)?;
    let bytes = serde_json::to_vec(value)?;
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
        match read_journal(&owner, &request_id)? {
            Journal::Intent(intent) => recover_claim(tasks.inner(), &intent).await,
            Journal::Receipt(receipt) => recover_receipt(tasks.inner(), &receipt).await,
        }
    }
    .await;
    result.map_err(|e| format!("{e:#}"))
}

async fn recover_receipt(
    registry: &AgentTaskRegistry,
    receipt: &Receipt,
) -> Result<AgentTaskSnapshot> {
    if receipt.outcome != ReceiptOutcome::Accepted {
        bail!("This request was explicitly ended without dispatch; it cannot be replayed");
    }
    let current = registry.get(&receipt.task_id).await.context(
        "The confirmed submission's task is no longer present. Inspect history; it will not be recreated or replayed")?;
    if current.input_context.as_ref() != Some(&receipt.context)
        || run_fingerprint(&current.run_dir) != receipt.run_hash
    {
        bail!("The confirmed conversation has advanced. Inspect its history; this receipt cannot replay or replace a newer turn");
    }
    api::native_input_files(
        &receipt.context.owner,
        staging::uuid(&receipt.context.scope_id)?,
    )
    .await?;
    staging::check_owner(&receipt.context.owner)?;
    Ok(current)
}

fn run_fingerprint(path: &Option<String>) -> Option<String> {
    // Run folder names contain a user-message slug; do not retain that path
    // in an otherwise text-free terminal receipt.
    path.as_ref()
        .map(|value| hex::encode(Sha256::digest(value.as_bytes())))
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
    let intent = match read_journal(owner, request_id) {
        Ok(Journal::Intent(intent)) => intent,
        Ok(Journal::Receipt(receipt)) => {
            let expected_task_id = previous
                .map(|value| value.task_id.clone())
                .unwrap_or_else(|| format!("task-input-{request}"));
            if receipt.context.request_hash != hash
                || receipt.uploads != uploads
                || receipt.attachments != attachments
                || receipt.task_id != expected_task_id
            {
                bail!("The closed request has different content; do not reuse its submission ID");
            }
            return Ok(Admission::Recovered(
                recover_receipt(registry, &receipt).await?,
            ));
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
                keep_inputs: false,
                inputs_kept: false,
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
    if cleanup_copies(intent).is_err() {
        eprintln!("accepted input retry copy retained; cleanup is still pending");
    }
}

fn cleanup_copies(intent: &Intent) -> Result<()> {
    let Some(context) = intent.snapshot.input_context.as_ref() else {
        bail!("Missing input context for private copy cleanup");
    };
    for (upload, attachment) in intent.uploads.iter().zip(&intent.attachments) {
        let id = staging::uuid(upload)?;
        let path = staging::root(&context.owner)?.join(format!("{id}.json"));
        if !path.try_exists()? {
            continue;
        }
        let record = staging::read_record(&context.owner, id)?;
        if record.bound_scope.as_ref() != Some(&context.scope_id)
            || record.remote.as_ref().map(|value| &value.id) != Some(attachment)
        {
            bail!("Accepted input cleanup does not match its saved scope/upload");
        }
        staging::remove_local(&context.owner, id)?;
    }
    Ok(())
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
