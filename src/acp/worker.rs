//! ACP-backed worker: drives an external coding agent over the Agent Client
//! Protocol (JSON-RPC over stdio).
//!
//! Instead of running a Rig agent loop, this worker spawns a subprocess for
//! any ACP-compatible CLI (Claude Code, Codex, Cursor CLI, etc.) and drives
//! it through `initialize` → `session/new` → `session/prompt` →
//! `session/exit`. Interactive follow-ups issue new `session/prompt` turns on
//! the same session.

use crate::acp::types::*;
use crate::agent::worker::WorkerTranscriptSnapshot;
use crate::config::types::AcpPermissionMode;
use crate::secrets::store::SecretsStore;
use crate::{AgentId, ChannelId, ProcessEvent, WorkerId};

use anyhow::{Context as _, bail};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use uuid::Uuid;

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
    pub input_rx: Option<mpsc::Receiver<String>>,
    /// System prompt prepended to each prompt turn.
    pub system_prompt: Option<String>,
    /// Secrets store for exact-match scrubbing of tool secret values.
    pub secrets_store: Option<Arc<SecretsStore>>,
    pub transcript_snapshot: WorkerTranscriptSnapshot,
    /// Owned subprocess; killed on drop so a cancelled worker never orphans
    /// the agent process.
    child: Option<Child>,
}

impl AcpWorker {
    /// Create a new ACP worker.
    pub fn new(
        channel_id: Option<ChannelId>,
        agent_id: AgentId,
        task: impl Into<String>,
        directory: PathBuf,
        command: String,
        args: Vec<String>,
        permission_mode: AcpPermissionMode,
        prompt_timeout: Duration,
        event_tx: broadcast::Sender<ProcessEvent>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
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
            transcript_snapshot: crate::agent::worker::new_worker_transcript_snapshot(),
            child: None,
        }
    }

    /// Create a new interactive ACP worker.
    pub fn new_interactive(
        channel_id: Option<ChannelId>,
        agent_id: AgentId,
        task: impl Into<String>,
        directory: PathBuf,
        command: String,
        args: Vec<String>,
        permission_mode: AcpPermissionMode,
        prompt_timeout: Duration,
        event_tx: broadcast::Sender<ProcessEvent>,
    ) -> (Self, mpsc::Sender<String>) {
        let (input_tx, input_rx) = mpsc::channel(32);
        let mut worker = Self::new(
            channel_id,
            agent_id,
            task,
            directory,
            command,
            args,
            permission_mode,
            prompt_timeout,
            event_tx,
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

    pub fn transcript_snapshot(&self) -> WorkerTranscriptSnapshot {
        self.transcript_snapshot.clone()
    }

    /// Send a status update via the process event bus.
    fn send_status(&self, status: &str) {
        let _ = self.event_tx.send(ProcessEvent::WorkerStatus {
            agent_id: self.agent_id.clone(),
            worker_id: self.id,
            channel_id: self.channel_id.clone(),
            status: status.to_string(),
        });
    }

    /// Send an idle event to mark this worker as waiting for follow-up input.
    fn send_idle(&self) {
        let _ = self.event_tx.send(ProcessEvent::WorkerIdle {
            agent_id: self.agent_id.clone(),
            worker_id: self.id,
            channel_id: self.channel_id.clone(),
        });
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

        let mut reader = BufReader::new(stdout);

        self.send_status("initializing ACP session");
        let agent_version = initialize(&mut stdin, &mut reader, &self.agent_id).await?;
        tracing::info!(
            worker_id = %self.id,
            agent_version = %agent_version,
            command = %self.command,
            "ACP agent initialized"
        );

        self.send_status("creating session");
        let session_id = create_session(&mut stdin, &mut reader, &self.directory).await?;
        tracing::info!(worker_id = %self.id, session_id = %session_id, "ACP session created");

        let prompt = self.build_prompt(&self.task);
        let result_text = self
            .run_turn(&mut stdin, &mut reader, &session_id, &prompt, true)
            .await?;

        // Interactive follow-up loop.
        if let Some(mut input_rx) = self.input_rx.take() {
            let scrubbed = self.scrub_text(&result_text);
            let scrubbed = crate::secrets::scrub::scrub_leaks(&scrubbed);
            let _ = self.event_tx.send(ProcessEvent::WorkerInitialResult {
                agent_id: self.agent_id.clone(),
                worker_id: self.id,
                channel_id: self.channel_id.clone(),
                result: scrubbed,
            });
            self.send_status("waiting for follow-up");
            self.send_idle();

            while let Some(follow_up) = input_rx.recv().await {
                self.send_status("processing follow-up");
                let follow_up_prompt = self.build_prompt(&follow_up);
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
                        self.send_status("failed");
                        break;
                    }
                };
                if !turn_text.is_empty() {
                    let scrubbed = self.scrub_text(&turn_text);
                    let scrubbed = crate::secrets::scrub::scrub_leaks(&scrubbed);
                    let _ = self.event_tx.send(ProcessEvent::WorkerInitialResult {
                        agent_id: self.agent_id.clone(),
                        worker_id: self.id,
                        channel_id: self.channel_id.clone(),
                        result: scrubbed,
                    });
                }
                self.send_status("waiting for follow-up");
                self.send_idle();
            }
        }

        self.send_status("exiting");
        let exit_result = exit_session(&mut stdin, &mut reader, &session_id).await;
        if let Err(error) = exit_result {
            tracing::debug!(worker_id = %self.id, %error, "ACP session/exit failed (agent may have exited)");
        }

        self.send_status("completed");

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
                IncomingMessage::Response { id, result, error } => {
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
                        if !saw_update && accumulated.trim().is_empty() {
                            if let Some(result) = result {
                                if let Some(text) = result.get("text").and_then(|v| v.as_str()) {
                                    accumulated = text.to_string();
                                }
                            }
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
                        if let Some(content) = &update.content {
                            if let Some(text) = content.as_text()
                                && !text.is_empty()
                            {
                                accumulated.push_str(text);
                                accumulated.push('\n');
                                let scrubbed = self.scrub_text(&accumulated);
                                if let Some(leak) = crate::secrets::scrub::scan_for_leaks(&scrubbed)
                                {
                                    tracing::warn!(
                                        worker_id = %self.id,
                                        leak_prefix = %&leak[..leak.len().min(8)],
                                        "potential secret detected in ACP worker output"
                                    );
                                }
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
                let _ = self.event_tx.send(ProcessEvent::WorkerPermission {
                    agent_id: self.agent_id.clone(),
                    worker_id: self.id,
                    channel_id: self.channel_id.clone(),
                    permission_id: permission.permission_id.clone(),
                    description: description.to_string(),
                    patterns: Vec::new(),
                });
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
            IncomingMessage::Response { id, result, error } => {
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
            IncomingMessage::Request {
                id, method, params, ..
            } => {
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
    directory: &PathBuf,
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
            IncomingMessage::Response { id, result, error } => {
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
