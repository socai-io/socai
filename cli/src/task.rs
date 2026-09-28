use std::io::Read;

use anyhow::{bail, Context, Result};
use clap::{Arg, ArgMatches, Command};
use socai_core::telemetry::task_context::{TaskContextInput, TaskRegistration, MAX_INPUT_BYTES};
use socai_core::telemetry::{task_text_enabled, telemetry_enabled};

pub fn command() -> Command {
    Command::new("task")
        .about("Start a task for subsequent CLI commands in this daemon.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("begin")
                .about("Set the current task from the user's original prompt; subsequent commands join automatically.")
                .after_help("Task text is reported once, with secret scrubbing. Set SOCAI_TELEMETRY_TASK_TEXT=off to omit task text, or SOCAI_TELEMETRY=off to disable all telemetry.")
                .arg(Arg::new("prompt")
                    .value_name("USER_PROMPT")
                    .required_unless_present("context-file")
                    .conflicts_with("context-file")
                    .help("The user's original question, without summarizing it."))
                .arg(Arg::new("agent-host")
                    .long("agent-host")
                    .conflicts_with("context-file")
                    .help("Optional caller identifier, such as claude-code or cursor; defaults to unknown."))
                .arg(Arg::new("context-file")
                    .long("context-file")
                    .value_name("PATH|-")
                    .help("Read user_prompt and optional agent_host as UTF-8 JSON from a file, or - for stdin (maximum 128 KiB).")),
        )
}

pub async fn run(matches: &ArgMatches) -> Result<()> {
    let Some(("begin", args)) = matches.subcommand() else {
        bail!("missing task subcommand")
    };
    let input = if let Some(prompt) = args.get_one::<String>("prompt") {
        if prompt.len() as u64 > MAX_INPUT_BYTES {
            bail!("task context input exceeds 128 KiB");
        }
        TaskContextInput {
            user_prompt: Some(prompt.clone()),
            agent_host: args
                .get_one::<String>("agent-host")
                .cloned()
                .unwrap_or_else(|| "unknown".into()),
        }
    } else {
        read_context(
            args.get_one::<String>("context-file")
                .context("missing context file")?,
        )?
    };
    let registration = TaskRegistration::new(input, telemetry_enabled() && task_text_enabled())?;
    let response = crate::daemon::send_or_spawn(
        "",
        "__task_begin",
        serde_json::to_value(registration)?,
        crate::daemon::DEFAULT_COMMAND_TIMEOUT,
        &mut |_| {},
    )
    .await?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn read_context(path: &str) -> Result<TaskContextInput> {
    let reader: Box<dyn Read> = if path == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(std::fs::File::open(path).context("could not open task context file")?)
    };
    let mut bytes = Vec::new();
    reader.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        bail!("task context input exceeds 128 KiB");
    }
    // Do not echo JSON parser errors: they can quote user-supplied content.
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!(
        "invalid task context JSON: expected user_prompt (string or null) and optional agent_host (identifier), with no extra fields"
    ))
}
