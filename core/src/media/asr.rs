use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const MODEL_ID: &str = "sherpa-onnx-whisper-small-int8";
const MODEL_BASE_URL: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-small/resolve/8f3c18b358db4d1f2fc1eae49d75cd20989e4309";
const ENCODER_FILE: &str = "small-encoder.int8.onnx";
const ENCODER_SHA256: &str = "4cbe7b22fa9026b843b60a68640c747de05bafb1a11b57edc0e66c232d9f33a9";
const ENCODER_BYTES: u64 = 112_442_483;
const DECODER_FILE: &str = "small-decoder.int8.onnx";
const DECODER_SHA256: &str = "acad50b5c782696e91b55914cc5ab4f756f1532f76e22aa6fc615f39fb69a8ee";
const DECODER_BYTES: u64 = 262_226_114;
const TOKENS_FILE: &str = "small-tokens.txt";
const TOKENS_SHA256: &str = "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126";
const TOKENS_BYTES: u64 = 816_730;
const VAD_FILE: &str = "silero_vad.onnx";
const VAD_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx";
const VAD_SHA256: &str = "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6";
const VAD_BYTES: u64 = 643_854;
const MODEL_TOTAL_BYTES: u64 = ENCODER_BYTES + DECODER_BYTES + TOKENS_BYTES + VAD_BYTES;
const PROTOCOL_VERSION: u32 = 1;

struct AsrModelStatus {
    installed: bool,
    model_dir: String,
    missing_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalAsrStatus {
    pub ready: bool,
    pub state: String,
    pub model_name: String,
    pub model_dir: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
struct InstallProgress {
    installing: bool,
    downloaded_bytes: u64,
    error: Option<String>,
}

#[derive(Clone)]
struct ModelPaths {
    root: PathBuf,
    encoder: PathBuf,
    decoder: PathBuf,
    tokens: PathBuf,
    vad: PathBuf,
}

#[derive(Clone, PartialEq, Eq)]
struct ModelFingerprint {
    root: PathBuf,
    files: Vec<(u64, SystemTime)>,
}

impl ModelPaths {
    fn from_root(root: PathBuf) -> Self {
        Self {
            encoder: root.join(ENCODER_FILE),
            decoder: root.join(DECODER_FILE),
            tokens: root.join(TOKENS_FILE),
            vad: root.join(VAD_FILE),
            root,
        }
    }

    fn invalid_files(&self) -> Result<Vec<String>> {
        let files = [
            (&self.encoder, ENCODER_BYTES, ENCODER_SHA256),
            (&self.decoder, DECODER_BYTES, DECODER_SHA256),
            (&self.tokens, TOKENS_BYTES, TOKENS_SHA256),
            (&self.vad, VAD_BYTES, VAD_SHA256),
        ];
        let mut invalid = Vec::new();
        let mut fingerprint = Vec::with_capacity(files.len());
        for (path, expected_bytes, _) in &files {
            let Ok(metadata) = std::fs::metadata(path) else {
                invalid.push(relative_display(&self.root, path));
                continue;
            };
            if !metadata.is_file() || metadata.len() != *expected_bytes {
                invalid.push(relative_display(&self.root, path));
                continue;
            }
            let Ok(modified) = metadata.modified() else {
                invalid.push(relative_display(&self.root, path));
                continue;
            };
            fingerprint.push((metadata.len(), modified));
        }
        if !invalid.is_empty() {
            return Ok(invalid);
        }

        let fingerprint = ModelFingerprint {
            root: self.root.clone(),
            files: fingerprint,
        };
        let verified = VERIFIED_MODEL.get_or_init(|| std::sync::Mutex::new(None));
        if verified
            .lock()
            .is_ok_and(|cached| cached.as_ref() == Some(&fingerprint))
        {
            return Ok(Vec::new());
        }
        for (path, _, expected_sha256) in files {
            if sha256_file(path)? != expected_sha256 {
                invalid.push(relative_display(&self.root, path));
            }
        }
        if invalid.is_empty() {
            if let Ok(mut cached) = verified.lock() {
                *cached = Some(fingerprint);
            }
        }
        Ok(invalid)
    }
}

#[derive(Serialize)]
struct WorkerRequest<'a> {
    protocol: u32,
    id: u64,
    path: &'a str,
    max_seconds: u64,
}

#[derive(Deserialize)]
struct WorkerResponse {
    protocol: u32,
    id: u64,
    ok: bool,
    transcript: Option<String>,
    error: Option<String>,
}

struct AsrWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

static WORKER: OnceLock<tokio::sync::Mutex<Option<AsrWorker>>> = OnceLock::new();
static INSTALL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
static VERIFIED_MODEL: OnceLock<std::sync::Mutex<Option<ModelFingerprint>>> = OnceLock::new();
static INSTALL_PROGRESS: OnceLock<std::sync::Mutex<InstallProgress>> = OnceLock::new();

fn install_progress() -> InstallProgress {
    INSTALL_PROGRESS
        .get_or_init(|| std::sync::Mutex::new(InstallProgress::default()))
        .lock()
        .map(|progress| progress.clone())
        .unwrap_or_default()
}

fn update_install_progress(installing: bool, downloaded_bytes: u64, error: Option<String>) {
    if let Ok(mut progress) = INSTALL_PROGRESS
        .get_or_init(|| std::sync::Mutex::new(InstallProgress::default()))
        .lock()
    {
        progress.installing = installing;
        progress.downloaded_bytes = downloaded_bytes.min(MODEL_TOTAL_BYTES);
        progress.error = error;
    }
}

pub async fn local_asr_status() -> Result<LocalAsrStatus> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let model = asr_model_status(deadline).await?;
    let progress = install_progress();
    let helper_available = find_asr_helper().is_some();
    let (ready, state, error) = if !helper_available {
        (
            false,
            "helper_missing",
            Some("the bundled local ASR helper is unavailable".to_string()),
        )
    } else if model.installed {
        (true, "ready", None)
    } else if progress.installing {
        (false, "downloading", None)
    } else if let Some(error) = progress.error {
        (false, "error", Some(error))
    } else {
        (false, "model_missing", None)
    };
    Ok(LocalAsrStatus {
        ready,
        state: state.to_string(),
        model_name: "Whisper small".to_string(),
        model_dir: model.model_dir,
        downloaded_bytes: if model.installed {
            MODEL_TOTAL_BYTES
        } else {
            progress.downloaded_bytes
        },
        total_bytes: MODEL_TOTAL_BYTES,
        error,
    })
}

async fn asr_model_status(deadline: Instant) -> Result<AsrModelStatus> {
    model_status_for_paths(model_paths()?, deadline).await
}

async fn model_status_for_paths(paths: ModelPaths, deadline: Instant) -> Result<AsrModelStatus> {
    let model_dir = paths.root.display().to_string();
    let remaining = remaining_before(deadline, "verifying the local ASR model")?;
    let verification = tokio::time::timeout(
        remaining,
        tokio::task::spawn_blocking(move || paths.invalid_files()),
    )
    .await
    .map_err(|_| anyhow!("local Whisper small timed out while verifying model files"))?
    .context("local ASR model verification task failed")?;
    let missing_files = verification?;
    let installed = missing_files.is_empty();
    Ok(AsrModelStatus {
        installed,
        model_dir,
        missing_files,
    })
}

struct InstallFileLock(std::fs::File);

impl Drop for InstallFileLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

async fn acquire_install_lock(path: PathBuf, deadline: Instant) -> Result<InstallFileLock> {
    let remaining = remaining_before(deadline, "waiting for another ASR model installation")?;
    let task = tokio::task::spawn_blocking(move || -> Result<InstallFileLock> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open ASR install lock {}", path.display()))?;
        fs2::FileExt::lock_exclusive(&file)
            .with_context(|| format!("failed to lock ASR install path {}", path.display()))?;
        Ok(InstallFileLock(file))
    });
    tokio::time::timeout(remaining, task)
        .await
        .map_err(|_| anyhow!("local Whisper small timed out waiting for model installation"))?
        .context("ASR install lock task failed")?
}

async fn install_asr_model(deadline: Instant) -> Result<AsrModelStatus> {
    let install = INSTALL.get_or_init(|| tokio::sync::Mutex::new(()));
    let remaining = remaining_before(deadline, "waiting for local ASR model setup")?;
    let _guard = tokio::time::timeout(remaining, install.lock())
        .await
        .map_err(|_| anyhow!("local Whisper small timed out waiting for model setup"))?;
    let paths = model_paths()?;
    let parent = paths
        .root
        .parent()
        .ok_or_else(|| {
            anyhow!(
                "ASR model directory has no parent: {}",
                paths.root.display()
            )
        })?
        .to_path_buf();
    tokio::fs::create_dir_all(&parent).await?;
    let _file_lock =
        acquire_install_lock(parent.join(format!(".{MODEL_ID}.lock")), deadline).await?;
    cleanup_stale_install_dirs(&parent).await?;
    // Another process may have completed the installation while this process
    // waited for the filesystem lock. Verify that winner before replacing it.
    let status = asr_model_status(deadline).await?;
    if status.installed {
        update_install_progress(false, MODEL_TOTAL_BYTES, None);
        return Ok(status);
    }
    update_install_progress(true, 0, None);
    let staging = parent.join(format!(".{MODEL_ID}.install-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&staging).await?;
    let staged = ModelPaths::from_root(staging.clone());

    let install_result = async {
        download_model_file(
            ENCODER_FILE,
            &staged.encoder,
            ENCODER_SHA256,
            ENCODER_BYTES,
            0,
            deadline,
        )
        .await?;
        download_model_file(
            DECODER_FILE,
            &staged.decoder,
            DECODER_SHA256,
            DECODER_BYTES,
            ENCODER_BYTES,
            deadline,
        )
        .await?;
        download_model_file(
            TOKENS_FILE,
            &staged.tokens,
            TOKENS_SHA256,
            TOKENS_BYTES,
            ENCODER_BYTES + DECODER_BYTES,
            deadline,
        )
        .await?;
        download_verified(
            VAD_URL,
            &staged.vad,
            VAD_SHA256,
            Some(VAD_BYTES),
            ENCODER_BYTES + DECODER_BYTES + TOKENS_BYTES,
            deadline,
        )
        .await?;

        let staged_status = model_status_for_paths(staged.clone(), deadline).await?;
        if !staged_status.installed {
            anyhow::bail!(
                "downloaded Whisper small model is incomplete or corrupt: {}",
                staged_status.missing_files.join(", ")
            );
        }
        if paths.root.exists() {
            tokio::fs::remove_dir_all(&paths.root)
                .await
                .with_context(|| {
                    format!(
                        "failed to replace incomplete ASR model at {}",
                        paths.root.display()
                    )
                })?;
        }
        tokio::fs::rename(&staging, &paths.root)
            .await
            .with_context(|| format!("failed to install ASR model at {}", paths.root.display()))?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if install_result.is_err() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }
    install_result?;

    let status = asr_model_status(deadline).await?;
    if !status.installed {
        anyhow::bail!(
            "ASR model install completed with missing or invalid files: {}",
            status.missing_files.join(", ")
        );
    }
    update_install_progress(false, MODEL_TOTAL_BYTES, None);
    Ok(status)
}

async fn cleanup_stale_install_dirs(parent: &Path) -> Result<()> {
    let prefix = format!(".{MODEL_ID}.install-");
    let mut entries = tokio::fs::read_dir(parent).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(&prefix) || !entry.file_type().await?.is_dir() {
            continue;
        }
        tokio::fs::remove_dir_all(entry.path())
            .await
            .with_context(|| {
                format!(
                    "failed to remove stale ASR installation {}",
                    entry.path().display()
                )
            })?;
    }
    Ok(())
}

async fn download_model_file(
    filename: &str,
    target: &Path,
    sha256: &str,
    bytes: u64,
    completed_bytes: u64,
    deadline: Instant,
) -> Result<()> {
    let url = format!("{MODEL_BASE_URL}/{filename}");
    download_verified(&url, target, sha256, Some(bytes), completed_bytes, deadline).await
}

pub async fn transcribe_local_file_with_timeout(
    path: impl AsRef<Path>,
    max_seconds: u64,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .context("local ASR timeout is too large")?;
    transcribe_local_file_inner(path.as_ref(), max_seconds, timeout, deadline).await
}

async fn transcribe_local_file_inner(
    path: &Path,
    max_seconds: u64,
    timeout: Duration,
    deadline: Instant,
) -> Result<String> {
    let helper = find_asr_helper()
        .context("local ASR helper is unavailable; reinstall socai or set SOCAI_ASR_HELPER")?;
    let status = match install_asr_model(deadline).await {
        Ok(status) => status,
        Err(error) => {
            let message = format!("{error:#}");
            update_install_progress(false, install_progress().downloaded_bytes, Some(message));
            return Err(error).with_context(|| {
                "failed to prepare the default local Whisper small model on first use"
            });
        }
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("failed to resolve media path {}", path.display()))?;
    let path_text = path
        .to_str()
        .ok_or_else(|| anyhow!("media path is not valid UTF-8: {}", path.display()))?;

    let worker_slot = WORKER.get_or_init(|| tokio::sync::Mutex::new(None));
    let remaining = remaining_before(deadline, "waiting for the local ASR worker")?;
    let mut slot = tokio::time::timeout(remaining, worker_slot.lock())
        .await
        .map_err(|_| anyhow!("local Whisper small timed out after {}s", timeout.as_secs()))?;
    let stopped = match slot.as_mut() {
        Some(worker) => worker.child.try_wait()?.is_some(),
        None => false,
    };
    if stopped {
        *slot = None;
    }
    if slot.is_none() {
        let remaining = remaining_before(deadline, "starting the local ASR worker")?;
        *slot = Some(
            tokio::time::timeout(remaining, start_worker(&helper, &status.model_dir))
                .await
                .map_err(|_| anyhow!("local Whisper small timed out while starting"))??,
        );
    }
    let remaining = remaining_before(deadline, "transcribing audio with local Whisper small")?;
    // Move the worker out of the shared slot while a request is in flight. If
    // this future is cancelled, dropping the local worker kills the child and
    // leaves the slot empty instead of preserving an unread stale response.
    let mut worker = slot
        .take()
        .context("local ASR worker was not initialized")?;
    let result =
        match tokio::time::timeout(remaining, worker.transcribe(path_text, max_seconds)).await {
            Ok(result) => result,
            Err(_) => Err(anyhow!(
                "local Whisper small timed out after {}s",
                timeout.as_secs()
            )),
        };
    if result.is_ok() {
        *slot = Some(worker);
    }
    result
}

impl AsrWorker {
    async fn transcribe(&mut self, path: &str, max_seconds: u64) -> Result<String> {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        let mut request = serde_json::to_vec(&WorkerRequest {
            protocol: PROTOCOL_VERSION,
            id,
            path,
            max_seconds,
        })?;
        request.push(b'\n');
        self.stdin.write_all(&request).await?;
        self.stdin.flush().await?;
        let line = self
            .stdout
            .next_line()
            .await?
            .context("local ASR helper exited without a response")?;
        let response: WorkerResponse = serde_json::from_str(&line)
            .with_context(|| format!("invalid local ASR helper response: {line}"))?;
        if response.protocol != PROTOCOL_VERSION || response.id != id {
            anyhow::bail!(
                "local ASR protocol mismatch: expected v{PROTOCOL_VERSION} request {id}, got v{} request {}",
                response.protocol,
                response.id
            );
        }
        if response.ok {
            response
                .transcript
                .filter(|text| !text.trim().is_empty())
                .context("local ASR helper returned an empty transcript")
        } else {
            anyhow::bail!(
                "{}",
                response.error.unwrap_or_else(|| "local ASR failed".into())
            )
        }
    }
}

async fn start_worker(helper: &Path, model_dir: &str) -> Result<AsrWorker> {
    let mut child = Command::new(helper)
        .arg("--serve")
        .arg("--model-dir")
        .arg(model_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to start local ASR helper {}", helper.display()))?;
    let stdin = child
        .stdin
        .take()
        .context("ASR helper stdin is unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("ASR helper stdout is unavailable")?;
    Ok(AsrWorker {
        child,
        stdin,
        stdout: BufReader::new(stdout).lines(),
        next_id: 0,
    })
}

fn find_asr_helper() -> Option<PathBuf> {
    let filename = if cfg!(windows) {
        "socai-asr.exe"
    } else {
        "socai-asr"
    };
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("SOCAI_ASR_HELPER") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(dir) = executable.parent() {
            candidates.push(dir.join(filename));
            if dir.file_name().is_some_and(|name| name == "deps") {
                if let Some(profile_dir) = dir.parent() {
                    candidates.push(profile_dir.join(filename));
                }
            }
        }
    }
    if let Some(workspace) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        candidates.push(workspace.join("target").join("debug").join(filename));
        candidates.push(workspace.join("target").join("release").join(filename));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn model_paths() -> Result<ModelPaths> {
    let root = if let Some(path) = std::env::var_os("SOCAI_ASR_MODEL_DIR") {
        PathBuf::from(path)
    } else if let Some(home) = std::env::var_os("SOCAI_HOME") {
        PathBuf::from(home).join("models").join(MODEL_ID)
    } else {
        dirs::home_dir()
            .context("could not resolve home directory for local ASR model")?
            .join(".socai")
            .join("models")
            .join(MODEL_ID)
    };
    Ok(ModelPaths::from_root(root))
}

async fn download_verified(
    url: &str,
    target: &Path,
    expected_sha256: &str,
    expected_size: Option<u64>,
    completed_bytes: u64,
    deadline: Instant,
) -> Result<()> {
    let part = target.with_extension(format!(
        "{}.part",
        target
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("download")
    ));
    if part.exists() {
        tokio::fs::remove_file(&part).await?;
    }
    let remaining = remaining_before(deadline, "downloading the local ASR model")?;
    let response = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15).min(remaining))
        .build()?
        .get(url)
        .timeout(remaining)
        .send()
        .await?
        .error_for_status()?;
    if let (Some(actual), Some(expected)) = (response.content_length(), expected_size) {
        if actual != expected {
            anyhow::bail!(
                "content length mismatch for {url}: expected {expected} bytes, got {actual}"
            );
        }
    }
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&part).await?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    update_install_progress(true, completed_bytes, None);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let next_downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .context("ASR download byte count overflowed")?;
        if expected_size.is_some_and(|limit| next_downloaded > limit) {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            anyhow::bail!(
                "download exceeded pinned size for {url}: expected at most {} bytes",
                expected_size.unwrap_or_default()
            );
        }
        file.write_all(&chunk).await?;
        hasher.update(&chunk);
        downloaded = next_downloaded;
        update_install_progress(true, completed_bytes.saturating_add(downloaded), None);
    }
    file.flush().await?;
    drop(file);
    if expected_size.is_some_and(|expected| downloaded != expected) {
        let _ = tokio::fs::remove_file(&part).await;
        anyhow::bail!(
            "download size mismatch for {url}: expected {} bytes, got {downloaded}",
            expected_size.unwrap_or_default()
        );
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest != expected_sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        anyhow::bail!("checksum mismatch for {url}: expected {expected_sha256}, got {digest}");
    }
    tokio::fs::rename(&part, target).await?;
    Ok(())
}

fn remaining_before(deadline: Instant, operation: &str) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| anyhow!("local Whisper small timed out while {operation}"))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("failed to open ASR model file {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read ASR model file {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
