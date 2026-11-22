use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde::Serialize;

const SKILL_NAME: &str = "socai-social-research";
const SKILL_MD: &str =
    include_str!("../../plugins/socai-social-research/skills/socai-social-research/SKILL.md");
const COMMANDS_MD: &str = include_str!(
    "../../plugins/socai-social-research/skills/socai-social-research/references/commands.md"
);
const CONNECTION_TROUBLESHOOTING_MD: &str = include_str!(
    "../../plugins/socai-social-research/skills/socai-social-research/references/connection-troubleshooting.md"
);

#[derive(Clone, Copy)]
enum Scope {
    User,
    Project,
}

#[derive(Serialize)]
struct IntegrationStatus {
    agent: &'static str,
    path: String,
    installed: bool,
    up_to_date: bool,
    would_install: bool,
}

struct PreparedInstall {
    target: PathBuf,
    staged: PathBuf,
}

struct CommittedInstall {
    target: PathBuf,
    backup: Option<PathBuf>,
    activated: bool,
}

pub fn command() -> Command {
    Command::new("integrate")
        .visible_alias("agent")
        .about("Install or inspect socai discovery for coding agents.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("install")
                .about("Install the socai social-research Skill for an agent.")
                .arg(
                    Arg::new("agent")
                        .required(true)
                        .value_parser([
                            "codex",
                            "claude",
                            "claude-code",
                            "cursor",
                            "gemini",
                            "gemini-cli",
                            "kimi",
                            "kimi-code",
                            "qwen",
                            "qwen-code",
                            "trae",
                            "trae-code",
                            "codebuddy",
                            "opencode",
                            "github-copilot",
                            "copilot",
                            "agents",
                            "all",
                        ])
                        .help("Agent host to configure."),
                )
                .arg(
                    Arg::new("scope")
                        .long("scope")
                        .default_value("user")
                        .value_parser(["user", "project"])
                        .help("Install for the current user or current project."),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Replace an existing socai Skill whose contents differ."),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help("Print target paths without writing files."),
                )
                .arg(json_arg()),
        )
        .subcommand(
            Command::new("status")
                .about("Inspect installed socai agent Skills.")
                .arg(
                    Arg::new("scope")
                        .long("scope")
                        .default_value("user")
                        .value_parser(["user", "project"])
                        .help("Inspect the current user or current project."),
                )
                .arg(json_arg()),
        )
}

fn json_arg() -> Arg {
    Arg::new("json")
        .long("json")
        .action(ArgAction::SetTrue)
        .help("Print machine-readable JSON.")
}

pub fn run(matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("install", sub)) => install(sub),
        Some(("status", sub)) => status(sub),
        _ => bail!("missing integrate subcommand"),
    }
}

fn install(matches: &ArgMatches) -> Result<()> {
    let scope = parse_scope(matches)?;
    let requested = matches
        .get_one::<String>("agent")
        .context("missing agent")?;
    let targets = target_paths(scope, requested)?;
    let force = matches.get_flag("force");
    let dry_run = matches.get_flag("dry-run");

    for (_, path) in &targets {
        validate_target(path, force)?;
    }

    if !dry_run {
        install_targets(&targets, force)?;
    }

    let statuses = targets
        .into_iter()
        .map(|(agent, path)| {
            let installed = managed_entry_exists(&path.join("SKILL.md"));
            let up_to_date = installed && target_is_current(&path);
            IntegrationStatus {
                agent,
                path: path.display().to_string(),
                installed,
                up_to_date,
                would_install: dry_run && !up_to_date,
            }
        })
        .collect::<Vec<_>>();
    print_statuses(&statuses, matches.get_flag("json"))
}

fn status(matches: &ArgMatches) -> Result<()> {
    let statuses = all_target_paths(parse_scope(matches)?)?
        .into_iter()
        .map(|(agent, path)| {
            let installed = managed_entry_exists(&path.join("SKILL.md"));
            let up_to_date = installed && target_is_current(&path);
            IntegrationStatus {
                agent,
                path: path.display().to_string(),
                installed,
                up_to_date,
                would_install: false,
            }
        })
        .collect::<Vec<_>>();
    print_statuses(&statuses, matches.get_flag("json"))
}

fn parse_scope(matches: &ArgMatches) -> Result<Scope> {
    match matches
        .get_one::<String>("scope")
        .map(String::as_str)
        .unwrap_or("user")
    {
        "user" => Ok(Scope::User),
        "project" => Ok(Scope::Project),
        value => bail!("unsupported integration scope: {value}"),
    }
}

fn target_paths(scope: Scope, requested: &str) -> Result<Vec<(&'static str, PathBuf)>> {
    let targets = all_target_paths(scope)?;
    if requested == "all" {
        return Ok(targets);
    }
    let requested = match requested {
        "claude" => "claude-code",
        "gemini" => "gemini-cli",
        "kimi" => "kimi-code",
        "qwen" => "qwen-code",
        "trae" => "trae-code",
        "copilot" => "github-copilot",
        value => value,
    };
    Ok(targets
        .into_iter()
        .filter(|(agent, _)| *agent == requested)
        .collect())
}

fn all_target_paths(scope: Scope) -> Result<Vec<(&'static str, PathBuf)>> {
    Ok(match scope {
        Scope::User => {
            let home = dirs::home_dir().context("could not determine the user home directory")?;
            let configured_root = |name: &str, fallback: &str| {
                std::env::var_os(name)
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(fallback))
            };
            vec![
                (
                    "codex",
                    configured_root("CODEX_HOME", ".codex")
                        .join("skills")
                        .join(SKILL_NAME),
                ),
                (
                    "claude-code",
                    configured_root("CLAUDE_CONFIG_DIR", ".claude")
                        .join("skills")
                        .join(SKILL_NAME),
                ),
                (
                    "cursor",
                    home.join(".cursor").join("skills").join(SKILL_NAME),
                ),
                (
                    "gemini-cli",
                    home.join(".gemini").join("skills").join(SKILL_NAME),
                ),
                (
                    "kimi-code",
                    configured_root("KIMI_CODE_HOME", ".kimi-code")
                        .join("skills")
                        .join(SKILL_NAME),
                ),
                (
                    "qwen-code",
                    home.join(".qwen").join("skills").join(SKILL_NAME),
                ),
                (
                    "trae-code",
                    home.join(".trae-cn").join("skills").join(SKILL_NAME),
                ),
                (
                    "codebuddy",
                    home.join(".codebuddy").join("skills").join(SKILL_NAME),
                ),
                (
                    "opencode",
                    home.join(".config")
                        .join("opencode")
                        .join("skills")
                        .join(SKILL_NAME),
                ),
                (
                    "github-copilot",
                    home.join(".copilot").join("skills").join(SKILL_NAME),
                ),
                (
                    "agents",
                    home.join(".agents").join("skills").join(SKILL_NAME),
                ),
            ]
        }
        // Codex discovers repository Skills from the open Agent Skills path.
        // Keep the generic `agents` name as an explicit portable-host alias.
        Scope::Project => {
            let base = project_root()?;
            let scoped = |agent_dir: &str| base.join(agent_dir).join("skills").join(SKILL_NAME);
            vec![
                ("codex", scoped(".agents")),
                ("claude-code", scoped(".claude")),
                ("cursor", scoped(".cursor")),
                ("gemini-cli", scoped(".gemini")),
                ("kimi-code", scoped(".kimi-code")),
                ("qwen-code", scoped(".qwen")),
                ("trae-code", scoped(".trae")),
                ("codebuddy", scoped(".codebuddy")),
                ("opencode", scoped(".opencode")),
                ("github-copilot", scoped(".github")),
                ("agents", scoped(".agents")),
            ]
        }
    })
}

fn validate_target(path: &Path, force: bool) -> Result<()> {
    reject_symlink_components(path)?;
    if !path.exists() || target_is_current(path) || force {
        return Ok(());
    }
    bail!(
        "{} already exists with different contents; inspect it, then retry with --force to replace only the socai Skill files",
        path.display()
    )
}

fn install_targets(targets: &[(&'static str, PathBuf)], force: bool) -> Result<()> {
    let mut seen = HashSet::new();
    let unique_targets = targets
        .iter()
        .map(|(_, path)| path)
        .filter(|path| seen.insert((*path).clone()))
        .filter(|path| !target_is_current(path))
        .cloned()
        .collect::<Vec<_>>();
    if unique_targets.is_empty() {
        return Ok(());
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos();
    let mut prepared = Vec::with_capacity(unique_targets.len());
    for (index, target) in unique_targets.iter().enumerate() {
        match prepare_install(target, nonce, index) {
            Ok(value) => prepared.push(value),
            Err(err) => {
                cleanup_staged(&prepared);
                return Err(err);
            }
        }
    }

    let mut committed = Vec::with_capacity(prepared.len());
    for (index, item) in prepared.iter().enumerate() {
        if let Err(err) = reject_symlink_components(&item.target) {
            cleanup_staged(&prepared[index..]);
            return Err(with_rollback(err, &committed));
        }
        let backup = match prepare_backup(&item.target, nonce, index) {
            Ok(value) => value,
            Err(err) => {
                cleanup_staged(&prepared[index..]);
                return Err(with_rollback(err, &committed));
            }
        };

        committed.push(CommittedInstall {
            target: item.target.clone(),
            backup,
            activated: false,
        });
        if !force
            && committed
                .last()
                .and_then(|entry| entry.backup.as_deref())
                .is_some_and(|backup| !target_is_current(backup))
        {
            cleanup_staged(&prepared[index..]);
            return Err(with_rollback(
                anyhow!(
                    "{} changed while the integration was being prepared; retry without --force after inspecting it",
                    item.target.display()
                ),
                &committed,
            ));
        }

        if let Err(err) = fs::rename(&item.staged, &item.target) {
            cleanup_staged(&prepared[index..]);
            let activation_error = Err::<(), _>(err)
                .with_context(|| {
                    format!(
                        "could not activate integration at {}",
                        item.target.display()
                    )
                })
                .unwrap_err();
            return Err(with_rollback(activation_error, &committed));
        }
        if let Some(entry) = committed.last_mut() {
            entry.activated = true;
        }
    }

    for item in committed {
        if let Some(backup) = item.backup {
            remove_path(&backup).with_context(|| {
                format!("could not remove integration backup {}", backup.display())
            })?;
        }
    }
    Ok(())
}

fn project_root() -> Result<PathBuf> {
    let current = std::env::current_dir().context("could not determine current directory")?;
    Ok(current
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .unwrap_or(&current)
        .to_path_buf())
}

fn prepare_backup(target: &Path, nonce: u128, index: usize) -> Result<Option<PathBuf>> {
    if !path_exists(target)? {
        return Ok(None);
    }
    let backup = sibling_path(target, "backup", nonce, index)?;
    if path_exists(&backup)? {
        bail!("integration backup already exists: {}", backup.display());
    }
    fs::rename(target, &backup).with_context(|| {
        format!(
            "could not stage the existing integration at {}",
            target.display()
        )
    })?;
    Ok(Some(backup))
}

fn prepare_install(target: &Path, nonce: u128, index: usize) -> Result<PreparedInstall> {
    let parent = target
        .parent()
        .with_context(|| format!("integration path has no parent: {}", target.display()))?;
    reject_symlink_components(parent)?;
    fs::create_dir_all(parent)
        .with_context(|| format!("could not create integration parent {}", parent.display()))?;
    reject_symlink_components(parent)?;

    let staged = sibling_path(target, "staged", nonce, index)?;
    fs::create_dir(&staged)
        .with_context(|| format!("could not create staging directory {}", staged.display()))?;
    let result = (|| -> Result<()> {
        let references = staged.join("references");
        fs::create_dir(&references)
            .with_context(|| format!("could not create {}", references.display()))?;
        write_new(&staged.join("SKILL.md"), SKILL_MD)?;
        write_new(&references.join("commands.md"), COMMANDS_MD)?;
        write_new(
            &references.join("connection-troubleshooting.md"),
            CONNECTION_TROUBLESHOOTING_MD,
        )?;
        if !target_is_current(&staged) {
            bail!(
                "staged integration verification failed for {}",
                target.display()
            );
        }
        Ok(())
    })();
    if let Err(err) = result {
        let _ = remove_path(&staged);
        return Err(err);
    }
    Ok(PreparedInstall {
        target: target.to_path_buf(),
        staged,
    })
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("could not create {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("could not write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("could not flush {}", path.display()))
}

fn sibling_path(target: &Path, kind: &str, nonce: u128, index: usize) -> Result<PathBuf> {
    let parent = target
        .parent()
        .with_context(|| format!("integration path has no parent: {}", target.display()))?;
    Ok(parent.join(format!(
        ".{SKILL_NAME}.{kind}-{}-{nonce}-{index}",
        std::process::id()
    )))
}

fn cleanup_staged(items: &[PreparedInstall]) {
    for item in items {
        let _ = remove_path(&item.staged);
    }
}

fn rollback_committed(items: &[CommittedInstall]) -> Result<()> {
    let mut failures = Vec::new();
    for item in items.iter().rev() {
        if item.activated {
            if let Err(err) = remove_path(&item.target) {
                failures.push(format!("remove {}: {err:#}", item.target.display()));
                continue;
            }
        } else if path_exists(&item.target).unwrap_or(true) {
            failures.push(format!(
                "restore {}: target was recreated concurrently; backup was preserved",
                item.target.display()
            ));
            continue;
        }
        if let Some(backup) = &item.backup {
            if let Err(err) = fs::rename(backup, &item.target) {
                failures.push(format!(
                    "restore {} from {}: {err}",
                    item.target.display(),
                    backup.display()
                ));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(failures.join("; "))
    }
}

fn with_rollback(primary: anyhow::Error, items: &[CommittedInstall]) -> anyhow::Error {
    match rollback_committed(items) {
        Ok(()) => primary,
        Err(recovery) => anyhow!("{primary:#}; rollback failed: {recovery:#}"),
    }
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    let configured_root = ["CODEX_HOME", "CLAUDE_CONFIG_DIR", "KIMI_CODE_HOME"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .find(|root| path.starts_with(root));
    let has_named_integration_root = configured_root.is_none()
        && path
            .components()
            .any(|component| is_integration_root_component(component.as_os_str().to_str()));
    let mut current = PathBuf::new();
    let mut inside_integration_root = configured_root.is_none() && !has_named_integration_root;
    for component in path.components() {
        let component = component.as_os_str();
        current.push(component);
        if configured_root
            .as_ref()
            .is_some_and(|root| current == *root)
            || is_integration_root_component(component.to_str())
        {
            inside_integration_root = true;
        }
        if !inside_integration_root {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!(
                    "refusing integration path containing a symbolic link: {}",
                    current.display()
                );
            }
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("could not inspect {}", current.display()));
            }
        }
    }
    Ok(())
}

fn is_integration_root_component(component: Option<&str>) -> bool {
    matches!(
        component,
        Some(
            ".agents"
                | ".claude"
                | ".codex"
                | ".config"
                | ".cursor"
                | ".gemini"
                | ".kimi-code"
                | ".qwen"
                | ".trae-cn"
                | ".trae"
                | ".codebuddy"
                | ".opencode"
                | ".copilot"
                | ".github"
        )
    )
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("could not inspect {}", path.display())),
    }
}

fn remove_path(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| format!("could not inspect {}", path.display()));
        }
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .with_context(|| format!("could not remove {}", path.display()))
}

fn target_is_current(path: &Path) -> bool {
    let skill = path.join("SKILL.md");
    let commands = path.join("references/commands.md");
    let troubleshooting = path.join("references/connection-troubleshooting.md");
    reject_symlink_components(&skill).is_ok()
        && reject_symlink_components(&commands).is_ok()
        && reject_symlink_components(&troubleshooting).is_ok()
        && regular_file_matches(&skill, SKILL_MD)
        && regular_file_matches(&commands, COMMANDS_MD)
        && regular_file_matches(&troubleshooting, CONNECTION_TROUBLESHOOTING_MD)
}

fn regular_file_matches(path: &Path, expected: &str) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_file() && fs::read_to_string(path).ok().as_deref() == Some(expected)
}

fn managed_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn print_statuses(statuses: &[IntegrationStatus], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(statuses)?);
        return Ok(());
    }
    for status in statuses {
        let state = if status.would_install {
            "would install"
        } else if status.up_to_date {
            "ready"
        } else if status.installed {
            "outdated"
        } else {
            "not installed"
        };
        println!("{}: {} ({state})", status.agent, status.path);
    }
    Ok(())
}
