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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare(
    registry: &AgentTaskRegistry,
    owner: &str,
    request_id: &str,
    hash: &str,
    uploads: Vec<String>,
    attachments: Vec<String>,
    previous: Option<&AgentTaskSnapshot>,
    create: impl FnOnce() -> Result<AgentTaskSnapshot>,
) -> Result<Admission> {
    staging::check_owner(owner)?;
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
                request_hash: hash.into(),
            });
            Intent {
                version: 1,
                snapshot,
                uploads: uploads.clone(),
                attachments: attachments.clone(),
                claimed: false,
                previous_run_dir: previous.and_then(|p| p.run_dir.clone()),
                previous_context: previous.and_then(|p| p.input_context.clone()),
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
    {
        bail!("The original input request has different content; recover that request before changing it");
    }
    let scope = staging::uuid(&context.scope_id)?;
    if intent.claimed {
        // A revoked/deleted remote scope must never resurrect a removed row.
        api::native_input_files(owner, scope).await?;
        cleanup_claimed_copies(&intent);
        if let Some(snapshot) = registry.live_input_claim(&intent.snapshot).await {
            return Ok(Admission::Recovered(snapshot));
        }
        let _lease = conversation_lease(owner, &intent.snapshot.task_id)?;
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
        return Ok(Admission::Recovered(recovered));
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
