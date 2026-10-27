use std::future::Future;
use std::io::BufRead as _;
use std::path::PathBuf;
use std::sync::Arc;

use crate::runner::{emit_event, execute_run, prepare_run, record_run, PreparedRun, RunSpec};
use crate::WorkerBuildInfo;
use anyhow::{anyhow, Context, Result};
use socai_core::runtime::SocaiRuntime;
use socai_worker_protocol::{
    EventSequencer, ProtocolError, SessionBinding, WorkerCommand, WorkerOutput,
    CONTROL_PROTOCOL_VERSION, MAX_FRAME_BYTES,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info};
use uuid::Uuid;

struct ActiveRun {
    run_id: String,
    spec: RunSpec,
    prepared: PreparedRun,
    sequencer: Arc<Mutex<EventSequencer>>,
    task: JoinHandle<()>,
    control: mpsc::UnboundedSender<RunControl>,
    paused: bool,
}

enum RunControl {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
}

fn event_sequencer(
    binding: SessionBinding,
    run_id: String,
    sequence_offset: u64,
) -> EventSequencer {
    EventSequencer::from_next_sequence(binding, run_id, sequence_offset.saturating_add(1))
}

pub async fn run_stdio() -> Result<()> {
    run_stdio_with_build_info(WorkerBuildInfo::default()).await
}

pub async fn run_stdio_with_build_info(build: WorkerBuildInfo) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

    let session_dir = session_dir()?;
    tokio::fs::create_dir_all(&session_dir)
        .await
        .with_context(|| format!("failed to create {}", session_dir.display()))?;

    let (output_tx, output_rx) = mpsc::unbounded_channel();
    let mut writer = tokio::spawn(write_outputs(output_rx));
    let mut writer_failure = None;
    let worker_instance_id = Uuid::new_v4().to_string();
    let _ = output_tx.send(WorkerOutput::Ready {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        worker_instance_id: worker_instance_id.clone(),
        worker_version: build.worker_version,
        socai_core_version: build.socai_core_version,
        socai_core_revision: build.socai_core_revision,
        capabilities: build.capabilities,
    });

    let runtime = SocaiRuntime::new();
    let mut bound: Option<SessionBinding> = None;
    let mut active: Option<ActiveRun> = None;
    let (finished_tx, mut finished_rx) = mpsc::unbounded_channel::<String>();
    let mut lines = stdin_lines()?;

    loop {
        tokio::select! {
            result = &mut writer => {
                writer_failure = Some(match result {
                    Ok(Ok(())) => anyhow!("worker output task stopped before the command loop"),
                    Ok(Err(error)) => error.context("worker output task failed"),
                    Err(error) => anyhow!("worker output task could not be joined: {error}"),
                });
                let _ = cancel_active(&mut active, &output_tx, "worker output failed").await;
                break;
            }
            completed = finished_rx.recv(), if active.is_some() => {
                if let Some(run_id) = completed {
                    if active.as_ref().is_some_and(|run| run.run_id == run_id) {
                        socai_core::sites::login_wait::clear_login_resume(&run_id);
                        active = None;
                    }
                }
            }
            line = lines.recv() => {
                let Some(line) = line else {
                    let _ = cancel_active(&mut active, &output_tx, "worker input closed").await;
                    break;
                };
                let line = line.context("failed to read worker command")?;
                if line.len() > MAX_FRAME_BYTES {
                    send_error(&output_tx, None, "frame_too_large", "command exceeds 1 MiB");
                    continue;
                }
                let command = match serde_json::from_str::<WorkerCommand>(&line) {
                    Ok(command) => command,
                    Err(error) => {
                        send_error(&output_tx, None, "invalid_json", &error.to_string());
                        continue;
                    }
                };
                if let Err(error) = command.validate() {
                    send_protocol_error(&output_tx, request_id(&command), error);
                    continue;
                }
                match command {
                    WorkerCommand::SessionBind { request_id, binding, .. } => {
                        match bound.as_ref() {
                            Some(existing) if existing == &binding => {
                                let _ = output_tx.send(WorkerOutput::Bound {
                                    protocol_version: CONTROL_PROTOCOL_VERSION,
                                    request_id,
                                    binding,
                                    reused: true,
                                });
                            }
                            Some(_) => send_error(
                                &output_tx,
                                Some(request_id),
                                "session_mismatch",
                                "worker process is already bound to another session",
                            ),
                            None => {
                                bound = Some(binding.clone());
                                let _ = output_tx.send(WorkerOutput::Bound {
                                    protocol_version: CONTROL_PROTOCOL_VERSION,
                                    request_id,
                                    binding,
                                    reused: false,
                                });
                            }
                        }
                    }
                    WorkerCommand::RunStart {
                        request_id,
                        binding,
                        run_id,
                        prompt,
                        provider,
                        model,
                        enabled_sites,
                        max_steps,
                        max_tokens,
                        sequence_offset,
                        host_tools,
                        host_tool_endpoint,
                        remote_ocr,
                        external_write_grants,
                        research,
                        ..
                    } => {
                        if bound.as_ref() != Some(&binding) {
                            send_error(&output_tx, Some(request_id), "stale_binding", "run binding does not match this worker");
                            continue;
                        }
                        reap_terminal_run(&mut active).await;
                        if active.is_some() {
                            send_error(&output_tx, Some(request_id), "session_busy", "session already has an active run");
                            continue;
                        }
                        let research = match serde_json::from_value::<socai_core::agent::research_options::ResearchOptions>(if research.is_null() { serde_json::json!({}) } else { research }) {
                            Ok(value) if value.validate().is_ok() => value,
                            _ => { send_error(&output_tx, Some(request_id), "invalid_research", "Invalid research options"); continue; }
                        };
                        let spec = RunSpec {
                            binding: binding.clone(),
                            run_id: run_id.clone(),
                            prompt: prompt.clone(),
                            provider,
                            model,
                            enabled_sites,
                            max_steps,
                            max_tokens,
                            host_tools,
                            host_tool_endpoint,
                            remote_ocr,
                            external_write_grants,
                            research,
                        };
                        let prepared = match prepare_run(&session_dir, &spec) {
                            Ok(value) => value,
                            Err(error) => {
                                send_error(&output_tx, Some(request_id), "prepare_failed", &format!("{error:#}"));
                                continue;
                            }
                        };
                        let sequencer = Arc::new(Mutex::new(event_sequencer(
                            binding,
                            run_id.clone(),
                            sequence_offset,
                        )));
                        let _ = output_tx.send(WorkerOutput::RunAccepted {
                            protocol_version: CONTROL_PROTOCOL_VERSION,
                            request_id,
                            run_id: run_id.clone(),
                        });
                        let (task, control) = spawn_run_task(
                            runtime.clone(),
                            spec.clone(),
                            prepared_for_task(&prepared),
                            sequencer.clone(),
                            output_tx.clone(),
                            finished_tx.clone(),
                        );
                        active = Some(ActiveRun {
                            run_id: run_id.clone(),
                            spec,
                            prepared,
                            sequencer,
                            task,
                            control,
                            paused: false,
                        });
                    }
                    WorkerCommand::RunCancel { request_id, binding, run_id, .. } => {
                        if bound.as_ref() != Some(&binding) {
                            send_error(&output_tx, Some(request_id), "stale_binding", "cancel binding does not match this worker");
                            continue;
                        }
                        if active.as_ref().is_none_or(|run| run.run_id != run_id) {
                            send_error(&output_tx, Some(request_id), "run_not_active", "run is not active in this worker");
                            continue;
                        }
                        if cancel_active(&mut active, &output_tx, "cancelled by user").await {
                            let _ = output_tx.send(WorkerOutput::CommandAccepted {
                                protocol_version: CONTROL_PROTOCOL_VERSION,
                                request_id,
                            });
                        } else {
                            send_error(
                                &output_tx,
                                Some(request_id),
                                "run_not_active",
                                "run already completed",
                            );
                        }
                    }
                    WorkerCommand::RunPause { request_id, binding, run_id, .. } => {
                        if bound.as_ref() != Some(&binding) {
                            send_error(&output_tx, Some(request_id), "stale_binding", "pause binding does not match this worker");
                            continue;
                        }
                        let Some(run) = active.as_mut().filter(|run| run.run_id == run_id) else {
                            send_error(&output_tx, Some(request_id), "run_not_active", "run is not active in this worker");
                            continue;
                        };
                        if !run.paused {
                            let (acknowledged, acknowledgement) = oneshot::channel();
                            if run.control.send(RunControl::Pause(acknowledged)).is_err()
                                || acknowledgement.await.is_err()
                                || run.task.is_finished()
                                || run.sequencer.lock().await.is_terminal()
                            {
                                send_error(&output_tx, Some(request_id), "run_not_active", "run already completed");
                                continue;
                            }
                            run.paused = true;
                        }
                        let _ = output_tx.send(WorkerOutput::CommandAccepted {
                            protocol_version: CONTROL_PROTOCOL_VERSION,
                            request_id,
                        });
                    }
                    WorkerCommand::RunResume { request_id, binding, run_id, .. } => {
                        if bound.as_ref() != Some(&binding) {
                            send_error(&output_tx, Some(request_id), "stale_binding", "resume binding does not match this worker");
                            continue;
                        }
                        if active.as_ref().is_none_or(|run| run.run_id != run_id) {
                            send_error(&output_tx, Some(request_id), "run_not_active", "run is not active in this worker");
                            continue;
                        }
                        let run = active.as_mut().expect("active run was checked");
                        if run.paused {
                            let (acknowledged, acknowledgement) = oneshot::channel();
                            if run.control.send(RunControl::Resume(acknowledged)).is_err()
                                || acknowledgement.await.is_err()
                                || run.task.is_finished()
                                || run.sequencer.lock().await.is_terminal()
                            {
                                send_error(&output_tx, Some(request_id), "run_not_active", "run already completed");
                                continue;
                            }
                            run.paused = false;
                        } else {
                            socai_core::sites::login_wait::signal_login_resume(&run_id);
                        }
                        let _ = output_tx.send(WorkerOutput::CommandAccepted {
                            protocol_version: CONTROL_PROTOCOL_VERSION,
                            request_id,
                        });
                    }
                    WorkerCommand::Shutdown { .. } => {
                        let _ = cancel_active(&mut active, &output_tx, "worker shutdown").await;
                        break;
                    }
                }
            }
        }
    }

    runtime.disconnect_browser().await;
    drop(output_tx);
    if let Some(error) = writer_failure {
        return Err(error);
    }
    writer.await.context("worker output task failed")??;
    info!(%worker_instance_id, "session worker stopped");
    Ok(())
}

async fn reap_terminal_run(active: &mut Option<ActiveRun>) {
    let should_reap = match active.as_ref() {
        Some(run) if run.paused => false,
        Some(run) if run.task.is_finished() => true,
        Some(run) => run.sequencer.lock().await.is_terminal(),
        None => false,
    };
    if !should_reap {
        return;
    }
    let Some(run) = active.take() else {
        return;
    };
    let run_id = run.run_id.clone();
    if let Err(error) = run.task.await {
        error!(%error, %run_id, "completed session worker task could not be joined");
    }
    socai_core::sites::login_wait::clear_login_resume(&run_id);
}

fn spawn_run_task(
    runtime: SocaiRuntime,
    spec: RunSpec,
    prepared: PreparedRun,
    sequencer: Arc<Mutex<EventSequencer>>,
    output: mpsc::UnboundedSender<WorkerOutput>,
    finished: mpsc::UnboundedSender<String>,
) -> (JoinHandle<()>, mpsc::UnboundedSender<RunControl>) {
    let run_id = spec.run_id.clone();
    let (control, mut controls) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let execution = execute_run(runtime, spec, prepared, sequencer, output);
        drive_run_with_controls(execution, &mut controls).await;
        let _ = finished.send(run_id);
    });
    (task, control)
}

async fn drive_run_with_controls<F>(
    execution: F,
    controls: &mut mpsc::UnboundedReceiver<RunControl>,
) where
    F: Future<Output = ()>,
{
    tokio::pin!(execution);
    loop {
        tokio::select! {
            () = &mut execution => break,
            command = controls.recv() => match command {
                Some(RunControl::Pause(acknowledged)) => {
                    let _ = acknowledged.send(());
                    loop {
                        match controls.recv().await {
                            Some(RunControl::Resume(acknowledged)) => {
                                let _ = acknowledged.send(());
                                break;
                            }
                            Some(RunControl::Pause(acknowledged)) => {
                                let _ = acknowledged.send(());
                            }
                            None => return,
                        }
                    }
                }
                Some(RunControl::Resume(acknowledged)) => {
                    let _ = acknowledged.send(());
                }
                None => return,
            }
        }
    }
}

fn prepared_for_task(prepared: &PreparedRun) -> PreparedRun {
    PreparedRun {
        conversation_dir: prepared.conversation_dir.clone(),
        run_dir: prepared.run_dir.clone(),
        seed_messages: prepared.seed_messages.clone(),
        context_note: prepared.context_note.clone(),
    }
}

async fn cancel_active(
    active: &mut Option<ActiveRun>,
    output: &mpsc::UnboundedSender<WorkerOutput>,
    reason: &str,
) -> bool {
    let Some(run) = active.take() else {
        return false;
    };
    let was_running = !run.task.is_finished();
    if was_running {
        run.task.abort();
    }
    let _ = run.task.await;
    socai_core::sites::login_wait::clear_login_resume(&run.run_id);
    if !was_running && !run.paused {
        return false;
    }
    let mut sequencer = run.sequencer.lock().await;
    if sequencer.is_terminal() {
        return false;
    }
    record_run(
        &run.prepared,
        &run.spec.prompt,
        &format!("[task cancelled: {reason}]"),
        "cancelled",
    );
    if let Some(event) = sequencer.cancelled(reason) {
        emit_event(output, event);
        true
    } else {
        false
    }
}

async fn write_outputs(mut receiver: mpsc::UnboundedReceiver<WorkerOutput>) -> Result<()> {
    let mut stdout = tokio::io::stdout();
    while let Some(frame) = receiver.recv().await {
        let mut encoded = serde_json::to_vec(&frame)?;
        if encoded.len() > MAX_FRAME_BYTES {
            anyhow::bail!(
                "worker output frame is {} bytes, limit is {} bytes",
                encoded.len(),
                MAX_FRAME_BYTES
            );
        }
        encoded.push(b'\n');
        stdout.write_all(&encoded).await?;
        stdout.flush().await?;
    }
    Ok(())
}

fn stdin_lines() -> Result<mpsc::UnboundedReceiver<std::io::Result<String>>> {
    let (sender, receiver) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("socai-worker-stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let failed = line.is_err();
                if sender.send(line).is_err() || failed {
                    break;
                }
            }
        })
        .context("failed to start worker input thread")?;
    Ok(receiver)
}

fn session_dir() -> Result<PathBuf> {
    let value = std::env::var("SOCAI_SESSION_DIR")
        .context("SOCAI_SESSION_DIR is required for a Session Worker")?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        anyhow::bail!("SOCAI_SESSION_DIR must be absolute");
    }
    Ok(path)
}

fn request_id(command: &WorkerCommand) -> Option<String> {
    Some(match command {
        WorkerCommand::SessionBind { request_id, .. }
        | WorkerCommand::RunStart { request_id, .. }
        | WorkerCommand::RunCancel { request_id, .. }
        | WorkerCommand::RunPause { request_id, .. }
        | WorkerCommand::RunResume { request_id, .. }
        | WorkerCommand::Shutdown { request_id, .. } => request_id.clone(),
    })
}

fn send_protocol_error(
    output: &mpsc::UnboundedSender<WorkerOutput>,
    request_id: Option<String>,
    error: ProtocolError,
) {
    let code = match &error {
        ProtocolError::VersionMismatch { .. } => "protocol_version_mismatch",
        ProtocolError::InvalidField { .. } => "invalid_command",
    };
    send_error(
        output,
        request_id.map(|value| value.chars().take(160).collect()),
        code,
        &error.to_string(),
    );
}

fn send_error(
    output: &mpsc::UnboundedSender<WorkerOutput>,
    request_id: Option<String>,
    code: &str,
    message: &str,
) {
    error!(code, message, "session worker command rejected");
    let _ = output.send(WorkerOutput::CommandError {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id,
        code: code.into(),
        message: message.chars().take(4096).collect(),
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn test_run_spec(run_id: &str) -> RunSpec {
        RunSpec {
            binding: SessionBinding {
                user_id: "user-1".into(),
                session_id: "session-1".into(),
                kernel_id: "kernel-1".into(),
                assignment_epoch: 1,
            },
            run_id: run_id.into(),
            prompt: "research".into(),
            provider: None,
            model: None,
            enabled_sites: Vec::new(),
            max_steps: None,
            max_tokens: None,
            host_tools: Vec::new(),
            host_tool_endpoint: None,
            remote_ocr: false,
            external_write_grants: Vec::new(),
            research: Default::default(),
        }
    }

    fn test_prepared_run() -> PreparedRun {
        PreparedRun {
            conversation_dir: PathBuf::from("conversation"),
            run_dir: PathBuf::from("run"),
            seed_messages: Vec::new(),
            context_note: String::new(),
        }
    }

    #[tokio::test]
    async fn pause_acknowledgement_stops_polling_until_resume() {
        let (progress, mut observed) = mpsc::unbounded_channel();
        let execution = async move {
            for step in 1..=3 {
                progress.send(step).expect("progress receiver");
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        };
        let (control, mut controls) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            drive_run_with_controls(execution, &mut controls).await;
        });

        assert_eq!(observed.recv().await, Some(1));
        let (paused, pause_ack) = oneshot::channel();
        control
            .send(RunControl::Pause(paused))
            .expect("pause command");
        pause_ack.await.expect("pause acknowledgement");
        while observed.try_recv().is_ok() {}
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), observed.recv(),)
                .await
                .is_err(),
            "the execution future advanced after pause was acknowledged"
        );

        let (resumed, resume_ack) = oneshot::channel();
        control
            .send(RunControl::Resume(resumed))
            .expect("resume command");
        resume_ack.await.expect("resume acknowledgement");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), observed.recv(),)
                .await
                .expect("execution resumed")
                .is_some()
        );
        task.await.expect("controlled execution task");
    }

    #[tokio::test]
    async fn terminal_run_is_reaped_before_a_follow_up_is_admitted() {
        let binding = SessionBinding {
            user_id: "user-1".into(),
            session_id: "session-1".into(),
            kernel_id: "kernel-1".into(),
            assignment_epoch: 1,
        };
        let sequencer = Arc::new(Mutex::new(EventSequencer::new(binding, "run-1")));
        sequencer
            .lock()
            .await
            .failed("completed failure")
            .expect("terminal event");
        let task = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        });
        let (control, _controls) = mpsc::unbounded_channel();
        let mut active = Some(ActiveRun {
            run_id: "run-1".into(),
            spec: test_run_spec("run-1"),
            prepared: test_prepared_run(),
            sequencer,
            task,
            control,
            paused: false,
        });

        reap_terminal_run(&mut active).await;

        assert!(active.is_none());

        let mut resumed = event_sequencer(
            SessionBinding {
                user_id: "user-1".into(),
                session_id: "session-1".into(),
                kernel_id: "kernel-2".into(),
                assignment_epoch: 2,
            },
            "run-1".into(),
            17,
        );
        let event = resumed
            .event(socai_worker_protocol::WorkerEventPayload::Step { step: 2 })
            .expect("resumed event");
        assert_eq!(event.sequence, 18);
    }

    #[tokio::test]
    async fn paused_run_is_retained_and_can_still_be_cancelled() {
        let binding = SessionBinding {
            user_id: "user-1".into(),
            session_id: "session-1".into(),
            kernel_id: "kernel-1".into(),
            assignment_epoch: 1,
        };
        let sequencer = Arc::new(Mutex::new(EventSequencer::new(binding, "run-paused")));
        let completed_task = tokio::spawn(async {});
        tokio::task::yield_now().await;
        assert!(completed_task.is_finished());
        let (control, _controls) = mpsc::unbounded_channel();
        let mut active = Some(ActiveRun {
            run_id: "run-paused".into(),
            spec: test_run_spec("run-paused"),
            prepared: test_prepared_run(),
            sequencer,
            task: completed_task,
            control,
            paused: true,
        });

        reap_terminal_run(&mut active).await;
        assert!(active.is_some());

        let (output, _receiver) = mpsc::unbounded_channel();
        assert!(cancel_active(&mut active, &output, "cancelled while paused").await);
        assert!(active.is_none());
    }
}
