#!/usr/bin/env bash
set -euo pipefail

python3 - <<'PY'
from pathlib import Path
import re

CONFLICT_RE = re.compile(r'<<<<<<< ours\n(.*?)=======\n(.*?)>>>>>>> theirs\n', re.S)

def resolve(path_s, choices):
    path = Path(path_s)
    text = path.read_text()
    blocks = list(CONFLICT_RE.finditer(text))
    if len(blocks) != len(choices):
        raise SystemExit(f'{path}: expected {len(choices)} conflicts, found {len(blocks)}')
    out = []
    pos = 0
    for idx, (match, choice) in enumerate(zip(blocks, choices), 1):
        ours, theirs = match.group(1), match.group(2)
        if choice == 'ours':
            merged = ours
        elif choice == 'theirs':
            merged = theirs
        elif callable(choice):
            merged = choice(ours, theirs)
        else:
            raise SystemExit(f'{path}: invalid choice at conflict {idx}: {choice!r}')
        out.append(text[pos:match.start()])
        out.append(merged)
        pos = match.end()
    out.append(text[pos:])
    resolved = ''.join(out)
    if any(marker in resolved for marker in ('<<<<<<< ours', '=======', '>>>>>>> theirs')):
        raise SystemExit(f'{path}: conflict marker remains after resolution')
    path.write_text(resolved)

def replace_region(text, start, end, replacement, label):
    start_i = text.find(start)
    if start_i < 0:
        raise SystemExit(f'{label}: start marker not found')
    end_i = text.find(end, start_i)
    if end_i < 0:
        raise SystemExit(f'{label}: end marker not found')
    return text[:start_i] + replacement + text[end_i:]

def channel_sig(ours, theirs):
    if 'task_type: Option<&str>' not in ours or 'PreparedWorkerSpawn' not in theirs:
        raise SystemExit('channel_dispatch conflict 1 shape changed')
    return ours.replace('std::result::Result<WorkerId, AgentError>',
                        'std::result::Result<PreparedWorkerSpawn, AgentError>')

def opencode_interactive(ours, theirs):
    if 'with_secret_scan_mode' not in ours or 'Some(input_tx)' not in theirs:
        raise SystemExit('channel_dispatch interactive OpenCode conflict shape changed')
    return '''        let worker =
            worker.with_secret_scan_mode(state.deps.runtime_config.sandbox.load().secret_scanner);
        (
            worker.with_sqlite_pool(state.deps.sqlite_pool.clone()),
            Some(input_tx),
        )
'''

def opencode_noninteractive(ours, theirs):
    if 'with_secret_scan_mode' not in ours or 'None' not in theirs:
        raise SystemExit('channel_dispatch noninteractive OpenCode conflict shape changed')
    return '''        let worker =
            worker.with_secret_scan_mode(state.deps.runtime_config.sandbox.load().secret_scanner);
        (
            worker.with_sqlite_pool(state.deps.sqlite_pool.clone()),
            None,
        )
'''

resolve('src/agent/channel_dispatch.rs', [
    channel_sig,
    opencode_interactive,
    opencode_noninteractive,
    'theirs',
    'theirs',
    'theirs',
    'theirs',
])

def worker_hook(ours, theirs):
    if 'with_secret_scan_mode' not in ours or 'with_worker_registry' not in theirs:
        raise SystemExit('worker hook conflict shape changed')
    return '''        .with_secret_scan_mode(deps.runtime_config.sandbox.load().secret_scanner)
        .with_worker_registry(callback, deps.process_control_registry.clone());
'''

resolve('src/agent/worker.rs', [worker_hook, 'theirs', 'theirs'])
worker = Path('src/agent/worker.rs')
text = worker.read_text()
old = 'crate::secrets::scrub::scrub_leaks(&scrubbed);'
new = 'crate::secrets::scrub::scrub_leaks_with_mode(&scrubbed, self.deps.runtime_config.sandbox.load().secret_scanner);'
if text.count(old) != 2:
    raise SystemExit(f'worker.rs: expected two registry result scrubs, found {text.count(old)}')
worker.write_text(text.replace(old, new))

resolve('src/api/channels.rs', ['theirs'])

resolve('src/hooks/spacebot.rs', ['theirs', 'theirs'])
hooks = Path('src/hooks/spacebot.rs')
text = hooks.read_text()
if text.count('crate::tools::MAX_TOOL_OUTPUT_BYTES') != 2:
    raise SystemExit('spacebot hook: expected two upstream fixed output limits')
text = text.replace('crate::tools::MAX_TOOL_OUTPUT_BYTES', 'crate::tools::tool_output_limit()')
hooks.write_text(text)

resolve('src/main.rs', ['theirs'])

resolve('src/opencode/worker.rs', ['theirs', 'theirs'])
opencode = Path('src/opencode/worker.rs')
text = opencode.read_text()
replacements = {
    'crate::secrets::scrub::scrub_leaks(&scrubbed_result);':
        'crate::secrets::scrub::scrub_leaks_with_mode(&scrubbed_result, self.secret_scan_mode);',
    'crate::secrets::scrub::scrub_leaks(&scrubbed);':
        'crate::secrets::scrub::scrub_leaks_with_mode(&scrubbed, self.secret_scan_mode);',
}
for old, new in replacements.items():
    if text.count(old) != 1:
        raise SystemExit(f'opencode worker: expected one configured scrub for {old}, found {text.count(old)}')
    text = text.replace(old, new, 1)
opencode.write_text(text)

def set_status_tests(ours, theirs):
    if 'outcome_without_evidence_is_rejected' not in ours or 'claims_completing_once' not in theirs:
        raise SystemExit('set_status test conflict shape changed')
    return ours.replace('non_interactive_outcome_claims_completing_idempotently',
                        'non_interactive_outcome_claims_completing_once')

resolve('src/tools/set_status.rs', [set_status_tests])
resolve('src/tools/spawn_worker.rs', ['theirs', 'theirs', 'theirs', 'theirs'])

pc = Path('src/agent/process_control.rs')
text = pc.read_text()
old = '''pub enum WorkerBackend {
    Builtin,
    OpenCode,
}
'''
new = '''pub enum WorkerBackend {
    Builtin,
    OpenCode,
    Acp,
}
'''
if text.count(old) != 1:
    raise SystemExit('process_control: WorkerBackend enum shape changed')
text = text.replace(old, new, 1)
old = '''            Self::Builtin => formatter.write_str("builtin"),
            Self::OpenCode => formatter.write_str("opencode"),
'''
new = '''            Self::Builtin => formatter.write_str("builtin"),
            Self::OpenCode => formatter.write_str("opencode"),
            Self::Acp => formatter.write_str("acp"),
'''
if text.count(old) != 1:
    raise SystemExit('process_control: WorkerBackend Display shape changed')
text = text.replace(old, new, 1)
pc.write_text(text)

# ACP must participate in the same registration and operation-correlated flow
# as builtin and OpenCode workers. Keeping the old channel-owned input/handle
# maps here would make ACP invisible to the new agent-owned registry.
acp = Path('src/acp/worker.rs')
text = acp.read_text()
old = 'use crate::agent::worker::WorkerTranscriptSnapshot;\n'
new = '''use crate::agent::process_control::{
    WorkerCallbackContext, WorkerFollowUp, WorkerOperationContext,
};
use crate::agent::worker::WorkerTranscriptSnapshot;
'''
if text.count(old) != 1:
    raise SystemExit('acp worker: process-control import anchor changed')
text = text.replace(old, new, 1)
if text.count('use uuid::Uuid;\n') != 1:
    raise SystemExit('acp worker: uuid import shape changed')
text = text.replace('use uuid::Uuid;\n', '', 1)

old = '    pub input_rx: Option<mpsc::Receiver<String>>,\n'
new = '    pub input_rx: Option<mpsc::Receiver<WorkerFollowUp>>,\n'
if text.count(old) != 1:
    raise SystemExit('acp worker: input_rx shape changed')
text = text.replace(old, new, 1)

old = '''    pub transcript_snapshot: WorkerTranscriptSnapshot,
    /// Owned subprocess; killed on drop so a cancelled worker never orphans
'''
new = '''    pub transcript_snapshot: WorkerTranscriptSnapshot,
    pub callback: WorkerCallbackContext,
    pub initial_operation: Option<WorkerOperationContext>,
    pub process_control_registry: Arc<crate::agent::process_control::ProcessControlRegistry>,
    /// Owned subprocess; killed on drop so a cancelled worker never orphans
'''
if text.count(old) != 1:
    raise SystemExit('acp worker: struct field anchor changed')
text = text.replace(old, new, 1)

new_ctor = r'''    pub fn new(
        id: WorkerId,
        callback: WorkerCallbackContext,
        initial_operation: WorkerOperationContext,
        channel_id: Option<ChannelId>,
        agent_id: AgentId,
        task: impl Into<String>,
        directory: PathBuf,
        command: String,
        args: Vec<String>,
        permission_mode: AcpPermissionMode,
        prompt_timeout: Duration,
        event_tx: broadcast::Sender<ProcessEvent>,
        process_control_registry: Arc<crate::agent::process_control::ProcessControlRegistry>,
    ) -> Self {
        Self {
            id,
            channel_id,
            agent_id,
            task: task.into(),
            directory,
            command,
            args,
            permission_mode,
            prompt_timeout,
            event_tx,
            input_rx: None,
            system_prompt: None,
            secrets_store: None,
            secret_scan_mode: SecretScanMode::Strict,
            child_registry: None,
            transcript_snapshot: crate::agent::worker::new_worker_transcript_snapshot(),
            callback,
            initial_operation: Some(initial_operation),
            process_control_registry,
            child: None,
        }
    }

'''
text = replace_region(
    text,
    '    pub fn new(\n',
    '    /// Attach the shared subprocess registry',
    new_ctor,
    'acp worker constructor',
)

new_interactive = r'''    pub fn new_interactive(
        id: WorkerId,
        callback: WorkerCallbackContext,
        initial_operation: WorkerOperationContext,
        channel_id: Option<ChannelId>,
        agent_id: AgentId,
        task: impl Into<String>,
        directory: PathBuf,
        command: String,
        args: Vec<String>,
        permission_mode: AcpPermissionMode,
        prompt_timeout: Duration,
        event_tx: broadcast::Sender<ProcessEvent>,
        process_control_registry: Arc<crate::agent::process_control::ProcessControlRegistry>,
    ) -> (Self, mpsc::Sender<WorkerFollowUp>) {
        let (input_tx, input_rx) = mpsc::channel(32);
        let mut worker = Self::new(
            id,
            callback,
            initial_operation,
            channel_id,
            agent_id,
            task,
            directory,
            command,
            args,
            permission_mode,
            prompt_timeout,
            event_tx,
            process_control_registry,
        );
        worker.input_rx = Some(input_rx);
        (worker, input_tx)
    }

'''
text = replace_region(
    text,
    '    pub fn new_interactive(\n',
    '    /// Set the system prompt injected into each prompt turn.',
    new_interactive,
    'acp worker interactive constructor',
)

new_status = r'''    /// Send a status update through the agent-owned worker registry.
    async fn send_status(&self, status: &str) {
        let applied = self
            .process_control_registry
            .update_worker_status(self.callback, status)
            .await;
        if applied != crate::agent::process_control::WorkerMutationResult::Applied {
            return;
        }
        self.event_tx
            .send(ProcessEvent::WorkerStatus {
                agent_id: self.agent_id.clone(),
                worker_id: self.id,
                worker_registration_id: self.callback.registration_id,
                channel_id: self.channel_id.clone(),
                status: status.to_string(),
            })
            .ok();
    }

    /// Send an idle event correlated to the operation that just completed.
    fn send_idle(&self, operation_id: crate::agent::process_control::WorkerOperationId) {
        self.event_tx
            .send(ProcessEvent::WorkerIdle {
                agent_id: self.agent_id.clone(),
                worker_id: self.id,
                worker_registration_id: self.callback.registration_id,
                operation_id,
                channel_id: self.channel_id.clone(),
            })
            .ok();
    }

'''
text = replace_region(
    text,
    '    /// Send a status update via the process event bus.\n',
    '    /// Scrub tool secret values from text',
    new_status,
    'acp worker status helpers',
)

for old, new in [
    ('self.send_status("initializing ACP session");', 'self.send_status("initializing ACP session").await;'),
    ('self.send_status("creating session");', 'self.send_status("creating session").await;'),
    ('self.send_status("exiting");', 'self.send_status("exiting").await;'),
    ('self.send_status("completed");', 'self.send_status("completed").await;'),
]:
    if text.count(old) != 1:
        raise SystemExit(f'acp worker: expected one status call {old}')
    text = text.replace(old, new, 1)

old = '''        self.send_status(&format!(
            "ACP session ready — resume with `omp -r {short_id}`"
        ));
'''
new = '''        self.send_status(&format!(
            "ACP session ready — resume with `omp -r {short_id}`"
        ))
        .await;
'''
if text.count(old) != 1:
    raise SystemExit('acp worker: session-ready status shape changed')
text = text.replace(old, new, 1)

interactive_block = r'''        // Interactive follow-up loop.
        if let Some(mut input_rx) = self.input_rx.take() {
            let scrubbed = self.scrub_text(&result_text);
            let scrubbed = crate::secrets::scrub::scrub_leaks_with_mode(
                &scrubbed,
                self.secret_scan_mode,
            );
            let operation = self
                .initial_operation
                .take()
                .expect("fresh ACP workers have an initial operation");
            let scrubbed = crate::agent::process_control::operation_result_or_marker(
                scrubbed,
                crate::agent::process_control::WorkerBackend::Acp,
            );
            let applied = self
                .process_control_registry
                .complete_worker_operation(
                    self.callback,
                    operation.operation_id,
                    "waiting for follow-up",
                )
                .await;
            if applied == crate::agent::process_control::WorkerMutationResult::Applied {
                self.event_tx
                    .send(ProcessEvent::WorkerOperationResult {
                        agent_id: self.agent_id.clone(),
                        worker_id: self.id,
                        worker_registration_id: self.callback.registration_id,
                        operation_id: operation.operation_id,
                        result_target: operation.result_target.clone(),
                        result: scrubbed,
                    })
                    .ok();
                self.send_status("waiting for follow-up").await;
                self.send_idle(operation.operation_id);
            }

            while let Some(follow_up) = input_rx.recv().await {
                self.send_status("processing follow-up").await;
                let follow_up_prompt = self.build_prompt(&follow_up.message);
                let turn_text = match self
                    .run_turn(
                        &mut stdin,
                        &mut reader,
                        &session_id,
                        &follow_up_prompt,
                        false,
                    )
                    .await
                {
                    Ok(text) => text,
                    Err(error) => {
                        tracing::error!(
                            worker_id = %self.id,
                            %error,
                            "ACP follow-up failed"
                        );
                        self.send_status("failed").await;
                        break;
                    }
                };
                let turn_text = crate::agent::process_control::operation_result_or_marker(
                    turn_text,
                    crate::agent::process_control::WorkerBackend::Acp,
                );
                let scrubbed = self.scrub_text(&turn_text);
                let scrubbed = crate::secrets::scrub::scrub_leaks_with_mode(
                    &scrubbed,
                    self.secret_scan_mode,
                );
                let applied = self
                    .process_control_registry
                    .complete_worker_operation(
                        self.callback,
                        follow_up.operation.operation_id,
                        "waiting for follow-up",
                    )
                    .await;
                if applied == crate::agent::process_control::WorkerMutationResult::Applied {
                    self.event_tx
                        .send(ProcessEvent::WorkerOperationResult {
                            agent_id: self.agent_id.clone(),
                            worker_id: self.id,
                            worker_registration_id: self.callback.registration_id,
                            operation_id: follow_up.operation.operation_id,
                            result_target: follow_up.operation.result_target.clone(),
                            result: scrubbed,
                        })
                        .ok();
                    self.send_status("waiting for follow-up").await;
                    self.send_idle(follow_up.operation.operation_id);
                }
            }
        }

'''
text = replace_region(
    text,
    '        // Interactive follow-up loop.\n',
    '        self.send_status("exiting").await;\n',
    interactive_block,
    'acp worker interactive loop',
)
acp.write_text(text)

dispatch = Path('src/agent/channel_dispatch.rs')
text = dispatch.read_text()
acp_spawn = r'''/// Spawn an ACP-backed worker for coding tasks.
///
/// Instead of a Rig agent loop, this spawns an ACP-compatible CLI subprocess
/// (Claude Code, Codex, Cursor CLI, etc.) and drives it over JSON-RPC stdio.
pub async fn spawn_acp_worker_from_state(
    state: &ChannelState,
    task: impl Into<String>,
    directory: &str,
    interactive: bool,
    required_skills: &[&str],
    task_context: WorkerTaskContext<'_>,
) -> std::result::Result<PreparedWorkerSpawn, AgentError> {
    if !interactive {
        return Err(AgentError::Other(anyhow::anyhow!(
            "ACP workers must be interactive"
        )));
    }

    let task = task.into();
    ensure_dispatch_readiness(state, "acp_worker");
    spawn_acp_worker_inner(
        state,
        &task,
        directory,
        interactive,
        required_skills,
        task_context,
    )
    .await
}

async fn spawn_acp_worker_inner(
    state: &ChannelState,
    task: &str,
    directory: &str,
    interactive: bool,
    required_skills: &[&str],
    task_context: WorkerTaskContext<'_>,
) -> std::result::Result<PreparedWorkerSpawn, AgentError> {
    let directory = expand_tilde(directory);

    let rc = &state.deps.runtime_config;
    let acp_config = rc.acp.load();

    if !acp_config.enabled {
        return Err(AgentError::Other(anyhow::anyhow!(
            "ACP workers are not enabled in config"
        )));
    }

    let persist_directory = directory.clone();
    let acp_secrets_store = state.deps.runtime_config.secrets.load().as_ref().clone();

    let mut worker_status_text = build_worker_status_text(rc.as_ref(), &state.deps.sandbox);
    let task_management = crate::prompts::text::get("fragments/opencode_task_management").trim();
    worker_status_text = Some(match worker_status_text {
        Some(existing) => format!("{existing}\n\n{task_management}"),
        None => task_management.to_string(),
    });

    if !required_skills.is_empty() {
        let skills = rc.skills.load();
        let mut entries = Vec::new();
        for name in required_skills {
            match skills.get(name) {
                Some(skill) => {
                    entries.push(format!("- {} — {}", skill.file_path.display(), skill.name));
                }
                None => {
                    tracing::warn!(skill = %name, "required skill not found, skipping injection");
                }
            }
        }
        if !entries.is_empty() {
            let block = format!(
                "## Required Skills\n\nBefore starting the task, read each of these skill \
                 files and follow them — they are part of the task's contract, not \
                 suggestions:\n{}",
                entries.join("\n")
            );
            worker_status_text = Some(match worker_status_text {
                Some(existing) => format!("{existing}\n\n{block}"),
                None => block,
            });
        }
    }

    let worker_task = worker_task_prompt(task, task_context.task_context);
    let worker_id = uuid::Uuid::new_v4();
    let autonomy_run = state.autonomy_run();
    let persisted_task = format!("[acp] {task}");
    let provenance = WorkerProvenance {
        origin_channel_id: Some(state.channel_id.clone()),
        origin_branch_id: task_context.origin_branch_id,
        task: persisted_task.clone(),
        task_id: None,
        autonomy_run_id: autonomy_run.as_ref().map(|run| run.run_id.clone()),
        spawning_process: crate::ProcessId::Channel(state.channel_id.clone()),
    };
    let reservation = state
        .deps
        .process_control_registry
        .reserve_worker(
            worker_id,
            &provenance,
            **state.deps.runtime_config.max_concurrent_workers.load(),
        )
        .await
        .map_err(|error| AgentError::Other(anyhow::anyhow!(error)))?;
    let callback = reservation.callback_context();
    let initial_operation = WorkerOperationContext {
        operation_id: WorkerOperationId::new(),
        requester: WorkerRequester::Channel {
            channel_id: state.channel_id.clone(),
        },
        result_target: WorkerResultTarget::Channel {
            channel_id: state.channel_id.clone(),
        },
        autonomy_run_id: autonomy_run.as_ref().map(|run| run.run_id.clone()),
    };

    let (worker, input_tx) = crate::acp::AcpWorker::new_interactive(
        worker_id,
        callback,
        initial_operation.clone(),
        Some(state.channel_id.clone()),
        state.deps.agent_id.clone(),
        &worker_task,
        directory,
        acp_config.command.clone(),
        acp_config.args.clone(),
        acp_config.permissions,
        std::time::Duration::from_secs(acp_config.prompt_timeout_secs),
        state.deps.event_tx.clone(),
        state.deps.process_control_registry.clone(),
    );
    let worker = worker.with_child_registry(state.deps.child_registry.clone());
    let worker = match worker_status_text {
        Some(ref prompt) => worker.with_system_prompt(prompt),
        None => worker,
    };
    let worker = match &acp_secrets_store {
        Some(store) => worker.with_secrets_store(store.clone()),
        None => worker,
    };
    let worker =
        worker.with_secret_scan_mode(state.deps.runtime_config.sandbox.load().secret_scanner);

    let transcript_snapshot = worker.transcript_snapshot();
    let (runtime_control, cancel_rx, terminal_notify) = WorkerRuntimeControl::new(
        transcript_snapshot.clone(),
        None,
        Some(input_tx),
        None,
        Some(state.process_run_logger.clone()),
    );
    let admission = state
        .deps
        .process_control_registry
        .register_new_worker(
            reservation,
            provenance,
            WorkerBackend::Acp,
            true,
            initial_operation.clone(),
            "starting",
            runtime_control,
        )
        .await
        .map_err(|error| AgentError::Other(anyhow::anyhow!(error)))?;

    if let Some(run) = &autonomy_run
        && !run.register_child(crate::agent::autonomy::AutonomyChild::WorkerOperation {
            worker_id,
            operation_id: initial_operation.operation_id,
        })
    {
        state
            .deps
            .process_control_registry
            .remove_worker_if_registration_matches(callback)
            .await;
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn worker: autonomy epoch is finishing"
        )));
    }

    if let Err(error) = state
        .process_run_logger
        .log_worker_started(
            Some(&state.channel_id),
            worker_id,
            &persisted_task,
            "acp",
            &state.deps.agent_id,
            interactive,
            Some(&persist_directory),
            autonomy_run.as_ref().map(|run| run.run_id.as_str()),
            task_context.origin_branch_id,
        )
        .await
    {
        if let Some(run) = &autonomy_run {
            run.settle_child(crate::agent::autonomy::AutonomyChild::WorkerOperation {
                worker_id,
                operation_id: initial_operation.operation_id,
            });
        }
        state
            .deps
            .process_control_registry
            .remove_worker_if_registration_matches(callback)
            .await;
        return Err(AgentError::Other(anyhow::anyhow!(error)));
    }

    if !state
        .deps
        .process_control_registry
        .worker_is_in_state(callback, WorkerRuntimeState::Starting)
        .await
    {
        settle_cancelled_start(
            &state.deps.process_control_registry,
            &state.process_run_logger,
            callback,
            autonomy_run.as_ref(),
            initial_operation.operation_id,
        )
        .await;
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't start worker: cancelled during durable start"
        )));
    }

    let worker_span = tracing::info_span!(
        "worker.run",
        worker_id = %worker_id,
        channel_id = %state.channel_id,
        worker_type = "acp",
    );
    let (start_gate, start_rx) = WorkerStartGate::new();
    let handle = spawn_worker_task(
        callback,
        state.deps.process_control_registry.clone(),
        cancel_rx,
        terminal_notify,
        start_rx,
        state.deps.event_tx.clone(),
        state.deps.agent_id.clone(),
        Some(state.channel_id.clone()),
        state.process_run_logger.clone(),
        transcript_snapshot,
        None,
        acp_secrets_store,
        Some(state.deps.task_store.clone()),
        state.deps.runtime_config.sandbox.load().secret_scanner,
        "acp",
        async move {
            let result = worker.run().await.map_err(SpacebotError::from);
            let result = result?;
            Ok::<WorkerOutcome, SpacebotError>(WorkerOutcome::Success {
                result: result.result_text,
            })
        }
        .instrument(worker_span),
    );

    if let Err(handle) = state
        .deps
        .process_control_registry
        .install_task_handle(admission.callback_context(), handle)
        .await
    {
        handle.abort();
        settle_cancelled_start(
            &state.deps.process_control_registry,
            &state.process_run_logger,
            callback,
            autonomy_run.as_ref(),
            initial_operation.operation_id,
        )
        .await;
        return Err(AgentError::Other(anyhow::anyhow!(
            "worker registration detached before task handle installation"
        )));
    }

    let acp_task = format!("[acp] {task}");
    {
        let mut status = state.status_block.write().await;
        status.add_worker(
            worker_id,
            callback.registration_id,
            &acp_task,
            false,
            interactive,
        );
    }

    let started_event = crate::ProcessEvent::WorkerStarted {
        agent_id: state.deps.agent_id.clone(),
        worker_id,
        worker_registration_id: callback.registration_id,
        channel_id: Some(state.channel_id.clone()),
        task: acp_task,
        worker_type: "acp".into(),
        interactive,
        directory: Some(persist_directory.to_string_lossy().to_string()),
    };

    state
        .deps
        .working_memory
        .emit(
            crate::memory::WorkingMemoryEventType::WorkerSpawned,
            format!("Worker spawned (acp): {task}"),
        )
        .channel(state.channel_id.to_string())
        .importance(0.6)
        .record();

    tracing::info!(worker_id = %worker_id, task = %task, interactive, "ACP worker spawned");

    Ok(PreparedWorkerSpawn {
        worker_id,
        callback,
        registry: state.deps.process_control_registry.clone(),
        run_logger: state.process_run_logger.clone(),
        start_gate,
        started_event,
        event_tx: state.deps.event_tx.clone(),
        autonomy_run,
        operation_id: initial_operation.operation_id,
    })
}

'''
text = replace_region(
    text,
    '/// Spawn an ACP-backed worker for coding tasks.\n',
    '/// Spawn a future as a tokio task that sends a `WorkerComplete` event on completion.\n',
    acp_spawn,
    'channel_dispatch ACP spawn',
)
dispatch.write_text(text)
PY

# The resolver writes semantic code, then lets the repository-pinned rustfmt
# normalize all touched Rust files before the port is committed.
cargo fmt --all

git add \
  src/acp/worker.rs \
  src/agent/channel_dispatch.rs \
  src/agent/process_control.rs \
  src/agent/worker.rs \
  src/api/channels.rs \
  src/hooks/spacebot.rs \
  src/main.rs \
  src/opencode/worker.rs \
  src/tools/set_status.rs \
  src/tools/spawn_worker.rs

git diff --cached --check

if git grep -n -E '^(<<<<<<<|=======|>>>>>>>)' -- ':!docs/integration-conflicts/**'; then
  echo 'conflict markers remain after #653 resolver' >&2
  exit 1
fi
