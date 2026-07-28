mod daemon;
mod progress;
mod tui;
mod version;

use anyhow::Result;
use clap::{Arg, ArgAction, ArgMatches};
use serde_json::{Map, Value};
use socai_core::cloud as socai_pro;
use socai_core::config as socai_config;
use socai_core::sites::{all_sites, find_site, ArgKind, CommandArg, SiteCommand, SiteSpec};

fn build_cli() -> clap::Command {
    let mut root = clap::Command::new("socai")
        .about("socai — site-savvy browser agent")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand(
            clap::Command::new("version")
                .about("Print installed version and latest release status.")
                .arg(
                    Arg::new("no-check")
                        .long("no-check")
                        .action(ArgAction::SetTrue)
                        .help("Only print the installed version; do not check GitHub Releases."),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Print machine-readable JSON."),
                ),
        )
        .subcommand(
            clap::Command::new("update")
                .about("Update the macOS release-binary install to the latest version."),
        )
        .subcommand(clap::Command::new("stop").about("Stop the background socai rust daemon."))
        .subcommand(
            clap::Command::new("config")
                .about("Read and write persistent socai preferences.")
                .subcommand(clap::Command::new("path").about("Print the config file path."))
                .subcommand(
                    clap::Command::new("get")
                        .about("Print the full config, or a single config key.")
                        .arg(Arg::new("key").help("Key to read, e.g. runs.dir or chrome.profile.")),
                )
                .subcommand(clap::Command::new("list").about("Print the full config as JSON."))
                .subcommand(
                    clap::Command::new("set")
                        .about("Persist a config key.")
                        .arg(
                            Arg::new("key")
                                .required(true)
                                .help("Key to set, e.g. runs.dir or chrome.profile."),
                        )
                        .arg(Arg::new("value").required(true).help("Value to store.")),
                )
                .subcommand(
                    clap::Command::new("unset")
                        .about("Remove a config key.")
                        .arg(
                            Arg::new("key")
                                .required(true)
                                .help("Key to remove, e.g. runs.dir or chrome.profile."),
                        ),
                ),
        )
        .subcommand(
            clap::Command::new("pro")
                .about("Activate and inspect socai pro access.")
                .subcommand(clap::Command::new("status").about("Print socai pro status."))
                .subcommand(
                    clap::Command::new("activate")
                        .about("Activate this install with an invite code.")
                        .arg(
                            Arg::new("invite_code")
                                .required(true)
                                .help("Invite code from the server operator."),
                        )
                        .arg(
                            Arg::new("server")
                                .long("server")
                                .hide(true)
                                .value_name("URL")
                                .help("Developer override for socai-server base URL."),
                        )
                        .arg(
                            Arg::new("label")
                                .long("label")
                                .value_name("TEXT")
                                .help("Optional device label shown in server records."),
                        ),
                ),
        )
        .subcommand(clap::Command::new("__daemon").hide(true));
    for site in all_sites() {
        let mut site_cmd = clap::Command::new(site.id)
            .about(site.about)
            .subcommand_required(true)
            .arg_required_else_help(true);
        for command in site.commands {
            site_cmd = site_cmd.subcommand(command_to_clap(command, false));
        }
        root = root.subcommand(site_cmd);
    }
    root
}

fn command_to_clap(command: &'static SiteCommand, hidden: bool) -> clap::Command {
    let mut cmd = clap::Command::new(command.name)
        .about(command.about)
        .hide(hidden);
    for arg in command.args {
        cmd = cmd.arg(arg_to_clap(arg));
    }
    cmd.arg(
        Arg::new("pretty")
            .long("pretty")
            .action(ArgAction::SetTrue)
            .help("Pretty-print the JSON result"),
    )
    .arg(
        Arg::new("debug-snapshot")
            .long("debug-snapshot")
            .action(ArgAction::SetTrue)
            .help(
                "Record DOM + a11y tree + screenshot bundles to <run_dir>/snapshots/ \
                 at every page change between tool operations.",
            ),
    )
}

fn arg_to_clap(arg: &'static CommandArg) -> Arg {
    let mut clap_arg = Arg::new(arg.key)
        .value_name(arg.value_name)
        .help(arg.help)
        .required(arg.required);
    if let Some(long) = arg.long {
        clap_arg = clap_arg.long(long);
    }
    match arg.kind {
        ArgKind::Str => clap_arg,
        ArgKind::StrList => clap_arg.action(ArgAction::Append),
        ArgKind::Int => clap_arg.value_parser(clap::value_parser!(i64)),
        ArgKind::Flag => clap_arg.action(ArgAction::SetTrue),
        ArgKind::KeyValueMap => clap_arg.action(ArgAction::Append),
    }
}

/// Collect clap matches into the JSON args object the daemon command expects.
fn collect_args(command: &'static SiteCommand, matches: &ArgMatches) -> Result<Value> {
    let mut args = Map::new();
    for arg in command.args {
        match arg.kind {
            ArgKind::Str => {
                if let Some(value) = matches.get_one::<String>(arg.key) {
                    args.insert(arg.key.to_string(), Value::String(value.clone()));
                }
            }
            ArgKind::StrList => {
                if let Some(values) = matches.get_many::<String>(arg.key) {
                    args.insert(
                        arg.key.to_string(),
                        Value::Array(values.cloned().map(Value::String).collect()),
                    );
                }
            }
            ArgKind::Int => {
                if let Some(value) = matches.get_one::<i64>(arg.key) {
                    args.insert(arg.key.to_string(), Value::from(*value));
                }
            }
            ArgKind::Flag => {
                if matches.get_flag(arg.key) {
                    args.insert(arg.key.to_string(), Value::Bool(true));
                }
            }
            ArgKind::KeyValueMap => {
                if let Some(values) = matches.get_many::<String>(arg.key) {
                    let mut map = Map::new();
                    for raw in values {
                        let (key, value) = raw.split_once('=').ok_or_else(|| {
                            anyhow::anyhow!(
                                "--{} expects key=value, got: {raw}",
                                arg.long.unwrap_or(arg.key)
                            )
                        })?;
                        map.insert(
                            key.trim().to_string(),
                            Value::String(value.trim().to_string()),
                        );
                    }
                    args.insert(arg.key.to_string(), Value::Object(map));
                }
            }
        }
    }
    args.insert(
        "debug_snapshot".to_string(),
        Value::Bool(matches.get_flag("debug-snapshot")),
    );
    Ok(Value::Object(args))
}

async fn run_site_command(
    site: &'static SiteSpec,
    command: &'static SiteCommand,
    matches: &ArgMatches,
) -> Result<()> {
    let args = collect_args(command, matches)?;
    let timeout = if command.slow.applies(&args) {
        daemon::LONG_COMMAND_TIMEOUT
    } else {
        daemon::DEFAULT_COMMAND_TIMEOUT
    };
    let preview = args
        .get("preview")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let show_reading = site.id == "xhs" && !preview;
    let show_ocr = args.get("ocr").and_then(Value::as_bool).unwrap_or(false);
    let total = args
        .get("num_notes")
        .and_then(Value::as_i64)
        .unwrap_or(10)
        .max(1) as u64;
    let mut renderer = progress::ProgressRenderer::new(show_reading, show_ocr, total);
    let result = {
        let mut on_progress = |event| renderer.update(event);
        daemon::send_or_spawn(site.id, command.name, args, timeout, &mut on_progress).await
    };
    renderer.finish();
    let result = result?;
    print_command_result(&result, matches.get_flag("pretty"))
}

fn should_warn_for_update(subcommand: &str) -> bool {
    !matches!(
        subcommand,
        "__daemon" | "update" | "version" | "config" | "pro"
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "info,async_tungstenite=off,tungstenite=off,hyper=off,reqwest=off".into()
            }),
        )
        .init();

    let matches = build_cli().get_matches();
    let Some((name, sub_matches)) = matches.subcommand() else {
        tui::run().await?;
        return Ok(());
    };
    if should_warn_for_update(name) {
        version::maybe_warn_if_outdated().await;
    }

    match name {
        "version" => {
            version::print_version_command(
                sub_matches.get_flag("no-check"),
                sub_matches.get_flag("json"),
            )
            .await?
        }
        "update" => version::run_update_command().await?,
        "config" => run_config_command(sub_matches)?,
        "pro" => run_pro_command(sub_matches).await?,
        "stop" => {
            // Graceful shutdown reaches whoever owns the IPC endpoint; the
            // sweep then kills any orphan daemon from any binary or SOCAI_HOME,
            // so one `socai stop` always leaves a clean slate.
            let daemon_stopped = daemon::stop_daemon().await?;
            let swept = daemon::kill_lingering_helpers().await;

            if daemon_stopped || swept > 0 {
                eprintln!("socai rust daemon stopped");
            } else {
                eprintln!("socai rust daemon is not running");
            }
            if swept > 0 {
                eprintln!("cleaned up {swept} lingering socai helper process(es)");
            }
        }
        "__daemon" => daemon::run_daemon().await?,
        _ => {
            let site = find_site(name).ok_or_else(|| anyhow::anyhow!("unknown command: {name}"))?;
            let (command_name, command_matches) = sub_matches
                .subcommand()
                .ok_or_else(|| anyhow::anyhow!("missing {name} subcommand"))?;
            let command = site
                .command(command_name)
                .ok_or_else(|| anyhow::anyhow!("unknown {name} command: {command_name}"))?;
            run_site_command(site, command, command_matches).await?;
        }
    }

    Ok(())
}

async fn run_pro_command(matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("activate", sub)) => {
            let invite_code = sub
                .get_one::<String>("invite_code")
                .expect("invite_code is required");
            let label = sub
                .get_one::<String>("label")
                .map(String::as_str)
                .unwrap_or("cli");
            let status = if let Some(server) = sub.get_one::<String>("server") {
                socai_pro::activate_with_base_url(server, invite_code, label).await?
            } else {
                socai_pro::activate(invite_code, label).await?
            };
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        _ => {
            println!("{}", serde_json::to_string_pretty(&socai_pro::status()?)?);
        }
    }
    Ok(())
}

fn run_config_command(matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("path", _)) => {
            println!("{}", socai_config::config_path()?.display());
        }
        Some(("get", sub)) => match sub.get_one::<String>("key") {
            Some(key) => match socai_config::get_config_key(key)? {
                Some(Value::String(value)) => println!("{value}"),
                Some(value) => println!("{}", serde_json::to_string_pretty(&value)?),
                None => anyhow::bail!("config key is not set: {key}"),
            },
            None => println!(
                "{}",
                serde_json::to_string_pretty(&socai_config::load_config_value()?)?
            ),
        },
        Some(("set", sub)) => {
            let key = sub.get_one::<String>("key").expect("key is required");
            let value = sub.get_one::<String>("value").expect("value is required");
            let canonical = socai_config::canonical_config_key(key)?;
            let path = socai_config::set_config_key(key, value)?;
            println!("set {canonical} in {}", path.display());
            if canonical == "chrome.profile" && value.trim().eq_ignore_ascii_case("remote") {
                eprintln!(
                    "Note: the remote hosted browser is beta — it needs socai pro, applies daily session limits, and its behaviour may change between releases."
                );
            }
            if canonical.starts_with("chrome.") {
                eprintln!(
                    "If the socai daemon is already running, run `socai stop` once so the next session uses the new chrome preference."
                );
            }
        }
        Some(("unset", sub)) => {
            let key = sub.get_one::<String>("key").expect("key is required");
            let canonical = socai_config::canonical_config_key(key)?;
            let path = socai_config::unset_config_key(key)?;
            println!("unset {canonical} in {}", path.display());
            if canonical.starts_with("chrome.") {
                eprintln!(
                    "If the socai daemon is already running, run `socai stop` once so the next session uses the new chrome preference."
                );
            }
        }
        // `socai config` / `socai config list` both print the whole config.
        _ => println!(
            "{}",
            serde_json::to_string_pretty(&socai_config::load_config_value()?)?
        ),
    }
    Ok(())
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
