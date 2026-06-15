mod daemon;
mod tracking;
mod tui;
mod version;

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::Value;
use socai_core::config as socai_config;

#[derive(Parser, Debug)]
#[command(name = "socai")]
#[command(version)]
#[command(about = "socai — XHS-savvy browser agent")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Search Xiaohongshu and print the first results page's note cards as JSON.
    #[command(name = "search_notes")]
    SearchNotes {
        query: String,
        /// Search-result filter as `group=option` (repeatable), e.g.
        /// `--filter publish_time=一天内 --filter note_type=图文`. Groups:
        /// sort, note_type, publish_time, search_scope, distance.
        #[arg(long = "filter", value_name = "GROUP=OPTION")]
        filter: Vec<String>,
        /// Auto-scroll the feed to collect at least this many cards
        /// (titles/likes/covers only — note bodies are not opened). Omit for
        /// the first page only (~19 cards).
        #[arg(long = "num-notes")]
        num_notes: Option<i64>,
        #[arg(long)]
        pretty: bool,
        /// Record DOM + a11y tree + screenshot bundles to <run_dir>/snapshots/
        /// at every page change between tool operations.
        #[arg(long = "debug-snapshot")]
        debug_snapshot: bool,
    },
    /// Run a Xiaohongshu topic scan (note body + top comments per note).
    #[command(name = "topic_scan")]
    TopicScan {
        query: String,
        #[arg(long)]
        tab: Option<String>,
        /// Search-result filter as `group=option` (repeatable), e.g.
        /// `--filter publish_time=一天内 --filter note_type=图文`. Groups:
        /// sort, note_type, publish_time, search_scope, distance.
        #[arg(long = "filter", value_name = "GROUP=OPTION")]
        filter: Vec<String>,
        /// Number of notes to read; scrolls the feed only if the first page
        /// holds fewer.
        #[arg(long = "num-notes")]
        num_notes: Option<i64>,
        /// Download media, add local_path fields, emit media_manifest_path /
        /// media_manifest_count, and write <run_dir>/media_manifest.json.
        #[arg(long = "download-media")]
        download_media: bool,
        #[arg(long)]
        pretty: bool,
        /// Record DOM + a11y tree + screenshot bundles to <run_dir>/snapshots/
        /// at every page change between tool operations.
        #[arg(long = "debug-snapshot")]
        debug_snapshot: bool,
    },
    /// Open a note from the current search/topic page and print the parsed note.
    #[command(name = "extract_note")]
    ExtractNote {
        #[arg(long = "note-id")]
        note_id: String,
        #[arg(long)]
        pretty: bool,
        /// Record DOM + a11y tree + screenshot bundles to <run_dir>/snapshots/
        /// at every page change between tool operations.
        #[arg(long = "debug-snapshot")]
        debug_snapshot: bool,
    },
    /// Print installed version and latest release status.
    Version {
        /// Only print the installed version; do not check GitHub Releases.
        #[arg(long = "no-check")]
        no_check: bool,
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Update the macOS release-binary install to the latest version.
    Update,
    /// Read and write persistent socai preferences.
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommand>,
    },
    /// Stop the background socai rust daemon.
    Stop,
    #[command(name = "__daemon", hide = true)]
    Daemon,
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// Print the config file path.
    Path,
    /// Print the full config, or a single config key.
    Get {
        /// Key to read, e.g. chrome.profile.
        key: Option<String>,
    },
    /// Print the full config as JSON.
    List,
    /// Persist a config key.
    Set {
        /// Key to set, e.g. chrome.profile.
        key: String,
        /// Value to store.
        value: String,
    },
    /// Remove a config key.
    Unset {
        /// Key to remove, e.g. chrome.profile.
        key: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,chromiumoxide=off,hyper=off,reqwest=off".into()),
        )
        .init();

    let args = Args::parse();
    let Some(command) = args.command else {
        tui::run().await?;
        return Ok(());
    };
    if should_warn_for_update(&command) {
        version::maybe_warn_if_outdated().await;
    }

    match command {
        Command::SearchNotes {
            query,
            filter,
            num_notes,
            pretty,
            debug_snapshot,
        } => {
            let mut input = serde_json::json!({ "query": query, "debug_snapshot": debug_snapshot });
            if !filter.is_empty() {
                input["filters"] = Value::Object(parse_filters(&filter)?);
            }
            if let Some(n) = num_notes {
                input["num_notes"] = serde_json::json!(n.max(1));
            }
            // Scrolling for a large `num_notes` can take a while; give it the
            // longer budget rather than the default single-action timeout.
            let timeout = if num_notes.is_some() {
                daemon::LONG_COMMAND_TIMEOUT
            } else {
                daemon::DEFAULT_COMMAND_TIMEOUT
            };
            let result = daemon::send_or_spawn("search_notes", input, timeout).await?;
            print_command_result(&result, pretty)?;
        }
        Command::TopicScan {
            query,
            tab,
            filter,
            num_notes,
            download_media,
            pretty,
            debug_snapshot,
        } => {
            let mut input = serde_json::json!({ "query": query, "debug_snapshot": debug_snapshot });
            if let Some(tab) = tab {
                input["tab_label"] = Value::String(tab);
            }
            if !filter.is_empty() {
                input["filters"] = Value::Object(parse_filters(&filter)?);
            }
            if let Some(n) = num_notes {
                input["num_notes"] = serde_json::json!(n.max(1));
            }
            if download_media {
                input["download_media"] = serde_json::json!(true);
            }

            let result =
                daemon::send_or_spawn("topic_scan", input, daemon::LONG_COMMAND_TIMEOUT).await?;
            print_command_result(&result, pretty)?;
        }
        Command::ExtractNote {
            note_id,
            pretty,
            debug_snapshot,
        } => {
            let input = serde_json::json!({ "note_id": note_id, "debug_snapshot": debug_snapshot });
            let result =
                daemon::send_or_spawn("extract_note", input, daemon::DEFAULT_COMMAND_TIMEOUT)
                    .await?;
            print_command_result(&result, pretty)?;
        }
        Command::Version { no_check, json } => {
            version::print_version_command(no_check, json).await?
        }
        Command::Update => version::run_update_command().await?,
        Command::Config { command } => run_config_command(command.unwrap_or(ConfigCommand::List))?,
        Command::Stop => {
            if daemon::stop_daemon().await? {
                eprintln!("socai rust daemon stopped");
            } else {
                eprintln!("socai rust daemon is not running");
            }
        }
        Command::Daemon => daemon::run_daemon().await?,
    }

    Ok(())
}

fn run_config_command(command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Path => {
            println!("{}", socai_config::config_path()?.display());
        }
        ConfigCommand::Get { key: None } | ConfigCommand::List => {
            println!(
                "{}",
                serde_json::to_string_pretty(&socai_config::load_config_value()?)?
            );
        }
        ConfigCommand::Get { key: Some(key) } => match socai_config::get_config_key(&key)? {
            Some(Value::String(value)) => println!("{value}"),
            Some(value) => println!("{}", serde_json::to_string_pretty(&value)?),
            None => anyhow::bail!("config key is not set: {key}"),
        },
        ConfigCommand::Set { key, value } => {
            let canonical = socai_config::canonical_config_key(&key)?;
            let path = socai_config::set_config_key(&key, &value)?;
            println!("set {canonical} in {}", path.display());
            if canonical.starts_with("chrome.") {
                eprintln!(
                    "If the socai daemon is already running, run `socai stop` once so the next session uses the new chrome preference."
                );
            }
        }
        ConfigCommand::Unset { key } => {
            let canonical = socai_config::canonical_config_key(&key)?;
            let path = socai_config::unset_config_key(&key)?;
            println!("unset {canonical} in {}", path.display());
            if canonical.starts_with("chrome.") {
                eprintln!(
                    "If the socai daemon is already running, run `socai stop` once so the next session uses the new chrome preference."
                );
            }
        }
    }
    Ok(())
}

/// Parse repeated `--filter group=option` args into a `{group: option}` object.
/// Group/option validity is enforced by the core tool, so this only splits on
/// the first `=` and rejects a missing one.
fn parse_filters(filters: &[String]) -> Result<serde_json::Map<String, Value>> {
    let mut map = serde_json::Map::new();
    for raw in filters {
        let (group, option) = raw
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--filter expects group=option, got: {raw}"))?;
        map.insert(
            group.trim().to_string(),
            Value::String(option.trim().to_string()),
        );
    }
    Ok(map)
}

fn should_warn_for_update(command: &Command) -> bool {
    !matches!(
        command,
        Command::Daemon | Command::Update | Command::Version { .. } | Command::Config { .. }
    )
}

fn print_command_result(result: &Value, pretty: bool) -> Result<()> {
    if let Some(run_dir) = result.get("run_dir").and_then(Value::as_str) {
        eprintln!("run_dir: {run_dir}");
    }

    let data = result.get("data").unwrap_or(result);
    if pretty {
        println!("{}", serde_json::to_string_pretty(data)?);
    } else {
        println!("{}", serde_json::to_string(data)?);
    }
    Ok(())
}
