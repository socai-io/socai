//! Interactive socai TUI — invoked by `socai tui` or by running `socai` with
//! no arguments: slash completion, inline model picker, command history,
//! Ctrl-C exit, Esc-cancellable sub-menus.

use std::borrow::Cow;
use std::io::{self, IsTerminal};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use inquire::{Select, Text};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Cmd, CompletionType, Config, Editor, EventHandler, Helper, KeyEvent, Modifiers};
use socai_core::agent::{
    catalog_models_for, config_for, configured_default_model_for, provider_credential_kind,
    resolve_provider, save_api_key, save_default_model, AgentEvent, Backend, CredentialKind,
    ModelCatalogEntry, Provider, PROVIDERS,
};
use socai_core::agent::{local_agent_tools, make_run_dir, Conversation};
use socai_core::runtime::{
    create_llm_provider_for, run_agent_task as run_agent_with_tools, AgentRunConfig, SocaiRuntime,
};
use socai_core::sites::{find_site, SiteSpec};

/// Site the interactive TUI drives. Becomes a runtime choice once the TUI
/// grows a site switcher.
const TUI_SITE_ID: &str = "xhs";

fn tui_site() -> Result<&'static SiteSpec> {
    find_site(TUI_SITE_ID)
        .ok_or_else(|| anyhow::anyhow!("TUI default site {TUI_SITE_ID} is not registered"))
}

const PROVIDER_ORDER: &[Provider] = &[
    Provider::Kimi,
    Provider::Qwen,
    Provider::QwenIntl,
    Provider::Doubao,
    Provider::DeepSeek,
    Provider::OpenAI,
    Provider::Anthropic,
];

// Autocomplete-visible slash commands. `/new` is an alias of `/clear` handled
// in dispatch but intentionally omitted here so only `/clear` is suggested.
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("model", "Choose the active LLM model"),
    ("clear", "Clear the chat and start a new session"),
    ("exit", "Exit the TUI"),
];

const TUI_AGENT_PREAMBLE: &str =
    "You are running inside the Socai TUI as a conversational, multi-turn agent. \
     Besides the Xiaohongshu site tools you have local environment tools: \
     `read_file` (read text, or view image/screenshot artifacts) and `bash` (run \
     shell commands to write files, list/grep artifacts, etc.). Maintain \
     continuity with earlier turns in this chat. When the user asks you to save \
     or export something, write it with bash in the format they want. Stay within \
     the files relevant to the task and the user's ~/.socai data — do not run \
     destructive, networked, or system-wide commands.";

struct AppState {
    model: Option<String>,
    // Set alongside `model` by /model. Threaded through to the run so the
    // selection is honored verbatim — model-prefix inference can't tell
    // apart providers that share model ids (mainland vs international Qwen).
    provider: Option<Provider>,
    conversation: Conversation,
}

/// Live snapshot of an in-flight run, updated off the event stream so an
/// interrupted run can still be recorded with its run_id + partial answer.
#[derive(Default)]
struct LiveRun {
    run_id: String,
    assistant: String,
}

// ---------- rustyline helper: slash completion -----------------------------

struct SocaiHelper;

impl Completer for SocaiHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        _pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        if !line.starts_with('/') {
            return Ok((0, Vec::new()));
        }
        let first = line.split_whitespace().next().unwrap_or("");
        let prefix = first.trim_start_matches('/');
        let mut out = Vec::new();
        for (name, desc) in SLASH_COMMANDS {
            if name.starts_with(prefix) {
                out.push(Pair {
                    display: format!("/{name}  — {desc}"),
                    replacement: format!("/{name}"),
                });
            }
        }
        Ok((0, out))
    }
}

impl Hinter for SocaiHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        // Slash-command hint — only when the cursor is at the end of a
        // single-line buffer that starts with '/'.
        if pos != line.len() || !line.starts_with('/') || line.contains(' ') {
            return None;
        }
        let prefix = line.trim_start_matches('/');
        let matches: Vec<&(&str, &str)> = SLASH_COMMANDS
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .collect();
        match matches.as_slice() {
            [] => None,
            // Exactly one match → inline completion ghost for the rest + desc.
            [(name, desc)] => {
                if prefix.len() < name.len() {
                    Some(format!("{}  — {}", &name[prefix.len()..], desc))
                } else {
                    Some(format!("  — {desc}"))
                }
            }
            // Several match (e.g. just "/") → point at the interactive menu
            // (Enter opens it) rather than cramming options into the hint.
            _ => Some("  ↵ for command menu".to_string()),
        }
    }
}
impl Highlighter for SocaiHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        Cow::Borrowed(prompt)
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        // ANSI bright-black (dim) so the ghost suggestion is visible but muted.
        Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }
}
impl Validator for SocaiHelper {}
impl Helper for SocaiHelper {}

// ---------- entry point ----------------------------------------------------

pub async fn run() -> Result<()> {
    ensure_any_llm_key().await?;

    let runtime = SocaiRuntime::new();
    let conversation = Conversation::new(None).context("failed to create chat session")?;
    let mut state = AppState {
        model: None,
        provider: None,
        conversation,
    };

    let mut editor = build_editor()?;
    print_header(&state)?;

    loop {
        let res = tokio::task::block_in_place(|| editor.readline("socai> "));
        let line = match res {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => {
                println!();
                break;
            }
            Err(err) => {
                eprintln!("[socai] read error: {err}");
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let _ = editor.add_history_entry(trimmed);

        if trimmed.starts_with('/') {
            // A bare "/" opens the interactive, filterable command menu;
            // a fully-typed command (e.g. "/clear") dispatches directly.
            let command_line = if trimmed == "/" {
                match slash_menu().await? {
                    Some(name) => format!("/{name}"),
                    None => continue, // Esc — back to the prompt
                }
            } else {
                trimmed.to_string()
            };
            if handle_command(&command_line, &mut state).await? {
                break;
            }
            print_header(&state)?;
            continue;
        }

        if let Err(err) = run_agent_task(&runtime, trimmed, &mut state).await {
            eprintln!("[socai] error: {err:#}");
        }
    }

    let _ = runtime.close_all_site_sessions().await;
    runtime.disconnect_browser().await;
    Ok(())
}

fn build_editor() -> Result<Editor<SocaiHelper, DefaultHistory>> {
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .auto_add_history(false)
        .build();
    let mut editor: Editor<SocaiHelper, DefaultHistory> = Editor::with_config(config)?;
    editor.set_helper(Some(SocaiHelper));
    // Soft-newline bindings for multi-line input. Rustyline can only see
    // what the terminal actually sends, so coverage depends on terminal
    // settings:
    //   • Alt+Enter / Option+Enter — always works (terminals send `Esc \r`).
    //   • Shift+Enter — only if the terminal reports modifiers (WezTerm,
    //     Ghostty, Alacritty by default; iTerm2 with "Report modifiers" on).
    //   • Ctrl+Enter via Ctrl-J — only if the terminal is configured to send
    //     `^J` for Ctrl+Return (e.g. iTerm2 → Profile → Keys mapping).
    for ch in ['\r', '\n'] {
        editor.bind_sequence(
            KeyEvent::new(ch, Modifiers::ALT),
            EventHandler::Simple(Cmd::Newline),
        );
        editor.bind_sequence(
            KeyEvent::new(ch, Modifiers::SHIFT),
            EventHandler::Simple(Cmd::Newline),
        );
    }
    editor.bind_sequence(KeyEvent::ctrl('j'), EventHandler::Simple(Cmd::Newline));
    Ok(editor)
}

fn print_header(state: &AppState) -> Result<()> {
    println!("Current model: {}", active_model(state)?);
    println!(
        "Chat with the agent — turns share context within a session. \
         Type / then Enter for the command menu; /clear starts a new chat. \
         Ctrl+C interrupts a running task (records it); Ctrl+C at the prompt exits. \
         Alt+Enter for newline (Shift/Ctrl+Enter if your terminal supports it)."
    );
    Ok(())
}

// ---------- slash command dispatch -----------------------------------------

async fn handle_command(line: &str, state: &mut AppState) -> Result<bool> {
    let body = line.trim_start_matches('/').trim();
    let (cmd, rest) = body
        .split_once(char::is_whitespace)
        .map(|(c, r)| (c, r.trim()))
        .unwrap_or((body, ""));
    match cmd.to_ascii_lowercase().as_str() {
        "exit" | "quit" => Ok(true),
        "model" => {
            handle_model_command(state, rest).await?;
            Ok(false)
        }
        // `/new` is an undocumented alias of `/clear`.
        "clear" | "new" => {
            handle_clear_command(state)?;
            Ok(false)
        }
        "" => Ok(false),
        other => {
            eprintln!("[socai] unknown command: /{other}");
            Ok(false)
        }
    }
}

// ---------- interactive slash-command menu ---------------------------------

/// Filterable command menu opened by typing a bare `/`. Returns the chosen
/// command name (e.g. "clear"), or `None` on Esc. Typing narrows the list.
async fn slash_menu() -> Result<Option<String>> {
    let options: Vec<String> = SLASH_COMMANDS
        .iter()
        .map(|(name, desc)| format!("/{name}  — {desc}"))
        .collect();
    let chosen = tokio::task::spawn_blocking(move || {
        Select::new("Command", options)
            .with_help_message("type to filter · ↑/↓ to move · enter to run · esc to cancel")
            .prompt_skippable()
    })
    .await
    .context("slash menu thread panicked")??;
    Ok(chosen.and_then(|label| {
        label
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .map(str::to_string)
            .filter(|name| !name.is_empty())
    }))
}

// ---------- /clear (and its /new alias) ------------------------------------

fn handle_clear_command(state: &mut AppState) -> Result<()> {
    state.conversation =
        Conversation::new(state.model.clone()).context("failed to start a new session")?;
    println!("[socai] chat cleared — new session {}", state.conversation.id);
    Ok(())
}

// ---------- model picker ---------------------------------------------------

#[derive(Clone)]
struct ModelOption {
    label: String,
    provider: Provider,
    model: String,
}

fn model_options() -> Vec<ModelOption> {
    PROVIDER_ORDER
        .iter()
        .flat_map(|provider| {
            let cfg = config_for(*provider);
            let selected_model = configured_default_model_for(*provider);
            let key_status = match provider_credential_kind(*provider) {
                Some(CredentialKind::ApiKey) => "api key set",
                Some(CredentialKind::CodexOAuth) => "codex oauth set",
                None => "api key missing",
            };
            let mut models = catalog_models_for(*provider);
            if !selected_model.trim().is_empty()
                && !models.iter().any(|model| model.id == selected_model)
            {
                models.insert(
                    0,
                    ModelCatalogEntry {
                        id: selected_model.clone(),
                        display_name: Some(selected_model.clone()),
                        source: Some("saved-default".into()),
                        recommended: false,
                        pricing: None,
                    },
                );
            }
            models.into_iter().map(move |model| {
                let active = if model.id == selected_model {
                    " · current"
                } else {
                    ""
                };
                let label = format!(
                    "{} — {} · {} ({}{})",
                    cfg.display_name,
                    model.label(),
                    model.id,
                    key_status,
                    active
                );
                ModelOption {
                    label,
                    provider: *provider,
                    model: model.id,
                }
            })
        })
        .collect()
}

async fn handle_model_command(state: &mut AppState, rest: &str) -> Result<()> {
    if !rest.trim().is_empty() {
        let trimmed = rest.trim();
        // Provider names take precedence over model-prefix inference — the
        // prefix match can't distinguish "qwen-intl" from a qwen model id.
        let (provider, model) = if let Some(provider) = Provider::from_name(trimmed) {
            (provider, configured_default_model_for(provider))
        } else {
            (resolve_provider(None, Some(trimmed))?, trimmed.to_string())
        };
        set_active_model(state, provider, model).await?;
        return Ok(());
    }

    let current = active_model(state)?;
    let options = model_options();
    let starting_cursor = options.iter().position(|o| o.model == current).unwrap_or(0);
    let labels: Vec<String> = options.iter().map(|o| o.label.clone()).collect();

    let chosen_label = tokio::task::spawn_blocking(move || {
        Select::new("Select model", labels)
            .with_starting_cursor(starting_cursor)
            .with_help_message("↑/↓ to move · enter to confirm · esc to cancel")
            .prompt_skippable()
    })
    .await
    .context("model picker thread panicked")??;

    let Some(chosen_label) = chosen_label else {
        return Ok(()); // Esc — fall back to main prompt
    };

    let chosen = options
        .into_iter()
        .find(|o| o.label == chosen_label)
        .context("model picker lost its row")?;

    set_active_model(state, chosen.provider, chosen.model).await
}

async fn set_active_model(state: &mut AppState, provider: Provider, model: String) -> Result<()> {
    if provider_credential_kind(provider).is_none() && !prompt_save_key(provider).await? {
        println!("[socai] model unchanged.");
        return Ok(());
    }

    let path = save_default_model(provider, &model)?;
    state.model = Some(model.clone());
    state.provider = Some(provider);
    println!(
        "[socai] model set to {model}; saved defaults to {}",
        path.display()
    );
    Ok(())
}

// ---------- API key prompts (Esc-cancellable) ------------------------------

async fn prompt_save_key(provider: Provider) -> Result<bool> {
    let cfg = config_for(provider);
    let env_hint = cfg.env_keys.join(" or ");
    if provider == Provider::OpenAI {
        return prompt_openai_credential().await;
    }
    println!(
        "[socai] No API key found for {}. Enter one now (esc to cancel; you can also set {}).",
        cfg.display_name, env_hint
    );
    let label = cfg.display_name.to_string();
    let provider_for_save = provider;

    tokio::task::spawn_blocking(move || -> Result<bool> {
        let key = match Text::new(&format!("{label} API key:")).prompt_skippable()? {
            Some(k) => k,
            None => return Ok(false),
        };
        let trimmed = key.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let path = save_api_key(provider_for_save, trimmed)?;
        println!("[socai] Saved {label} key to {}", path.display());
        Ok(true)
    })
    .await
    .context("API key prompt thread panicked")?
}

async fn prompt_openai_credential() -> Result<bool> {
    println!("[socai] No OpenAI credential found. Choose Codex OAuth or paste an OpenAI API key.");
    let choice = tokio::task::spawn_blocking(move || {
        Select::new(
            "OpenAI credential",
            vec![
                "Use Codex OAuth".to_string(),
                "Paste OpenAI API key".to_string(),
            ],
        )
        .with_help_message("↑/↓ to move · enter to confirm · esc to cancel")
        .prompt_skippable()
    })
    .await
    .context("OpenAI credential picker thread panicked")??;

    let Some(choice) = choice else {
        return Ok(false);
    };
    if choice.starts_with("Use Codex OAuth") {
        run_codex_login().await?;
        return Ok(provider_credential_kind(Provider::OpenAI).is_some());
    }
    prompt_save_api_key_only(Provider::OpenAI).await
}

async fn prompt_save_api_key_only(provider: Provider) -> Result<bool> {
    let cfg = config_for(provider);
    let label = cfg.display_name.to_string();
    let provider_for_save = provider;
    tokio::task::spawn_blocking(move || -> Result<bool> {
        let key = match Text::new(&format!("{label} API key:")).prompt_skippable()? {
            Some(k) => k,
            None => return Ok(false),
        };
        let trimmed = key.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let path = save_api_key(provider_for_save, trimmed)?;
        println!("[socai] Saved {label} key to {}", path.display());
        Ok(true)
    })
    .await
    .context("API key prompt thread panicked")?
}

async fn run_codex_login() -> Result<()> {
    tokio::task::spawn_blocking(move || -> Result<()> {
        let status = std::process::Command::new("codex")
            .arg("login")
            .status()
            .context(
                "failed to start `codex login`; install Codex CLI or paste an OpenAI API key",
            )?;
        if !status.success() {
            anyhow::bail!("`codex login` exited with status {status}");
        }
        Ok(())
    })
    .await
    .context("codex login thread panicked")??;
    Ok(())
}

async fn ensure_any_llm_key() -> Result<()> {
    if PROVIDERS
        .iter()
        .any(|cfg| provider_credential_kind(cfg.provider).is_some())
    {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "No LLM credential found. Set OPENAI_API_KEY / ANTHROPIC_API_KEY / \
             KIMI_API_KEY / QWEN_API_KEY / DOUBAO_API_KEY / DEEPSEEK_API_KEY, \
             run `codex login`, or run `socai tui` \
             in a terminal to save one."
        );
    }

    println!("No LLM API key found. Choose a provider to configure.");
    let options: Vec<(String, Provider)> = PROVIDER_ORDER
        .iter()
        .map(|p| {
            let cfg = config_for(*p);
            (format!("{} ({})", cfg.display_name, cfg.env_keys[0]), *p)
        })
        .collect();
    let labels: Vec<String> = options.iter().map(|(l, _)| l.clone()).collect();

    let chosen_label = tokio::task::spawn_blocking(move || {
        Select::new("Provider", labels)
            .with_help_message("↑/↓ to move · enter to confirm · esc to cancel")
            .prompt_skippable()
    })
    .await
    .context("provider picker thread panicked")??;

    let Some(chosen_label) = chosen_label else {
        anyhow::bail!("cancelled");
    };
    let provider = options
        .into_iter()
        .find(|(l, _)| *l == chosen_label)
        .map(|(_, p)| p)
        .context("provider picker lost its row")?;

    if !prompt_save_key(provider).await? {
        anyhow::bail!("cancelled");
    }
    Ok(())
}

async fn ensure_model_key(provider: Provider) -> Result<()> {
    if provider_credential_kind(provider).is_some() {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        let cfg = config_for(provider);
        anyhow::bail!("missing API key for {}", cfg.display_name);
    }
    if !prompt_save_key(provider).await? {
        anyhow::bail!("cancelled");
    }
    Ok(())
}

// ---------- agent task runner ----------------------------------------------

fn active_model(state: &AppState) -> Result<String> {
    if let Some(model) = &state.model {
        if !model.trim().is_empty() {
            return Ok(model.clone());
        }
    }
    if let Some(model) = env_model() {
        return Ok(model);
    }
    let provider = resolve_provider(None, None)?;
    Ok(configured_default_model_for(provider))
}

async fn run_agent_task(runtime: &SocaiRuntime, task: &str, state: &mut AppState) -> Result<()> {
    let llm_provider = build_llm_provider(state.provider, state.model.as_deref()).await?;
    println!();
    println!("[socai] using {}", llm_provider.label());

    // Pin the run dir up front (instead of letting the loop allocate it) so an
    // interrupted or failed run still knows where partial artifacts live and
    // can record itself for follow-ups.
    let site = tui_site()?;
    let run_dir = make_run_dir(&format!("{} {task}", site.id));
    let _ = std::fs::create_dir_all(&run_dir);

    // Browser/network setup can fail (e.g. DNS ERR_NAME_NOT_RESOLVED). Record
    // the run even then, so a follow-up still knows what the user asked.
    println!("[socai] connecting browser...");
    let page = match runtime.ensure_site_page(site.id, site.home_url).await {
        Ok(page) => page,
        Err(err) => {
            state.conversation.record_run(
                task,
                &format!("[run did not start — browser/network error: {err:#}]"),
                &run_dir,
                "failed",
            );
            return Err(err);
        }
    };
    let agent_tools = site.default_agent_tools.unwrap_or(site.agent_tools);
    let mut tools = agent_tools(page, llm_provider.clone()).await?;
    tools.extend(local_agent_tools());

    // Track run_id + latest assistant text live off the event stream so we can
    // record a meaningful run even if it's interrupted mid-flight.
    let live = Arc::new(Mutex::new(LiveRun::default()));
    let live_for_printer = live.clone();
    let (tx, mut rx) = tokio::sync::broadcast::channel::<AgentEvent>(256);
    let printer = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if let Ok(mut g) = live_for_printer.lock() {
                match &event {
                    AgentEvent::Started { run_id, .. } => g.run_id = run_id.clone(),
                    AgentEvent::AssistantText { text, .. } => g.assistant = text.clone(),
                    _ => {}
                }
            }
            print_agent_event(&event);
        }
    });

    // Seed the conversation with prior runs and tell the agent which run dirs
    // belong to this conversation so it can read back earlier artifacts.
    let preamble = format!(
        "{TUI_AGENT_PREAMBLE}\n\n{}",
        state.conversation.context_note()
    );
    let agent_instructions = site
        .default_agent_instructions
        .unwrap_or(site.agent_instructions);
    let config = AgentRunConfig {
        extra_instructions: agent_instructions(&preamble),
        enabled_sites: vec![site.id.to_string()],
        seed_messages: state.conversation.chat_messages(),
        session_id: Some(state.conversation.id.clone()),
        run_dir: Some(run_dir.clone()),
        ..AgentRunConfig::default()
    };
    // Race the agent against Ctrl+C so the user can interrupt a long run and
    // ask a follow-up. (At the idle prompt, rustyline turns Ctrl+C into an exit
    // instead — these two contexts don't overlap.)
    let agent = run_agent_with_tools(task, llm_provider, tools, config, tx);
    tokio::pin!(agent);
    let outcome = tokio::select! {
        outcome = &mut agent => outcome,
        _ = tokio::signal::ctrl_c() => {
            printer.abort();
            let partial = live
                .lock()
                .map(|g| g.assistant.clone())
                .unwrap_or_default();
            let assistant = if partial.trim().is_empty() {
                "[interrupted by user before producing an answer]".to_string()
            } else {
                format!("{}\n\n[interrupted by user]", partial.trim())
            };
            // Record the interrupted run so a follow-up keeps its context.
            state.conversation.record_run(task, &assistant, &run_dir, "interrupted");
            println!("\n[socai] interrupted — recorded; ask a follow-up, or press Ctrl+C at the prompt to exit.");
            return Ok(());
        }
    };
    let _ = printer.await;
    let outcome = match outcome.context("agent loop failed") {
        Ok(outcome) => outcome,
        Err(err) => {
            // Record the failed run too, so its topic survives for follow-ups.
            state.conversation.record_run(
                task,
                &format!("[run failed: {err:#}]"),
                &run_dir,
                "failed",
            );
            return Err(err);
        }
    };

    println!();
    println!("{}", outcome.final_text.trim());
    println!();
    let cost = outcome
        .usage
        .cost
        .as_ref()
        .map(|cost| format!(" estimated_cost={:.6}_{}", cost.total, cost.currency))
        .unwrap_or_default();
    println!(
        "[socai] run_id={} steps={} input_tokens={} output_tokens={} cached_input_tokens={}{}",
        outcome.run_id,
        outcome.steps,
        outcome.usage.input_tokens,
        outcome.usage.output_tokens,
        outcome.usage.cache_read_input_tokens,
        cost,
    );
    println!("[socai] run_dir={}", outcome.run_dir.display());
    println!();

    // An API error surfaces as an Ok outcome whose final_text is the error.
    // Record a short marker instead of dumping the whole provider message into
    // the conversation seed.
    let recorded = if outcome.final_text.starts_with("API error:") {
        "[run failed: LLM API error]".to_string()
    } else {
        outcome.final_text.clone()
    };
    state
        .conversation
        .record_run(task, &recorded, &outcome.run_dir, "completed");
    Ok(())
}

async fn build_llm_provider(
    provider: Option<Provider>,
    model: Option<&str>,
) -> Result<Arc<dyn Backend>> {
    let model_or_env = model
        .map(str::to_string)
        .filter(|m| !m.trim().is_empty())
        .or_else(env_model);
    let effective_provider = match provider {
        Some(provider) => provider,
        None => resolve_provider(None, model_or_env.as_deref())?,
    };
    ensure_model_key(effective_provider).await?;
    create_llm_provider_for(Some(effective_provider.as_str()), model_or_env.as_deref())
}

fn env_model() -> Option<String> {
    std::env::var("SOCAI_MODEL")
        .ok()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

// ---------- event rendering ------------------------------------------------

fn print_agent_event(event: &AgentEvent) {
    match event {
        AgentEvent::Started {
            run_id,
            task,
            model,
        } => {
            println!("\n▸ task: {task}");
            println!("  run {run_id} · model {model}");
        }
        AgentEvent::Step { .. } => println!("\n──"),
        AgentEvent::AssistantText { text, .. } => {
            for line in text.lines() {
                println!("  {line}");
            }
        }
        AgentEvent::Reasoning { text, .. } => {
            for line in text.lines() {
                let trimmed = line.trim_end();
                if !trimmed.is_empty() {
                    println!("  · {trimmed}");
                }
            }
        }
        AgentEvent::ToolCall {
            name,
            input,
            repeat_count,
            ..
        } => {
            let mut preview = serde_json::to_string(input).unwrap_or_else(|_| input.to_string());
            if preview.chars().count() > 180 {
                preview = preview.chars().take(180).collect::<String>() + "...";
            }
            if *repeat_count > 1 {
                println!("  → {name}({preview}) repeat={repeat_count}");
            } else {
                println!("  → {name}({preview})");
            }
        }
        AgentEvent::ToolResult {
            name,
            summary,
            duration_ms,
            error,
            ..
        } => {
            if let Some(error) = error {
                println!("  ✗ {name} ({duration_ms}ms): {error}");
            } else {
                let first = summary.lines().next().unwrap_or("");
                println!("  ← {name} ({duration_ms}ms): {first}");
            }
        }
        AgentEvent::ApiError { step, message } => {
            println!("  ✗ API error on step {step}: {message}");
        }
        AgentEvent::Done { steps, .. } => println!("\n✓ done in {steps} steps"),
    }
}
