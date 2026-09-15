//! ACP-backed worker: drives an external coding agent over the Agent Client
//! Protocol (JSON-RPC over stdio).
//!
//! Instead of running a Rig agent loop, this worker spawns a subprocess for
//! any ACP-compatible CLI (Claude Code, Codex, Cursor CLI, etc.) and drives
//! it through `initialize` → `session/new` → `session/prompt` →
//! `session/exit`. Interactive follow-ups issue new `session/prompt` turns on
//! the same session.

use crate::acp::types::*;
use crate::agent::process_control::{
    WorkerCallbackContext, WorkerFollowUp, WorkerOperationContext, WorkerResultTarget,
};
use crate::agent::worker::WorkerTranscriptSnapshot;
use crate::config::AcpPermissionMode;
use crate::secrets::scrub::SecretScanMode;
use crate::secrets::store::SecretsStore;
use crate::{AgentId, ChannelId, ProcessEvent, WorkerId};

use anyhow::{Context as _, bail};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, mpsc};

/// Result of an ACP worker run.
pub struct AcpWorkerResult {
    pub result_text: String,
}

/// An ACP-backed worker that drives a coding session via subprocess.
pub struct AcpWorker {
    pub id: WorkerId,
    pub channel_id: Option<ChannelId>,
    pub agent_id: AgentId,
    pub task: String,
    pub directory: PathBuf,
    pub command: String,
    pub args: Vec<String>,
    pub permission_mode: AcpPermissionMode,
    pub prompt_timeout: Duration,
    pub event_tx: broadcast::Sender<ProcessEvent>,
    /// Input channel for interactive follow-ups.
    pub input_rx: Option<mpsc::Receiver<WorkerFollowUp>>,
    /// System prompt prepended to each prompt turn.
    pub system_prompt: Option<String>,
    /// Secrets store for exact-match scrubbing of tool secret values.
    pub secrets_store: Option<Arc<SecretsStore>>,
    /// Secret scan mode for the regex leak-detection layer (Layer 2).
    /// Mirrors the agent's `[agents.sandbox] secret_scanner` config; exact
    /// stored-secret scrubbing always runs regardless of this mode.
    pub secret_scan_mode: crate::secrets::scrub::SecretScanMode,
    /// Shared subprocess registry (when provided) — the subprocess is tracked
    /// under this worker's id so group cancellation can kill it even while
    /// the stdio driver is mid-wait. Ownership of the `Child` stays here.
    pub child_registry: Option<Arc<crate::supervisor::ChildRegistry>>,
    pub transcript_snapshot: WorkerTranscriptSnapshot,
    pub callback: WorkerCallbackContext,
    pub initial_operation: Option<WorkerOperationContext>,
    pub process_control_registry: Arc<crate::agent::process_control::ProcessControlRegistry>,
    /// Owned subprocess; killed on drop so a cancelled worker never orphans
    /// the agent process.
    child: Option<Child>,
}

impl AcpWorker {
    /// Create a new ACP worker.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
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

    /// Attach the shared subprocess registry so the spawned agent process is
    /// tracked under this worker's id.
    pub fn with_child_registry(mut self, registry: Arc<crate::supervisor::ChildRegistry>) -> Self {
        self.child_registry = Some(registry);
        self
    }

    /// Create a new interactive ACP worker.
    #[allow(clippy::too_many_arguments)]
    pub fn new_interactive(
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

    /// Set the system prompt injected into each prompt turn.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the secrets store for exact-match scrubbing of tool secret values.
    pub fn with_secrets_store(mut self, store: Arc<SecretsStore>) -> Self {
        self.secrets_store = Some(store);
        self
    }

    /// Set the secret leak scan mode for this worker's egress scrubbing.
    pub fn with_secret_scan_mode(mut self, mode: SecretScanMode) -> Self {
        self.secret_scan_mode = mode;
        self
    }

    pub fn transcript_snapshot(&self) -> WorkerTranscriptSnapshot {
        self.transcript_snapshot.clone()
    }

    /// Send a status update through the agent-owned worker registry.
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

    /// Scrub tool secret values from text, replacing each with `[REDACTED:<name>]`.
    fn scrub_text(&self, text: &str) -> String {
        match &self.secrets_store {
            Some(store) => crate::secrets::scrub::scrub_with_store(text, store, &self.agent_id),
            None => text.to_string(),
        }
    }

    /// Build the full prompt text for a turn: system prompt first, then the
    /// user's task/follow-up.
    fn build_prompt(&self, user_text: &str) -> String {
        match &self.system_prompt {
            Some(system) => format!("{system}\n\n{user_text}"),
            None => user_text.to_string(),
        }
    }

    /// Run the worker: spawn the agent subprocess, initialize the protocol,
    /// open a session, send the task, and monitor until completion.
    pub async fn run(mut self) -> anyhow::Result<AcpWorkerResult> {
        // Ensure the agent's working directory exists before spawning, so the
        // subprocess can be launched even when the task has not created the
        // directory yet (e.g. a channel that hands the whole job to one ACP
        // worker instead of first running a mkdir builtin worker).
        if let Err(error) = tokio::fs::create_dir_all(&self.directory).await {
            anyhow::bail!(
                "failed to create ACP agent working dir '{}': {error}",
                self.directory.display()
            );
        }
        let mut child = Command::new(&self.command)
            .args(&self.args)
            .current_dir(&self.directory)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to spawn ACP agent '{}' in '{}'",
                    self.command,
                    self.directory.display()
                )
            })?;

        let mut stdin = child.stdin.take().context("ACP agent stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("ACP agent stdout unavailable")?;
        self.child = Some(child);

        if let Some(registry) = &self.child_registry
            && let Some(child) = self.child.as_ref()
        {
            registry
                .register(&self.id.to_string(), child, &self.command)
                .await?;
        }

        let mut reader = BufReader::new(stdout);

        self.send_status("initializing ACP session").await;
        let agent_version = initialize(&mut stdin, &mut reader, &self.agent_id).await?;
        tracing::info!(
            worker_id = %self.id,
            agent_version = %agent_version,
            command = %self.command,
            "ACP agent initialized"
        );

        self.send_status("creating session").await;
        let session_id = create_session(&mut stdin, &mut reader, &self.directory).await?;
        tracing::info!(worker_id = %self.id, session_id = %session_id, "ACP session created");

        // omp does not auto-generate a title in acp mode, so without this the
        // session would show up untitled in `omp -r`. Patch the title line of
        // the session file on disk with the task's first sentence (best-effort,
        // never fails the worker) and surface the session id so the user can
        // resume it from anywhere with `omp -r <id>`.
        let short_id = session_id.chars().take(8).collect::<String>();
        self.send_status(&format!(
            "ACP session ready — resume with `omp -r {short_id}`"
        ))
        .await;
        self.patch_omp_session_title(&session_id, &self.task).await;

        let prompt = self.build_prompt(&self.task);
        let result_text = self
            .run_turn(&mut stdin, &mut reader, &session_id, &prompt, true)
            .await?;

        // Interactive follow-up loop.
        if let Some(mut input_rx) = self.input_rx.take() {
            let scrubbed = self.scrub_text(&result_text);
            let scrubbed =
                crate::secrets::scrub::scrub_leaks_with_mode(&scrubbed, self.secret_scan_mode);
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
                let scrubbed =
                    crate::secrets::scrub::scrub_leaks_with_mode(&scrubbed, self.secret_scan_mode);
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

        self.send_status("exiting").await;
        let exit_result = exit_session(&mut stdin, &mut reader, &session_id).await;
        if let Err(error) = exit_result {
            tracing::debug!(worker_id = %self.id, %error, "ACP session/exit failed (agent may have exited)");
        }

        self.send_status("completed").await;

        Ok(AcpWorkerResult { result_text })
    }

    /// Run a single prompt turn, accumulating text from `session/update`
    /// notifications until the turn completes (or times out).
    async fn run_turn(
        &self,
        stdin: &mut ChildStdin,
        reader: &mut BufReader<ChildStdout>,
        session_id: &str,
        prompt: &str,
        is_initial: bool,
    ) -> anyhow::Result<String> {
        let request = build_session_prompt_request(1, session_id, prompt);
        write_line(stdin, &request).await?;

        let mut accumulated = String::new();
        let mut saw_update = false;
        let timeout = self.prompt_timeout.max(Duration::from_secs(30));

        loop {
            let line = tokio::time::timeout(timeout, read_line(reader)).await;
            let line = match line {
                Err(_) => {
                    bail!(
                        "ACP prompt turn timed out after {} seconds{}",
                        timeout.as_secs(),
                        if is_initial { "" } else { " (follow-up)" }
                    )
                }
                Ok(Ok(line)) => line,
                Ok(Err(error)) => {
                    bail!("ACP agent stdout closed mid-turn: {error}");
                }
            };

            let Ok(message) = serde_json::from_str::<IncomingMessage>(&line) else {
                // Non-JSON lines from the agent (warnings, logs) are ignored.
                tracing::trace!(worker_id = %self.id, line = %line, "ignoring non-JSON ACP line");
                continue;
            };

            match message {
                IncomingMessage::Response {
                    id, result, error, ..
                } => {
                    if id == RequestId::Number(1) {
                        if let Some(error) = error {
                            bail!(
                                "ACP session/prompt error: {} ({})",
                                error.message,
                                error.code
                            );
                        }
                        // The prompt response arrives after the streamed
                        // updates; the accumulated text is the result.
                        if !saw_update
                            && accumulated.trim().is_empty()
                            && let Some(result) = result
                            && let Some(text) = result.get("text").and_then(|v| v.as_str())
                        {
                            accumulated = text.to_string();
                        }
                        return Ok(accumulated);
                    }
                }
                IncomingMessage::Notification { method, params, .. } => {
                    if method == "session/update" {
                        let Some(update) = params.as_ref().and_then(SessionUpdate::parse) else {
                            continue;
                        };
                        if update.session_id != session_id {
                            continue;
                        }
                        saw_update = true;
                        if let Some(content) = &update.content
                            && let Some(text) = content.as_text()
                            && !text.is_empty()
                        {
                            accumulated.push_str(text);
                            accumulated.push('\n');
                            let scrubbed = self.scrub_text(&accumulated);
                            if let Some(leak) = crate::secrets::scrub::scan_for_leaks_with_mode(
                                &scrubbed,
                                self.secret_scan_mode,
                            ) {
                                tracing::warn!(
                                    worker_id = %self.id,
                                    leak_prefix = %&leak[..leak.floor_char_boundary(leak.len().min(8))],
                                    "potential secret detected in ACP worker output"
                                );
                            }
                        }
                        match update.status {
                            Some(SessionStatus::Completed) => {
                                return Ok(accumulated.trim_end().to_string());
                            }
                            Some(SessionStatus::Cancelled) => {
                                bail!("ACP session cancelled by agent");
                            }
                            Some(SessionStatus::Error) => {
                                bail!("ACP session reported an error");
                            }
                            _ => {}
                        }
                    }
                }
                IncomingMessage::Request {
                    id, method, params, ..
                } => {
                    self.handle_agent_request(stdin, &id, &method, params.as_ref())
                        .await?;
                }
            }
        }
    }

    /// Best-effort: write a human-readable title into the first line of the
    /// omp session file on disk (`~/.omp/agent/sessions/<cwd>/<ts>_<id>.jsonl`)
    /// without changing the line's total byte length. omp pads the title line
    /// and updates it in place, and title auto-generation does not run in acp
    /// mode — so without this patch every ACP session appears untitled in the
    /// `omp -r` picker. Any failure is logged and ignored; the worker never
    /// fails because of this.
    async fn patch_omp_session_title(&self, session_id: &str, task: &str) {
        let Some(root) = omp_sessions_root() else {
            tracing::debug!(worker_id = %self.id, "omp sessions root unavailable; skipping title patch");
            return;
        };
        let title = title_from_task(task);
        let Ok(mut dir) = tokio::fs::read_dir(&root).await else {
            tracing::debug!(worker_id = %self.id, root = %root.display(), "omp sessions root missing; skipping title patch");
            return;
        };
        while let Ok(Some(entry)) = dir.next_entry().await {
            if !entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let Ok(mut sub) = tokio::fs::read_dir(entry.path()).await else {
                continue;
            };
            while let Ok(Some(file)) = sub.next_entry().await {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let name = file.file_name().to_string_lossy().into_owned();
                if !name.contains(session_id) {
                    continue;
                }
                if let Err(error) = patch_title_line(&path, &title).await {
                    tracing::debug!(
                        worker_id = %self.id,
                        session_id,
                        %error,
                        "failed to patch omp session title"
                    );
                } else {
                    tracing::info!(
                        worker_id = %self.id,
                        session_id,
                        title = %title,
                        "patched omp session title"
                    );
                }
                return;
            }
        }
        tracing::debug!(worker_id = %self.id, session_id, "omp session file not found; title left untouched");
    }

    /// Answer an incoming request from the agent. ACP agents use
    /// `permission/request` to ask for tool approval; we answer according to
    /// the configured permission mode. Unknown methods get a JSON-RPC error.
    async fn handle_agent_request(
        &self,
        stdin: &mut ChildStdin,
        id: &RequestId,
        method: &str,
        params: Option<&serde_json::Value>,
    ) -> anyhow::Result<()> {
        match method {
            "permission/request" => {
                let Some(permission) = params.and_then(PermissionRequest::parse) else {
                    write_line(stdin, &build_error_response(id, -32602, "invalid params")).await?;
                    return Ok(());
                };
                let outcome = match self.permission_mode {
                    AcpPermissionMode::AutoAccept => "allowed",
                    AcpPermissionMode::AutoReject => "rejected",
                };
                let description = permission
                    .permission
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                tracing::info!(
                    worker_id = %self.id,
                    permission_id = %permission.permission_id,
                    permission_type = %description,
                    outcome,
                    "ACP permission request"
                );
                let interaction_target = self
                    .process_control_registry
                    .worker_snapshot_for_callback(self.callback)
                    .await
                    .and_then(|snapshot| {
                        snapshot
                            .active_operation
                            .map(|operation| operation.result_target)
                    })
                    .unwrap_or(WorkerResultTarget::None);
                let event_tx = self.event_tx.clone();
                let event = ProcessEvent::WorkerPermission {
                    agent_id: self.agent_id.clone(),
                    worker_id: self.id,
                    worker_registration_id: self.callback.registration_id,
                    interaction_target,
                    channel_id: self.channel_id.clone(),
                    permission_id: permission.permission_id.clone(),
                    description: description.to_string(),
                    patterns: Vec::new(),
                };
                self.process_control_registry
                    .run_if_worker_state(
                        self.callback,
                        crate::agent::process_control::WorkerRuntimeState::Running,
                        move || {
                            event_tx.send(event).ok();
                        },
                    )
                    .await;
                let response = build_permission_response(id, outcome);
                write_line(stdin, &response).await?;
                Ok(())
            }
            other => {
                tracing::debug!(
                    worker_id = %self.id,
                    method = %other,
                    "unsupported ACP request from agent"
                );
                write_line(
                    stdin,
                    &build_error_response(id, -32601, &format!("method not found: {other}")),
                )
                .await?;
                Ok(())
            }
        }
    }
}

impl Drop for AcpWorker {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
    }
}

/// Write a single newline-delimited JSON message to the agent.
async fn write_line(stdin: &mut ChildStdin, value: &serde_json::Value) -> anyhow::Result<()> {
    stdin.write_all(&serialize_line(value)).await?;
    stdin.flush().await?;
    Ok(())
}

/// Read a single newline-delimited line from the agent.
async fn read_line(reader: &mut BufReader<ChildStdout>) -> anyhow::Result<String> {
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await?;
    if bytes == 0 {
        bail!("ACP agent stdout closed");
    }
    Ok(line.trim_end().to_string())
}

/// Send `initialize` and wait for the response, returning the agent version.
async fn initialize(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    agent_id: &AgentId,
) -> anyhow::Result<String> {
    let request = build_initialize_request(0, "spacebot", env!("CARGO_PKG_VERSION"));
    write_line(stdin, &request).await?;

    let timeout = Duration::from_secs(30);
    loop {
        let line = tokio::time::timeout(timeout, read_line(reader))
            .await
            .map_err(|_| anyhow::anyhow!("ACP agent did not respond to initialize within 30s"))??;

        let Ok(message) = serde_json::from_str::<IncomingMessage>(&line) else {
            tracing::trace!(agent_id = %agent_id, line = %line, "ignoring non-JSON ACP line during initialize");
            continue;
        };

        match message {
            IncomingMessage::Response {
                id, result, error, ..
            } => {
                if id == RequestId::Number(0) {
                    if let Some(error) = error {
                        bail!("ACP initialize error: {} ({})", error.message, error.code);
                    }
                    let version = result
                        .as_ref()
                        .and_then(|r| r.get("agentVersion"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    return Ok(version);
                }
            }
            IncomingMessage::Request { id, method, .. } => {
                // Before the session exists, the only legitimate incoming
                // request is a permission request (e.g. auth). Reject it.
                if method == "permission/request" {
                    write_line(stdin, &build_permission_response(&id, "rejected")).await?;
                    continue;
                }
                write_line(
                    stdin,
                    &build_error_response(&id, -32601, &format!("method not found: {method}")),
                )
                .await?;
            }
            IncomingMessage::Notification { .. } => {}
        }
    }
}

/// Send `session/new` and wait for the response, returning the session id.
async fn create_session(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    directory: &Path,
) -> anyhow::Result<String> {
    let request = build_session_new_request(1, &directory.to_string_lossy());
    write_line(stdin, &request).await?;

    let timeout = Duration::from_secs(30);
    loop {
        let line = tokio::time::timeout(timeout, read_line(reader))
            .await
            .map_err(|_| {
                anyhow::anyhow!("ACP agent did not respond to session/new within 30s")
            })??;

        let Ok(message) = serde_json::from_str::<IncomingMessage>(&line) else {
            continue;
        };

        match message {
            IncomingMessage::Response {
                id, result, error, ..
            } => {
                if id == RequestId::Number(1) {
                    if let Some(error) = error {
                        bail!("ACP session/new error: {} ({})", error.message, error.code);
                    }
                    return result
                        .as_ref()
                        .and_then(|r| r.get("sessionId"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .ok_or_else(|| {
                            anyhow::anyhow!("ACP session/new response missing sessionId")
                        });
                }
            }
            IncomingMessage::Request { id, method, .. } => {
                write_line(
                    stdin,
                    &build_error_response(&id, -32601, &format!("method not found: {method}")),
                )
                .await?;
            }
            IncomingMessage::Notification { .. } => {}
        }
    }
}

/// Resolve the omp session storage root (`~/.omp/agent/sessions`), honoring
/// the `PI_CODING_AGENT_SESSION_DIR` override omp itself supports.
fn omp_sessions_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    let home = dirs::home_dir()?;
    Some(home.join(".omp/agent/sessions"))
}

/// Derive a short, recognizable title from the task text: the first non-empty
/// line, stripped of leading markdown heading markers, capped at 80 chars.
fn title_from_task(task: &str) -> String {
    let first = task
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("ACP task");
    let stripped = first.trim_start_matches('#').trim();
    let mut title = stripped.chars().take(80).collect::<String>();
    if stripped.chars().count() > 80 {
        title.push('…');
    }
    if title.is_empty() {
        title = "ACP task".to_string();
    }
    title
}

/// Rewrite the first (title) line of an omp session file in place, keeping
/// the line's total byte length identical so omp's in-place title updates
/// keep working. The `pad` field absorbs the length difference.
async fn patch_title_line(path: &Path, title: &str) -> anyhow::Result<()> {
    use std::io::SeekFrom;
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .await
        .with_context(|| format!("open session file '{}'", path.display()))?;

    // Read just the first line (omp stores the title object there). The line
    // is short and fixed-padded, so a bounded read is enough.
    let mut head = vec![0u8; 4096];
    let n = file.read(&mut head).await?;
    if n == 0 {
        return Ok(());
    }
    let newline = head[..n].iter().position(|b| *b == b'\n');
    let line_bytes = match newline {
        Some(pos) => &head[..pos],
        None => &head[..n],
    };
    let trimmed = std::str::from_utf8(line_bytes)?.trim_end();
    let orig_len = trimmed.len();
    let value: serde_json::Value = serde_json::from_str(trimmed)?;

    // omp pads the title line to a fixed length and rewrites it in place, so
    // the patched line must keep its exact original byte length. The `pad`
    // field absorbs the difference. Build the line manually in omp's key
    // order (type, v, title, updatedAt, pad) so only the title text and pad
    // length change — serde's BTreeMap would reorder keys and shift the line.
    let updated_at = value
        .get("updatedAt")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Title must fit in the space the original pad provided (plus whatever
    // room the original empty title leaves). Trim until it does.
    let suffix = "\"}";
    let mut fitted_title = title.to_string();
    let mut new_line = loop {
        let prefix = format!(
            "{{\"type\":\"title\",\"v\":1,\"title\":{},\"updatedAt\":{},\"pad\":\"",
            serde_json::to_string(&fitted_title)?,
            serde_json::to_string(updated_at)?,
        );
        let fixed_len = prefix.len() + suffix.len();
        let pad_len = orig_len.saturating_sub(fixed_len);
        let candidate = format!("{prefix}{}{suffix}", " ".repeat(pad_len));
        if candidate.len() <= orig_len || fitted_title.chars().count() <= 1 {
            break candidate;
        }
        fitted_title.pop();
    };

    if new_line.len() != orig_len {
        tracing::debug!(
            path = %path.display(),
            orig_len,
            new_len = new_line.len(),
            "omp title line length mismatch; keeping original length"
        );
    }
    new_line.push('\n');

    file.seek(SeekFrom::Start(0)).await?;
    file.write_all(new_line.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

/// Send `session/exit`. The agent may close stdout immediately after; treat
/// that as success.
async fn exit_session(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    session_id: &str,
) -> anyhow::Result<()> {
    let request = build_session_exit_request(2, session_id);
    write_line(stdin, &request).await?;

    let timeout = Duration::from_secs(10);
    loop {
        match tokio::time::timeout(timeout, read_line(reader)).await {
            Err(_) => return Ok(()),
            Ok(Err(_)) => return Ok(()), // agent exited, that's fine
            Ok(Ok(line)) => {
                let Ok(message) = serde_json::from_str::<IncomingMessage>(&line) else {
                    continue;
                };
                match message {
                    IncomingMessage::Response { id, .. } => {
                        if id == RequestId::Number(2) {
                            return Ok(());
                        }
                    }
                    IncomingMessage::Request { id, method, .. } => {
                        write_line(
                            stdin,
                            &build_error_response(
                                &id,
                                -32601,
                                &format!("method not found: {method}"),
                            ),
                        )
                        .await?;
                    }
                    IncomingMessage::Notification { .. } => {}
                }
            }
        }
    }
}
