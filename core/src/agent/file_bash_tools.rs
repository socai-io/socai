//! Local environment tools for an interactive agent: a structured `read_file`
//! (the one thing a shell can't do well — feed an image into the model's
//! vision) plus a general `bash` escape hatch for everything else (write,
//! list, grep, mkdir, …).
//!
//! For the TUI (`local_agent_tools()`), scope is *prompt-enforced*, not
//! sandboxed: the user is running their own terminal and carries the same
//! privileges regardless of whether the agent has a `bash` tool. For the
//! desktop app (`desktop_agent_tools()`), the same content the agent reads —
//! Xiaohongshu note/comment text — is untrusted, adversarial input running on
//! a much wider, less technical install base, so these tools are additionally
//! confined to the `~/.socai` tree: `read_file` rejects any path that doesn't
//! lexically resolve under the root, and `bash` rejects any command whose
//! path-like tokens resolve outside it. This is a real, enforced boundary for
//! path arguments — not a full OS sandbox. It doesn't stop a shell command
//! from doing non-path-based damage (network calls, following a symlink
//! planted inside the root that points back out, etc.); it exists to block
//! the straightforward "read/write/delete something outside ~/.socai" cases a
//! prompt-injected note could ask for.
//!
//! Paths are resolved against the process working directory when relative; a
//! leading `~` expands to the home directory. `bash` runs with its working
//! directory set to the current run dir.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::{SharedTool, Tool, ToolContext, ToolResult, ToolResultBlock};

/// `~/.socai` (or `$SOCAI_HOME`), the root desktop's tools are confined to.
pub fn socai_home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("SOCAI_HOME") {
        return PathBuf::from(home);
    }
    dirs::home_dir()
        .map(|home| home.join(".socai"))
        .unwrap_or_else(|| PathBuf::from(".socai"))
}

/// Collapse `.`/`..` components without requiring the path to exist (unlike
/// `Path::canonicalize`, which needs every component to resolve on disk).
/// Doesn't follow symlinks — a symlink planted inside `root` that points
/// back outside it can still escape; see the module doc for what this
/// boundary does and doesn't cover.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Absolutize + normalize `path`, using `cwd` for relative paths.
fn absolutize(path: &Path, cwd: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    normalize_lexical(&absolute)
}

fn within_root(path: &Path, root: &Path, cwd: &Path) -> bool {
    absolutize(path, cwd).starts_with(normalize_lexical(root))
}

/// Best-effort scan for path-like tokens in a shell command line. Not a
/// parser — quoting, variable expansion, and encoded payloads can defeat it —
/// but it catches the straightforward `cat /etc/passwd` / `rm -rf ~/Documents`
/// style asks a prompt-injected note might make.
fn command_escapes_root(command: &str, root: &Path, cwd: &Path) -> Option<String> {
    for raw_token in command.split_whitespace() {
        let token = raw_token.trim_matches(|c| c == '"' || c == '\'' || c == ';' || c == ',');
        let looks_like_path =
            token.starts_with('/') || token.starts_with('~') || token.starts_with("./") || token.starts_with("../");
        if !looks_like_path {
            continue;
        }
        let expanded = if let Some(rest) = token.strip_prefix("~/") {
            dirs::home_dir()
                .map(|home| home.join(rest))
                .unwrap_or_else(|| PathBuf::from(token))
        } else if token == "~" {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from(token))
        } else {
            PathBuf::from(token)
        };
        if !within_root(&expanded, root, cwd) {
            return Some(token.to_string());
        }
    }
    None
}

/// Skip inlining image bytes larger than this (base64 of big images bloats the
/// context for little gain). The agent still gets the path to cite.
const MAX_INLINE_IMAGE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_READ_LIMIT: usize = 2000;
const BASH_OUTPUT_LIMIT: usize = 16_000;
const BASH_DEFAULT_TIMEOUT_MS: u64 = 120_000;

fn resolve_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if trimmed == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(trimmed)
}

fn image_media_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

fn truncate_output(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}\n…[output truncated at {limit} chars]")
}

/// `read_file` — read a text file (optionally a line window) or an image.
/// `root: Some(dir)` confines it to that directory tree (see module doc).
pub struct ReadFileTool {
    root: Option<PathBuf>,
}

impl ReadFileTool {
    pub fn unrestricted() -> Self {
        Self { root: None }
    }

    pub fn scoped_to(root: PathBuf) -> Self {
        Self { root: Some(root) }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        if self.root.is_some() {
            "Read a local file under ~/.socai (run artifacts, session records — \
             paths outside ~/.socai are rejected). Text files return their \
             contents (optionally a line window via `offset`/`limit`); image \
             files (png/jpg/webp/gif) are returned as an image you can \
             actually see — use this to inspect screenshot artifacts from \
             earlier run dirs. For plain text you may also just use `bash` \
             (cat/sed), but images must go through this tool."
        } else {
            "Read a local file. Text files return their contents (optionally a line \
             window via `offset`/`limit`); image files (png/jpg/webp/gif) are \
             returned as an image you can actually see — use this to inspect \
             screenshot artifacts from earlier run dirs. For plain text you may \
             also just use `bash` (cat/sed), but images must go through this tool."
        }
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (absolute, relative to cwd, or ~/...)." },
                "offset": { "type": "integer", "description": "1-based start line for text files." },
                "limit": { "type": "integer", "description": "Max lines to return for text files." }
            },
            "required": ["path"]
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let Some(raw) = input.get("path").and_then(Value::as_str) else {
            anyhow::bail!("read_file requires a `path`");
        };
        let path = resolve_path(raw);
        if let Some(root) = &self.root {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            if !within_root(&path, root, &cwd) {
                anyhow::bail!(
                    "{} is outside {} — this tool is confined to that directory",
                    path.display(),
                    root.display()
                );
            }
        }
        let meta = std::fs::metadata(&path)
            .map_err(|e| anyhow::anyhow!("cannot stat {}: {e}", path.display()))?;
        if meta.is_dir() {
            anyhow::bail!(
                "{} is a directory; use bash (ls) to list it",
                path.display()
            );
        }

        if let Some(media_type) = image_media_type(&path) {
            if meta.len() > MAX_INLINE_IMAGE_BYTES {
                return Ok(ToolResult::text(format!(
                    "Image {} is {} bytes — too large to inline. Reference it by path.",
                    path.display(),
                    meta.len()
                )));
            }
            let bytes = std::fs::read(&path)?;
            use base64::Engine as _;
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            return Ok(ToolResult::blocks(vec![
                ToolResultBlock::text(format!("Image {}", path.display())),
                ToolResultBlock::Image {
                    data,
                    media_type: media_type.to_string(),
                },
            ]));
        }

        if meta.len() > MAX_TEXT_BYTES {
            anyhow::bail!(
                "{} is {} bytes — too large to read; use offset/limit",
                path.display(),
                meta.len()
            );
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;

        let offset = input
            .get("offset")
            .and_then(Value::as_u64)
            .map(|o| o.max(1) as usize);
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .map(|l| l as usize);

        if offset.is_none() && limit.is_none() {
            let lines: Vec<&str> = content.lines().collect();
            if lines.len() > DEFAULT_READ_LIMIT {
                let shown = lines[..DEFAULT_READ_LIMIT].join("\n");
                return Ok(ToolResult::text(format!(
                    "{shown}\n\n[truncated at {DEFAULT_READ_LIMIT} of {} lines; use offset/limit for more]",
                    lines.len()
                )));
            }
            return Ok(ToolResult::text(content));
        }

        let start = offset.unwrap_or(1).saturating_sub(1);
        let take = limit.unwrap_or(DEFAULT_READ_LIMIT);
        let windowed: Vec<&str> = content.lines().skip(start).take(take).collect();
        Ok(ToolResult::text(windowed.join("\n")))
    }
}

/// `bash` — run a shell command. The flexible escape hatch for writing files,
/// listing/grepping artifacts, etc. Working directory is the current run dir.
/// `root: Some(dir)` confines path-like arguments to that directory tree (see
/// module doc — a best-effort static check, not a sandbox).
pub struct BashTool {
    root: Option<PathBuf>,
}

impl BashTool {
    pub fn unrestricted() -> Self {
        Self { root: None }
    }

    pub fn scoped_to(root: PathBuf) -> Self {
        Self { root: Some(root) }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        if self.root.is_some() {
            "Run a shell command via `sh -c` and return its stdout/stderr and \
             exit code. Working directory is the current run dir under \
             ~/.socai/runs. Confined to ~/.socai — commands referencing paths \
             outside it (other than by the app's own working directory) are \
             rejected. Use this to write output files (e.g. printf/tee), list \
             and grep run artifacts, mkdir, etc. within ~/.socai."
        } else {
            "Run a shell command via `sh -c` and return its stdout/stderr and exit \
             code. Working directory is the current run dir under ~/.socai/runs, so \
             relative paths land there; use absolute paths for other run dirs or \
             the session dir. Use this to write output files (e.g. printf/tee), \
             list and grep artifacts, mkdir, etc. Scope: stay within the files \
             relevant to this task and the user's ~/.socai data — do not run \
             destructive, networked, or system-wide commands."
        }
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to run via `sh -c`." },
                "timeout_ms": { "type": "integer", "description": "Optional timeout in milliseconds (default 120000)." }
            },
            "required": ["command"]
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| anyhow::anyhow!("bash requires a non-empty `command`"))?;
        let timeout = Duration::from_millis(
            input
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(BASH_DEFAULT_TIMEOUT_MS),
        );

        let cwd = if ctx.run_dir.is_dir() {
            ctx.run_dir.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };

        if let Some(root) = &self.root {
            if !within_root(&cwd, root, &cwd) {
                anyhow::bail!(
                    "run dir {} is outside {} — refusing to run bash there",
                    cwd.display(),
                    root.display()
                );
            }
            if let Some(escaped) = command_escapes_root(command, root, &cwd) {
                anyhow::bail!(
                    "command references `{escaped}`, which is outside {} — this \
                     tool is confined to that directory. Ask the user to run \
                     anything outside it themselves.",
                    root.display()
                );
            }
        }

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd.current_dir(&cwd);

        let output = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(result) => result.map_err(|e| anyhow::anyhow!("failed to run command: {e}"))?,
            Err(_) => anyhow::bail!("command timed out after {:?}", timeout),
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut parts = Vec::new();
        if !stdout.trim().is_empty() {
            parts.push(stdout.into_owned());
        }
        if !stderr.trim().is_empty() {
            parts.push(format!("[stderr]\n{stderr}"));
        }
        if !output.status.success() {
            parts.push(format!("[exit {}]", output.status.code().unwrap_or(-1)));
        }
        let body = if parts.is_empty() {
            "(no output)".to_string()
        } else {
            parts.join("\n")
        };
        Ok(ToolResult::text(truncate_output(&body, BASH_OUTPUT_LIMIT)))
    }
}

/// Local tools for the TUI: structured image-capable read + a general `bash`,
/// unrestricted (the user already carries the same privileges in their own
/// terminal). Append to a site tool set.
pub fn local_agent_tools() -> Vec<SharedTool> {
    vec![
        std::sync::Arc::new(ReadFileTool::unrestricted()),
        std::sync::Arc::new(BashTool::unrestricted()),
    ]
}

/// Local tools for the desktop app: the same `read_file`/`bash` pair, confined
/// to `~/.socai` (see module doc) since the desktop agent's primary input is
/// untrusted Xiaohongshu content on a much wider install base than the TUI.
pub fn desktop_agent_tools() -> Vec<SharedTool> {
    let root = socai_home_dir();
    vec![
        std::sync::Arc::new(ReadFileTool::scoped_to(root.clone())),
        std::sync::Arc::new(BashTool::scoped_to(root)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext::new("run", std::env::temp_dir())
    }

    #[tokio::test]
    async fn read_honors_offset_and_limit() {
        let dir = std::env::temp_dir().join(format!("socai_fs_win_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lines.txt");
        std::fs::write(&file, "a\nb\nc\nd").unwrap();

        let read = ReadFileTool::unrestricted()
            .call(
                json!({"path": file.to_string_lossy(), "offset": 2, "limit": 2}),
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(read.flat_text(), "b\nc");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn bash_runs_and_reports_exit() {
        let ok = BashTool::unrestricted()
            .call(json!({"command": "echo hi"}), &ctx())
            .await
            .unwrap();
        assert!(ok.flat_text().contains("hi"));

        let fail = BashTool::unrestricted()
            .call(json!({"command": "exit 3"}), &ctx())
            .await
            .unwrap();
        assert!(fail.flat_text().contains("[exit 3]"));
    }
}
