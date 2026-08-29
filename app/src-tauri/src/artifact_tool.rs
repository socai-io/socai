use async_trait::async_trait;
use cap_std::{ambient_authority, fs::Dir};
use serde_json::{json, Value};
use socai_core::agent::{Tool, ToolContext, ToolResult};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Publish a completed file from this conversation as a desktop download card.
/// The source stays in place; the user-facing copy lives in this turn's
/// `outputs/` directory.
pub(crate) struct PublishArtifactTool {
    conversation_dir: Option<PathBuf>,
}

impl PublishArtifactTool {
    pub(crate) fn new(conversation_dir: Option<PathBuf>) -> Self {
        Self { conversation_dir }
    }
}

#[async_trait]
impl Tool for PublishArtifactTool {
    fn name(&self) -> &str {
        "publish_artifact"
    }

    fn description(&self) -> &str {
        "Publish a completed user-requested file so the desktop app displays a \
         download card. Call this after creating and verifying the file. Pass \
         its path relative to the current run directory or an absolute path \
         inside the current conversation, including an earlier turn. The \
         source must be a regular file. Publishing never overwrites an existing \
         output. Do not tell the user a file is downloadable until this tool \
         succeeds."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Completed file path, relative to the current run directory or absolute within the current conversation."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let raw_path = input
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| anyhow::anyhow!("publish_artifact requires a non-empty `path`"))?;

        let run_root = ctx.run_dir.canonicalize().map_err(|error| {
            anyhow::anyhow!(
                "could not resolve current run directory {}: {error}",
                ctx.run_dir.display()
            )
        })?;
        let requested = resolve_source_path(raw_path, &ctx.run_dir);
        let requested_metadata = std::fs::symlink_metadata(&requested).map_err(|error| {
            anyhow::anyhow!(
                "could not inspect artifact source {}: {error}",
                requested.display()
            )
        })?;
        if requested_metadata.file_type().is_symlink() || !requested_metadata.is_file() {
            anyhow::bail!(
                "artifact source must be a regular file and cannot be a symbolic link: {}",
                requested.display()
            );
        }

        let source = requested.canonicalize().map_err(|error| {
            anyhow::anyhow!(
                "could not resolve artifact source {}: {error}",
                requested.display()
            )
        })?;
        let conversation_root = self
            .conversation_dir
            .as_deref()
            .map(Path::canonicalize)
            .transpose()
            .map_err(|error| {
                anyhow::anyhow!("could not resolve conversation directory: {error}")
            })?;
        let source_is_allowed = source.starts_with(&run_root)
            || conversation_root
                .as_deref()
                .is_some_and(|root| source.starts_with(root));
        if !source_is_allowed {
            anyhow::bail!(
                "artifact source is outside the current conversation: {}",
                requested.display()
            );
        }

        let mut source_file = open_read_only_no_follow(&source).map_err(|error| {
            anyhow::anyhow!(
                "could not open artifact source {}: {error}",
                source.display()
            )
        })?;
        let source_metadata = source_file.metadata().map_err(|error| {
            anyhow::anyhow!(
                "could not inspect open artifact source {}: {error}",
                source.display()
            )
        })?;
        if !source_metadata.is_file() {
            anyhow::bail!(
                "artifact source is no longer a regular file: {}",
                source.display()
            );
        }
        let opened_identity = same_file::Handle::from_file(source_file.try_clone()?)?;
        let current_identity = same_file::Handle::from_path(&source)?;
        if opened_identity != current_identity || requested.canonicalize()? != source {
            anyhow::bail!(
                "artifact source changed while publishing: {}",
                requested.display()
            );
        }

        let (run_dir, outputs, outputs_dir) = prepare_outputs_directory(&run_root)?;
        let (published, published_file) = if source.starts_with(&outputs) {
            let relative = source.strip_prefix(&outputs)?;
            let destination_file = outputs_dir
                .open(relative)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "could not open published artifact {}: {error}",
                        source.display()
                    )
                })?
                .into_std();
            let source_identity = same_file::Handle::from_file(source_file.try_clone()?)?;
            let destination_identity = same_file::Handle::from_file(destination_file.try_clone()?)?;
            if source_identity != destination_identity {
                anyhow::bail!(
                    "artifact source changed while validating its output: {}",
                    source.display()
                );
            }
            validate_published_destination(
                &outputs,
                &outputs_dir,
                relative,
                &source,
                &destination_file,
            )?;
            (source, destination_file)
        } else {
            let file_name = source
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("artifact source has no filename"))?;
            let relative = Path::new(file_name);
            let destination = outputs.join(file_name);
            match outputs_dir.symlink_metadata(file_name) {
                Ok(_) => {
                    anyhow::bail!(
                        "an artifact named `{}` is already published for this turn",
                        file_name.to_string_lossy()
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }

            let mut staged = tempfile::NamedTempFile::new_in(&run_root)?;
            std::io::copy(&mut source_file, staged.as_file_mut()).map_err(|error| {
                anyhow::anyhow!("could not stage artifact {}: {error}", source.display())
            })?;
            staged.as_file_mut().flush()?;
            let staged_name = staged
                .path()
                .strip_prefix(&run_root)
                .map_err(|_| anyhow::anyhow!("staged artifact escaped the current run"))?;
            if staged_name.parent() != Some(Path::new("")) {
                anyhow::bail!("staged artifact is not directly inside the current run");
            }
            validate_directory_handle(&run_root, &run_dir, "current run")?;
            let staged_file = run_dir
                .open(staged_name)
                .map_err(|error| anyhow::anyhow!("could not reopen staged artifact: {error}"))?
                .into_std();
            let staged_identity = same_file::Handle::from_file(staged.as_file().try_clone()?)?;
            let reopened_staged_identity = same_file::Handle::from_file(staged_file.try_clone()?)?;
            if staged_identity != reopened_staged_identity {
                anyhow::bail!("staged artifact changed before publication");
            }

            validate_directory_handle(&outputs, &outputs_dir, "artifact output")?;
            if let Err(error) = run_dir.hard_link(staged_name, &outputs_dir, file_name) {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::bail!(
                        "an artifact named `{}` is already published for this turn",
                        file_name.to_string_lossy()
                    );
                }
                return Err(anyhow::anyhow!(
                    "could not publish artifact to {}: {error}",
                    destination.display()
                ));
            }
            let destination_file = match outputs_dir.open(file_name) {
                Ok(file) => file.into_std(),
                Err(error) => {
                    remove_published_destination(&outputs_dir, relative, &staged_file);
                    return Err(anyhow::anyhow!(
                        "could not reopen published artifact {}: {error}",
                        destination.display()
                    ));
                }
            };
            let destination_identity = same_file::Handle::from_file(destination_file.try_clone()?)?;
            if staged_identity != destination_identity {
                remove_published_destination(&outputs_dir, relative, &staged_file);
                anyhow::bail!(
                    "published artifact changed while writing: {}",
                    destination.display()
                );
            }
            if let Err(error) = validate_published_destination(
                &outputs,
                &outputs_dir,
                relative,
                &destination,
                &destination_file,
            ) {
                remove_published_destination(&outputs_dir, relative, &staged_file);
                return Err(error);
            }
            (destination, destination_file)
        };

        let metadata = published_file.metadata()?;
        let name = published
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow::anyhow!("published artifact has no valid filename"))?;
        let relative_path = ctx.register_artifact(
            &published,
            name,
            "deliverable",
            &format!("User-downloadable file {name}"),
            json!({
                "category": "deliverable",
                "name": name,
                "size_bytes": metadata.len(),
            }),
            None,
            "publish_artifact",
        );

        Ok(ToolResult::text(serde_json::to_string_pretty(&json!({
            "ok": true,
            "artifact": {
                "name": name,
                "path": relative_path,
                "size_bytes": metadata.len(),
            },
            "message": "Artifact published. The desktop app will display a download card."
        }))?))
    }
}

fn resolve_source_path(raw_path: &str, run_dir: &Path) -> PathBuf {
    let path = PathBuf::from(raw_path);
    if path.is_absolute() {
        path
    } else {
        run_dir.join(path)
    }
}

fn prepare_outputs_directory(run_root: &Path) -> anyhow::Result<(Dir, PathBuf, Dir)> {
    let run_dir = Dir::open_ambient_dir(run_root, ambient_authority())?;
    validate_directory_handle(run_root, &run_dir, "current run")?;
    let outputs = run_root.join("outputs");
    match run_dir.symlink_metadata("outputs") {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            anyhow::bail!(
                "artifact output path must be a regular directory: {}",
                outputs.display()
            );
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            run_dir.create_dir("outputs")?;
        }
        Err(error) => return Err(error.into()),
    }
    let outputs_dir = run_dir.open_dir("outputs")?;
    validate_directory_handle(&outputs, &outputs_dir, "artifact output")?;
    Ok((run_dir, outputs, outputs_dir))
}

fn validate_directory_handle(path: &Path, directory: &Dir, label: &str) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    let opened_metadata = directory.metadata(".")?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !opened_metadata.is_dir()
        || path.canonicalize()? != path
    {
        anyhow::bail!(
            "{label} directory changed while publishing: {}",
            path.display()
        );
    }
    let opened_identity = same_file::Handle::from_file(directory.try_clone()?.into_std_file())?;
    let current_identity = same_file::Handle::from_path(path)?;
    if opened_identity != current_identity {
        anyhow::bail!(
            "{label} directory changed while publishing: {}",
            path.display()
        );
    }
    Ok(())
}

fn validate_published_destination(
    outputs: &Path,
    outputs_dir: &Dir,
    relative: &Path,
    destination: &Path,
    destination_file: &std::fs::File,
) -> anyhow::Result<()> {
    validate_directory_handle(outputs, outputs_dir, "artifact output")?;
    let relative_metadata = outputs_dir.symlink_metadata(relative)?;
    let ambient_metadata = std::fs::symlink_metadata(destination)?;
    if relative_metadata.file_type().is_symlink()
        || !relative_metadata.is_file()
        || ambient_metadata.file_type().is_symlink()
        || !ambient_metadata.is_file()
    {
        anyhow::bail!(
            "published artifact path changed after writing: {}",
            destination.display()
        );
    }
    let canonical = destination.canonicalize()?;
    let opened_identity = same_file::Handle::from_file(destination_file.try_clone()?)?;
    let relative_identity = same_file::Handle::from_file(outputs_dir.open(relative)?.into_std())?;
    let current_identity = same_file::Handle::from_path(&canonical)?;
    if !canonical.starts_with(outputs)
        || opened_identity != relative_identity
        || opened_identity != current_identity
    {
        anyhow::bail!(
            "published artifact escaped or changed after writing: {}",
            destination.display()
        );
    }
    Ok(())
}

fn remove_published_destination(outputs_dir: &Dir, relative: &Path, expected_file: &std::fs::File) {
    let Ok(current_file) = outputs_dir.open(relative) else {
        return;
    };
    let Ok(expected_clone) = expected_file.try_clone() else {
        return;
    };
    let Ok(expected_identity) = same_file::Handle::from_file(expected_clone) else {
        return;
    };
    let Ok(current_identity) = same_file::Handle::from_file(current_file.into_std()) else {
        return;
    };
    if expected_identity == current_identity {
        let _ = outputs_dir.remove_file(relative);
    }
}

fn open_read_only_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}
