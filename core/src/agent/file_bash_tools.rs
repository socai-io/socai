//! Local environment tools for an interactive agent: a structured `read_file`
//! (the one thing a shell can't do well — feed an image into the model's
//! vision) plus a general `shell` escape hatch for everything else (write,
//! list, grep, mkdir, …).
//!
//! TUI and desktop both use prompt-enforced scope rather than a filesystem
//! boundary: the tools carry the same local privileges as the socai process,
//! while their descriptions tell the agent to touch only files relevant to
//! the user's request. `ReadFileTool::scoped_to` and `ShellTool::scoped_to`
//! remain available to embedders that explicitly want a lexical path boundary,
//! but socai's interactive entrypoints use their unrestricted constructors.
//!
//! Paths are resolved against the current run directory when relative; a
//! leading `~` expands to the home directory. `shell` also runs with its
//! working directory set to the current run dir.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::{SharedTool, Tool, ToolContext, ToolResult, ToolResultBlock};

/// `~/.socai` (or `$SOCAI_HOME`), socai's application-data root.
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

fn within_roots(path: &Path, roots: &[PathBuf], cwd: &Path) -> bool {
    let resolved = absolutize(path, cwd);
    roots
        .iter()
        .any(|root| path_starts_with(&resolved, &normalize_lexical(root)))
}

#[cfg(not(windows))]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

// Windows paths are case-insensitive in normal use, while Path::starts_with
// compares components case-sensitively. Match the filesystem here so a user
// configuring `e:\\socai_data` does not make an `E:\\socai_data` artifact look
// like an escape (or vice versa).
#[cfg(windows)]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    root.components().all(|root_component| {
        path_components.next().is_some_and(|path_component| {
            path_component
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&root_component.as_os_str().to_string_lossy())
        })
    })
}

/// Best-effort scan for path-like tokens in a shell command line. Not a
/// parser — quoting, variable expansion, and encoded payloads can defeat it —
/// but it catches the straightforward `cat /etc/passwd` / `rm -rf ~/Documents`
/// style asks a prompt-injected note might make. Tokens that are nothing but
/// slashes (`/`, `//`) are skipped: in practice they are division operators in
/// an inline python/awk snippet, not references to the filesystem root.
fn command_escapes_roots(command: &str, roots: &[PathBuf], cwd: &Path) -> Option<String> {
    for raw_token in command.split_whitespace() {
        let token = raw_token.trim_matches(|c| c == '"' || c == '\'' || c == ';' || c == ',');
        if !looks_like_path(token) {
            continue;
        }
        let expanded = if let Some(rest) = token
            .strip_prefix("~/")
            .or_else(|| token.strip_prefix("~\\"))
        {
            dirs::home_dir()
                .map(|home| home.join(rest))
                .unwrap_or_else(|| PathBuf::from(token))
        } else if token == "~" {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from(token))
        } else {
            PathBuf::from(token)
        };
        if !within_roots(&expanded, roots, cwd) {
            return Some(token.to_string());
        }
    }
    None
}

#[cfg(not(windows))]
fn looks_like_path(token: &str) -> bool {
    (token.starts_with('/') && token.chars().any(|c| c != '/'))
        || token.starts_with('~')
        || token.starts_with("./")
        || token.starts_with("../")
}

#[cfg(windows)]
fn looks_like_path(token: &str) -> bool {
    let bytes = token.as_bytes();
    let drive_path = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    // Native Windows utilities use `/b`, `/c:pattern`, etc. as switches.
    // Treat the leading segment as a switch even when its payload contains
    // backslashes; paths such as `/etc/file` still contain a separator in the
    // leading segment and remain subject to the boundary check.
    let slash_switch = token.strip_prefix('/').is_some_and(|rest| {
        let head = rest.split(':').next().unwrap_or(rest);
        !head.is_empty()
            && head
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '?')
    });
    let slash_rooted_path = token.starts_with('/') && !slash_switch;

    drive_path
        || token.starts_with("\\\\")
        || token.starts_with('\\')
        || slash_rooted_path
        || token.starts_with('~')
        || token.starts_with(".\\")
        || token.starts_with("..\\")
        || token.starts_with("./")
        || token.starts_with("../")
}

fn roots_display(roots: &[PathBuf]) -> String {
    roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Skip inlining image bytes larger than this (base64 of big images bloats the
/// context for little gain). The agent still gets the path to cite.
const MAX_INLINE_IMAGE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_READ_LIMIT: usize = 2000;
const SHELL_OUTPUT_LIMIT: usize = 16_000;
const SHELL_DEFAULT_TIMEOUT_MS: u64 = 120_000;

fn resolve_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
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
/// A non-empty `roots` confines it to those directory trees (see module doc).
pub struct ReadFileTool {
    roots: Vec<PathBuf>,
}

impl ReadFileTool {
    pub fn unrestricted() -> Self {
        Self { roots: Vec::new() }
    }

    pub fn scoped_to(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        if !self.roots.is_empty() {
            "Read a local file under socai's data directories (run artifacts, \
             session records — paths outside them are rejected). Text files \
             return their contents (optionally a line window via \
             `offset`/`limit`); image files (png/jpg/webp/gif) are returned as \
             an image you can actually see — use this to inspect screenshot or \
             note-media artifacts from earlier run dirs. For plain text you may \
             also just use `shell`, but images must go through this \
             tool."
        } else {
            "Read a local file. Text files return their contents (optionally a line \
             window via `offset`/`limit`); image files (png/jpg/webp/gif) are \
             returned as an image you can actually see — use this to inspect \
             screenshot artifacts from earlier run dirs. For plain text you may \
             also just use `shell`, but images must go through this tool."
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

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let Some(raw) = input.get("path").and_then(Value::as_str) else {
            anyhow::bail!("read_file requires a `path`");
        };
        let cwd = if ctx.run_dir.is_dir() {
            ctx.run_dir.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };
        let path = absolutize(&resolve_path(raw), &cwd);
        if !self.roots.is_empty() {
            if !within_roots(&path, &self.roots, &cwd) {
                anyhow::bail!(
                    "{} is outside {} — this tool is confined to those directories",
                    path.display(),
                    roots_display(&self.roots)
                );
            }
        }
        let meta = std::fs::metadata(&path)
            .map_err(|e| anyhow::anyhow!("cannot stat {}: {e}", path.display()))?;
        if meta.is_dir() {
            anyhow::bail!("{} is a directory; use shell to list it", path.display());
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

/// `shell` — run a platform-native shell command. The flexible escape hatch for writing files,
/// listing/grepping artifacts, etc. Working directory is the current run dir.
/// A non-empty `roots` confines path-like arguments to those directory trees
/// (see module doc — a best-effort static check, not a sandbox).
pub struct ShellTool {
    roots: Vec<PathBuf>,
}

impl ShellTool {
    pub fn unrestricted() -> Self {
        Self { roots: Vec::new() }
    }

    pub fn scoped_to(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }
}

/// Compact host context injected into the system prompt whenever `shell` is
/// available. The model sees this before its first tool call, so it can choose
/// the platform's real shell contract instead of guessing from a generic
/// `shell` tool name. Runtime discovery is process-free and cached.
pub(crate) fn shell_runtime_prompt() -> String {
    let runtime = crate::util::machine::runtime_platform_info();
    let os_name = match runtime.os {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    let os = if runtime.os_version.trim().is_empty() {
        os_name.to_string()
    } else {
        format!("{os_name} {}", runtime.os_version.trim())
    };
    let mut facts = vec![format!("os={os}"), format!("arch={}", runtime.arch)];
    if !runtime.os_kernel_version.trim().is_empty()
        && runtime.os_kernel_version.trim() != runtime.os_version.trim()
    {
        facts.push(format!("kernel={}", runtime.os_kernel_version.trim()));
    }
    let shell_program = shell_runtime_program();
    facts.push(format!("shell={}", shell_runtime_name()));
    if shell_program.is_absolute() {
        facts.push(format!("shell_path={}", shell_program.display()));
    } else {
        facts.push(format!("shell_command={}", shell_program.display()));
    }

    let guidance = match runtime.os {
        "windows" => "Use PowerShell syntax for `shell` commands. Do not call `bash`, `sh`, or use POSIX-only syntax unless the user explicitly provides such an environment.",
        "macos" => "Use POSIX `sh` syntax for `shell` commands; commands are executed with `sh -c`. macOS ships BSD userland tools, so do not assume GNU-only command flags.",
        "linux" => "Use POSIX `sh` syntax for `shell` commands; commands are executed with `sh -c`. Do not assume Bash-only syntax unless `bash` is explicitly invoked and available.",
        _ => "Use the reported shell and its native syntax for `shell` commands. Do not assume shell-specific extensions unless their interpreter is explicitly invoked and available.",
    };

    format!("Runtime environment: {}. {guidance}", facts.join("; "))
}

#[cfg(not(windows))]
fn shell_runtime_name() -> &'static str {
    "POSIX sh"
}

#[cfg(windows)]
fn shell_runtime_name() -> &'static str {
    let executable = shell_runtime_program()
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("powershell");
    if executable.eq_ignore_ascii_case("pwsh") {
        "PowerShell (pwsh)"
    } else {
        "Windows PowerShell (powershell.exe)"
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        #[cfg(windows)]
        if !self.roots.is_empty() {
            "Run a PowerShell command and return its stdout/stderr and \
             exit code. Working directory is the current run dir. Confined to \
             socai's data directories — commands referencing paths outside \
             them are rejected. Use this to write output files (e.g. \
             Set-Content), list/search run artifacts, create directories, etc. within \
             those directories."
        } else {
            "Run a PowerShell command and return its stdout/stderr and exit \
             code. Working directory is the current run dir, so relative paths land \
             there; use absolute paths for other user-requested locations. Use this \
             to write output files, list/search artifacts, create directories, etc. \
             Scope: stay within files relevant to the user's task — do not run \
             destructive, networked, or system-wide commands."
        }

        #[cfg(not(windows))]
        if !self.roots.is_empty() {
            "Run a POSIX shell command via `sh -c` and return its stdout/stderr and \
             exit code. Working directory is the current run dir. Confined to \
             socai's data directories — commands referencing paths outside \
             them are rejected. Use this to write output files (e.g. \
             printf/tee), list and grep run artifacts, mkdir, etc. within \
             those directories."
        } else {
            "Run a POSIX shell command via `sh -c` and return its stdout/stderr and exit \
             code. Working directory is the current run dir, so relative paths land \
             there; use absolute paths for other user-requested locations. Use this \
             to write output files (e.g. printf/tee), list and grep artifacts, mkdir, \
             etc. Scope: stay within files relevant to the user's task — do not run \
             destructive, networked, or system-wide commands."
        }
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Platform-native shell command: PowerShell on Windows, `sh -c` elsewhere." },
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
            .ok_or_else(|| anyhow::anyhow!("shell requires a non-empty `command`"))?;
        let timeout = Duration::from_millis(
            input
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(SHELL_DEFAULT_TIMEOUT_MS),
        );

        let cwd = if ctx.run_dir.is_dir() {
            ctx.run_dir.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };

        if !self.roots.is_empty() {
            if !within_roots(&cwd, &self.roots, &cwd) {
                anyhow::bail!(
                    "run dir {} is outside {} — refusing to run shell there",
                    cwd.display(),
                    roots_display(&self.roots)
                );
            }
            if let Some(escaped) = command_escapes_roots(command, &self.roots, &cwd) {
                anyhow::bail!(
                    "command references `{escaped}`, which is outside {} — this \
                     tool is confined to those directories. Ask the user to run \
                     anything outside them themselves.",
                    roots_display(&self.roots)
                );
            }
        }

        let mut cmd = shell_command(command);
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
        Ok(ToolResult::text(truncate_output(&body, SHELL_OUTPUT_LIMIT)))
    }
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(shell_runtime_program());
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(windows)]
fn shell_command(command: &str) -> tokio::process::Command {
    let script = format!(
        "$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)\n{command}"
    );
    let mut cmd = tokio::process::Command::new(shell_runtime_program());
    cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
        .arg(script);
    cmd
}

/// Resolve once so the prompt and every command use the same executable.
fn shell_runtime_program() -> &'static Path {
    static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
    PROGRAM.get_or_init(resolve_shell_program).as_path()
}

/// Use the system POSIX shell on macOS and Linux instead of the user's
/// interactive shell (`$SHELL` may be zsh, fish, etc.). `/bin/sh` is the
/// execution contract exposed by this tool; PATH lookup is only a fallback
/// for Unix-like targets whose system shell lives elsewhere.
#[cfg(not(windows))]
fn resolve_shell_program() -> PathBuf {
    for candidate in [PathBuf::from("/bin/sh"), PathBuf::from("/usr/bin/sh")] {
        if candidate.is_file() {
            return candidate;
        }
    }

    executable_on_path("sh").unwrap_or_else(|| PathBuf::from("sh"))
}

/// Prefer the modern `pwsh` executable (PowerShell 7 in its standard install
/// location) for `&&` and predictable UTF-8 behavior, then fall back to the
/// Windows PowerShell installation that ships with the OS. Resolution is
/// filesystem-only so building the first prompt never launches shell code.
#[cfg(windows)]
fn resolve_shell_program() -> PathBuf {
    for root in [
        std::env::var_os("ProgramFiles"),
        std::env::var_os("ProgramW6432"),
    ]
    .into_iter()
    .flatten()
    {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            continue;
        }
        let candidate = root.join("PowerShell").join("7").join("pwsh.exe");
        if candidate.is_file() {
            return candidate;
        }
    }

    if let Some(path) = executable_on_path("pwsh.exe") {
        return path;
    }

    if let Some(system_root) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("WINDIR"))
    {
        let system_root = PathBuf::from(system_root);
        let candidate = system_root
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        if system_root.is_absolute() && candidate.is_file() {
            return candidate;
        }
    }

    if let Some(path) = executable_on_path("powershell.exe") {
        return path;
    }

    PathBuf::from("powershell.exe")
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| {
            if dir.is_absolute() {
                dir.join(name)
            } else {
                cwd.join(dir).join(name)
            }
        })
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.canonicalize().unwrap_or(candidate))
}

/// Backward-compatible Rust API alias. The agent-facing tool is named `shell`.
pub type BashTool = ShellTool;

/// Local tools for the TUI: structured image-capable read + a general `shell`,
/// unrestricted (the user already carries the same privileges in their own
/// terminal). Append to a site tool set.
pub fn local_agent_tools() -> Vec<SharedTool> {
    vec![
        std::sync::Arc::new(ReadFileTool::unrestricted()),
        std::sync::Arc::new(ShellTool::unrestricted()),
    ]
}

/// Local tools for the desktop app. Directory access is intentionally
/// unrestricted, matching the TUI and the user's own local process privileges.
pub fn desktop_agent_tools() -> Vec<SharedTool> {
    local_agent_tools()
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
    async fn shell_runs_and_reports_exit() {
        let ok = ShellTool::unrestricted()
            .call(json!({"command": "echo hi"}), &ctx())
            .await
            .unwrap();
        assert!(ok.flat_text().contains("hi"));

        let fail = ShellTool::unrestricted()
            .call(json!({"command": "exit 3"}), &ctx())
            .await
            .unwrap();
        assert!(fail.flat_text().contains("[exit 3]"));
    }
}
