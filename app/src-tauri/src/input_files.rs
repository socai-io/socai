//! Native picker and private retry bytes. No renderer-supplied file paths.
//!
//! Selecting/uploading is NOT task admission. Keep the composer capability
//! closed until durable conversation/task binding has been integrated.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use socai_core::cloud::input_files as api;
use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;
use uuid::Uuid;

const MAX_BYTES: usize = 20 * 1024 * 1024;
const MAX_STAGED_BYTES: usize = 50 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 16 * 1024;
static STAGING: Mutex<()> = Mutex::const_new(());

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    owner: String,
    upload_id: String,
    name: String,
    size: usize,
    sha256: String,
    attempted: bool,
    remote: Option<api::InputFileDescriptor>,
    bound_scope: Option<String>,
}

#[derive(Serialize)]
pub struct StagedInput {
    upload_id: String,
    name: String,
    size: usize,
    sha256: String,
    remote: Option<api::InputFileDescriptor>,
    attempted: bool,
}

impl From<Record> for StagedInput {
    fn from(value: Record) -> Self {
        Self {
            upload_id: value.upload_id,
            name: value.name,
            size: value.size,
            sha256: value.sha256,
            remote: value.remote,
            attempted: value.attempted,
        }
    }
}

#[derive(Serialize)]
pub struct NativeCapabilities {
    owner: String,
    #[serde(flatten)]
    transport: api::InputFileCapabilities,
    // Do not let a UI confuse available byte transport with task integration.
    desktop_admission_ready: bool,
}

#[derive(Serialize)]
pub struct PickerResult {
    inputs: Vec<StagedInput>,
    errors: Vec<String>,
}

#[derive(Clone, Serialize)]
struct UploadState {
    upload_id: String,
    phase: &'static str,
}

fn uuid(value: &str) -> Result<Uuid> {
    let id = Uuid::parse_str(value).context("Invalid private input ID")?;
    if id.get_version_num() != 4 {
        bail!("Private input ID must be UUID v4");
    }
    Ok(id)
}

fn check_owner(owner: &str) -> Result<()> {
    if owner.len() != 64
        || !owner.bytes().all(|byte| byte.is_ascii_hexdigit())
        || api::input_owner()? != owner
    {
        bail!("Input account or service changed; return to the original account before retrying");
    }
    Ok(())
}

fn supported_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 255
        && name.trim() == name
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        && name.rsplit_once('.').is_some_and(|(_, suffix)| {
            matches!(
                suffix.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "docx" | "txt" | "md" | "csv" | "json"
            )
        })
}

fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // Parent is the app's existing data directory, not a user-provided path.
        builder
            .create(path)
            .context("Cannot create private input directory")?;
    }
    if !std::fs::symlink_metadata(path)?.file_type().is_dir() {
        bail!("Private input directory is not a regular directory");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if std::fs::symlink_metadata(path)?.file_attributes() & 0x400 != 0 {
            bail!("Private input directory cannot be a reparse point");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn root(owner: &str) -> Result<PathBuf> {
    if owner.len() != 64 || !owner.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Invalid private input owner");
    }
    let app = crate::tasks::app_data_dir();
    std::fs::create_dir_all(&app)?;
    let base = app.join("pending-inputs-v1");
    private_directory(&base)?;
    let account = base.join(owner);
    private_directory(&account)?;
    Ok(account)
}

fn read_private(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options
        .open(path)
        .context("Private input retry data is unavailable")?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        bail!("Private input retry data is invalid or too large");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            bail!("Private input cannot be a reparse point");
        }
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        bail!("Private input retry data exceeds its limit");
    }
    Ok(bytes)
}

fn write_record(record: &Record) -> Result<()> {
    let id = uuid(&record.upload_id)?;
    let directory = root(&record.owner)?;
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() > MAX_METADATA_BYTES {
        bail!("Private input metadata exceeds its limit");
    }
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    file.persist(directory.join(format!("{id}.json")))
        .map_err(|_| anyhow::anyhow!("Cannot persist private input retry metadata"))?;
    #[cfg(unix)]
    File::open(directory)?.sync_all()?;
    Ok(())
}

fn read_record(owner: &str, id: Uuid) -> Result<Record> {
    let bytes = read_private(&root(owner)?.join(format!("{id}.json")), MAX_METADATA_BYTES)?;
    let record: Record =
        serde_json::from_slice(&bytes).context("Invalid private input retry metadata")?;
    if record.version != 1
        || record.owner != owner
        || record.upload_id != id.to_string()
        || !supported_name(&record.name)
        || record.size == 0
        || record.size > MAX_BYTES
        || record.sha256.len() != 64
        || !record.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || record
            .bound_scope
            .as_ref()
            .is_some_and(|scope| uuid(scope).is_err())
    {
        bail!("Private input retry metadata does not match its identity");
    }
    if let Some(remote) = &record.remote {
        if uuid(&remote.id).is_err()
            || remote.status != "ready"
            || remote.size != record.size
            || !remote.sha256.eq_ignore_ascii_case(&record.sha256)
            || !supported_name(&remote.name)
        {
            bail!("Stored upload acknowledgement does not match the selected input");
        }
    }
    Ok(record)
}

fn records(owner: &str) -> Result<Vec<Record>> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(root(owner)?)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .context("Invalid private input record")?;
        result.push(read_record(owner, uuid(id)?)?);
        // Fail closed instead of scanning an unbounded/tampered directory.
        if result.len() > 20 {
            bail!("Private input staging limit exceeded");
        }
    }
    result.sort_by(|left, right| left.upload_id.cmp(&right.upload_id));
    Ok(result)
}

fn stage(owner: &str, source: &Path) -> Result<StagedInput> {
    check_owner(owner)?;
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .context("Choose a file with a valid UTF-8 name")?;
    if !supported_name(name) {
        bail!("Choose Word (.docx), a supported image or UTF-8 text/CSV/JSON");
    }
    let mut source_options = OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A selected FIFO/device must not block staging before metadata can
        // reject it. Explicitly chosen symlinks to regular files remain valid.
        source_options.custom_flags(libc::O_NONBLOCK);
    }
    let source_file = source_options
        .open(source)
        .context("Cannot open selected input")?;
    let metadata = source_file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_BYTES as u64 {
        bail!("Selected input must be a non-empty regular file no larger than 20 MiB");
    }
    let existing = records(owner)?;
    let size: usize = existing.iter().map(|record| record.size).sum();
    if existing.len() >= 20 || size.saturating_add(metadata.len() as usize) > MAX_STAGED_BYTES {
        bail!("Local input drafts reached 20 files or 50 MiB; remove unused drafts first");
    }
    let mut bytes = Vec::new();
    (&source_file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let after = source_file.metadata()?;
    if bytes.len() != metadata.len() as usize
        || bytes.is_empty()
        || bytes.len() > MAX_BYTES
        || after.len() != metadata.len()
        || after.modified().ok() != metadata.modified().ok()
    {
        bail!("Selected input changed while reading; choose it again");
    }
    let id = Uuid::new_v4();
    let record = Record {
        version: 1,
        owner: owner.to_owned(),
        upload_id: id.to_string(),
        name: name.to_owned(),
        size: bytes.len(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        attempted: false,
        remote: None,
        bound_scope: None,
    };
    // Reserve the local quota before persisting bytes. A crash cannot leave
    // large retry data outside the counted reservation. Original paths are
    // never stored, so later edits to the original cannot change a retry.
    write_record(&record)?;
    let directory = root(owner)?;
    let mut file = tempfile::Builder::new()
        .prefix(&format!(".{id}."))
        .tempfile_in(&directory)?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(directory.join(format!("{id}.bin")))
        .map_err(|_| {
            anyhow::anyhow!(
                "Cannot persist selected input bytes; remove the incomplete draft before retrying"
            )
        })?;
    #[cfg(unix)]
    File::open(directory)?.sync_all()?;
    Ok(record.into())
}

async fn enabled(owner: &str) -> Result<()> {
    check_owner(owner)?;
    if !api::input_file_capabilities(owner).await?.enabled {
        bail!("Native input execution is not enabled on this service");
    }
    check_owner(owner)
}

#[tauri::command]
pub async fn input_files_capabilities() -> Result<NativeCapabilities, String> {
    let owner = api::input_owner().map_err(|error| error.to_string())?;
    let transport = api::input_file_capabilities(&owner)
        .await
        .map_err(|error| error.to_string())?;
    check_owner(&owner).map_err(|error| error.to_string())?;
    Ok(NativeCapabilities {
        owner,
        transport,
        desktop_admission_ready: false,
    })
}

/// OS picker only. No JS path/byte array is accepted, returned or persisted.
#[tauri::command]
pub async fn input_file_choose(app: AppHandle, owner: String) -> Result<PickerResult, String> {
    let _guard = STAGING.lock().await;
    enabled(&owner).await.map_err(|error| error.to_string())?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter(
            "Images, Word and text",
            &[
                "png", "jpg", "jpeg", "webp", "docx", "txt", "md", "csv", "json",
            ],
        )
        .pick_files(move |chosen| {
            let _ = sender.send(chosen);
        });
    let chosen = receiver
        .await
        .map_err(|_| "Input picker was interrupted".to_string())?;
    check_owner(&owner).map_err(|error| error.to_string())?;
    let Some(chosen) = chosen else {
        return Ok(PickerResult {
            inputs: Vec::new(),
            errors: Vec::new(),
        });
    };
    tokio::task::spawn_blocking(move || {
        let mut result = PickerResult {
            inputs: Vec::new(),
            errors: Vec::new(),
        };
        if chosen.len() > 10 {
            result.errors.push(
                "At most 10 files can be chosen per message; only the first 10 were staged".into(),
            );
        }
        for selected in chosen.into_iter().take(10) {
            let staged = selected
                .into_path()
                .map_err(|_| anyhow::anyhow!("Selected input is not a local file"))
                .and_then(|path| stage(&owner, &path));
            match staged {
                Ok(input) => result.inputs.push(input),
                Err(error) => result.errors.push(error.to_string()),
            }
        }
        check_owner(&owner)?;
        Ok::<_, anyhow::Error>(result)
    })
    .await
    .map_err(|_| "Input staging was interrupted".to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn input_file_drafts(owner: String) -> Result<Vec<StagedInput>, String> {
    let _guard = STAGING.lock().await;
    check_owner(&owner).map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        let result = records(&owner)?
            .into_iter()
            .filter(|record| record.bound_scope.is_none())
            .map(Into::into)
            .collect();
        check_owner(&owner)?;
        Ok::<_, anyhow::Error>(result)
    })
    .await
    .map_err(|_| "Cannot read private input drafts".to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn input_file_upload(
    app: AppHandle,
    owner: String,
    upload_id: String,
) -> Result<api::InputFileDescriptor, String> {
    let _guard = STAGING.lock().await;
    enabled(&owner).await.map_err(|error| error.to_string())?;
    let id = uuid(&upload_id).map_err(|error| error.to_string())?;
    let captured_owner = owner.clone();
    let (mut record, bytes) = tokio::task::spawn_blocking(move || {
        let mut record = read_record(&captured_owner, id)?;
        if record.bound_scope.is_some() {
            bail!("Input already belongs to an admitted conversation");
        }
        let bytes = read_private(&root(&captured_owner)?.join(format!("{id}.bin")), MAX_BYTES)?;
        if bytes.len() != record.size
            || !record
                .sha256
                .eq_ignore_ascii_case(&format!("{:x}", Sha256::digest(&bytes)))
        {
            bail!("Private retry bytes changed; do not reuse this upload ID");
        }
        record.attempted = true;
        write_record(&record)?;
        Ok::<_, anyhow::Error>((record, bytes))
    })
    .await
    .map_err(|_| "Cannot prepare private input upload".to_string())?
    .map_err(|error| error.to_string())?;
    let _ = app.emit(
        "input-file-state",
        UploadState {
            upload_id: upload_id.clone(),
            phase: "uploading-and-validating",
        },
    );
    let uploaded = api::upload_input_bytes(&owner, &record.name, bytes, id).await;
    let remote = match uploaded {
        Ok(remote) => remote,
        Err(error) => {
            let _ = app.emit(
                "input-file-state",
                UploadState {
                    upload_id,
                    phase: "unconfirmed",
                },
            );
            return Err(error.to_string());
        }
    };
    record.remote = Some(remote.clone());
    tokio::task::spawn_blocking(move || write_record(&record))
        .await
        .map_err(|_| "Upload outcome is unconfirmed; retry the same input".to_string())?
        .map_err(|_| "Cannot persist upload acknowledgement; retry the same input".to_string())?;
    check_owner(&owner).map_err(|error| error.to_string())?;
    let _ = app.emit(
        "input-file-state",
        UploadState {
            upload_id,
            phase: "ready",
        },
    );
    Ok(remote)
}

#[tauri::command]
pub async fn input_file_remove(
    owner: String,
    upload_id: String,
) -> Result<serde_json::Value, String> {
    let _guard = STAGING.lock().await;
    check_owner(&owner).map_err(|error| error.to_string())?;
    let id = uuid(&upload_id).map_err(|error| error.to_string())?;
    let record = read_record(&owner, id).map_err(|error| error.to_string())?;
    if record.bound_scope.is_some() {
        return Err("Accepted inputs are removed with their conversation".into());
    }
    let status = if let Some(remote) = &record.remote {
        api::remove_input_file(&owner, uuid(&remote.id).map_err(|error| error.to_string())?)
            .await
            .map_err(|error| error.to_string())?;
        "delete_pending"
    } else if record.attempted {
        "unconfirmed-reservation-may-remain"
    } else {
        "local-only"
    };
    let directory = root(&owner).map_err(|error| error.to_string())?;
    // Delete only copies for this validated opaque ID, never selected originals.
    // A process killed during the atomic byte write can leave its named temp
    // copy. Remove those owned copies before dropping the counted reservation.
    let mut copies = vec![directory.join(format!("{id}.bin"))];
    for entry in std::fs::read_dir(&directory)
        .map_err(|_| "Cannot inspect private input retry data".to_string())?
    {
        let entry = entry.map_err(|_| "Cannot inspect private input retry data".to_string())?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&format!(".{id}.")))
        {
            copies.push(entry.path());
        }
    }
    copies.push(directory.join(format!("{id}.json")));
    for path in copies {
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err("Cannot remove private input draft; retry removal".into());
            }
        }
    }
    check_owner(&owner).map_err(|error| error.to_string())?;
    Ok(serde_json::json!({ "status": status }))
}

#[tauri::command]
pub async fn input_file_original_save_as(
    app: AppHandle,
    owner: String,
    scope_id: String,
    file_id: String,
) -> Result<Option<String>, String> {
    check_owner(&owner).map_err(|error| error.to_string())?;
    let scope = uuid(&scope_id).map_err(|error| error.to_string())?;
    let id = uuid(&file_id).map_err(|error| error.to_string())?;
    let (file, bytes) = api::native_input_original(&owner, scope, id)
        .await
        .map_err(|error| error.to_string())?;
    check_owner(&owner).map_err(|error| error.to_string())?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(file.name)
        .save_file(move |destination| {
            let _ = sender.send(destination);
        });
    let Some(destination) = receiver
        .await
        .map_err(|_| "Save dialog was interrupted".to_string())?
    else {
        return Ok(None);
    };
    let destination = destination
        .into_path()
        .map_err(|_| "Invalid save destination".to_string())?;
    tokio::task::spawn_blocking(move || {
        check_owner(&owner)?;
        let parent = destination
            .parent()
            .context("Save destination has no parent")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&destination)
            .map_err(|_| anyhow::anyhow!("Cannot save original input"))?;
        Ok::<_, anyhow::Error>(Some(destination.to_string_lossy().to_string()))
    })
    .await
    .map_err(|_| "Original save was interrupted".to_string())?
    .map_err(|error| error.to_string())
}
