//! The autonomy channel: the agent's process for self-directed work.
//!
//! One channel wakes on a configured interval (or when wake events are
//! pending), surveys task state, enriches and proposes work according to the
//! configured [`AutonomyLevel`], executes user-approved tasks at level `act`,
//! records a run summary via `autonomy_complete`, and exits. See
//! `docs/design-docs/autonomy.md` and `docs/design-docs/wakes.md`.

use crate::agent::channel::{Channel, ChannelKind};
use crate::config::{AutonomyConfig, AutonomyLevel};
use crate::conversation::settings::{DelegationMode, ResolvedConversationSettings};
use crate::prompts::engine::{AutonomyRunHistoryView, AutonomyWakeEventView};
use crate::tasks::{Task, TaskListFilter, TaskStatus};
use crate::wakes::{AutonomyRunStatus, AutonomyRunStore};
use crate::{AgentDeps, InboundMessage, MessageContent, RoutedResponse};

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Conversation id (and channel id) for the autonomy channel. One per agent.
pub const AUTONOMY_CONVERSATION_ID: &str = "autonomy";

/// Retention window for consumed wake events, pruned after each run.
const WAKE_EVENT_RETENTION_DAYS: u32 = 30;

/// Maximum pending wake events pulled into a single run's context.
const WAKE_EVENT_BATCH_LIMIT: i64 = 200;

/// Grace period after the hard-timeout wrap-up message before aborting.
const HARD_TIMEOUT_GRACE_SECS: u64 = 60;

/// Retry budget for the completion contract — the same budget the
/// memory-persistence contract uses.
pub const AUTONOMY_CONTRACT_MAX_RETRIES: usize =
    crate::hooks::SpacebotHook::MEMORY_PERSISTENCE_CONTRACT_MAX_RETRIES;

/// Fallback summary recorded when a run ends without calling `autonomy_complete`.
pub const AUTONOMY_FALLBACK_SUMMARY: &str = "run ended without summary";

/// Shared state between the run driver, the channel, and the
/// `autonomy_complete` tool for a single autonomy run.
#[derive(Debug, Clone)]
pub struct AutonomyRunHandle {
    pub run_id: String,
    pub store: Arc<AutonomyRunStore>,
    completed: Arc<AtomicBool>,
<<<<<<< ours
    finish_request: Arc<Mutex<Option<AutonomyFinishRequest>>>,
=======
    state: Arc<Mutex<AutonomyRunState>>,
    changed: Arc<Notify>,
}

#[derive(Debug, Default)]
struct AutonomyRunState {
    finish_request: Option<AutonomyFinishRequest>,
    active_children: HashSet<AutonomyChild>,
    quiescent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutonomyChild {
    Branch(crate::BranchId),
    WorkerOperation {
        worker_id: crate::WorkerId,
        operation_id: crate::agent::process_control::WorkerOperationId,
    },
>>>>>>> theirs
}

#[derive(Debug, Clone)]
pub struct AutonomyFinishRequest {
    pub summary: String,
    pub actions: Vec<crate::wakes::AutonomyAction>,
}

impl AutonomyRunHandle {
    pub fn new(run_id: String, store: Arc<AutonomyRunStore>) -> Self {
        Self {
            run_id,
            store,
            completed: Arc::new(AtomicBool::new(false)),
            finish_request: Arc::new(Mutex::new(None)),
        }
    }

    /// Record that `autonomy_complete` was called for this run.
    pub fn mark_completed(&self) {
        self.completed.store(true, Ordering::Release);
    }

    pub fn completed(&self) -> bool {
        self.completed.load(Ordering::Acquire)
    }

    /// Store the first finish request so duplicate tool calls cannot replace
    /// the summary that will be committed after child workers settle.
    pub fn request_finish(&self, request: AutonomyFinishRequest) -> bool {
        let mut finish_request = self
            .finish_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if finish_request.is_some() {
            return false;
        }
        *finish_request = Some(request);
        true
    }

    pub fn finish_requested(&self) -> bool {
        let finish_request = self
            .finish_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        finish_request.is_some()
    }

    pub fn finish_request(&self) -> Option<AutonomyFinishRequest> {
        let finish_request = self
            .finish_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        finish_request.clone()
    }
}

/// Whether an autonomy run is due right now.
///
/// Pure over its inputs so the decision is unit-testable: a run is due when
/// the level is on, the current hour is inside the active window, and either
/// unconsumed wake events are pending or the interval has elapsed since the
/// last run (a never-run agent is immediately due).
pub fn autonomy_run_due(
    level: AutonomyLevel,
    now: chrono::DateTime<chrono::Utc>,
    last_run_started_at: Option<chrono::DateTime<chrono::Utc>>,
    pending_wake_events: i64,
    active_hours: Option<(u8, u8)>,
    current_hour: u8,
    interval_secs: u64,
) -> bool {
    if level == AutonomyLevel::Off {
        return false;
    }
    if let Some((start, end)) = active_hours
        && !crate::cron::scheduler::hour_in_active_window(current_hour, start, end)
    {
        return false;
    }
    if pending_wake_events > 0 {
        return true;
    }
    match last_run_started_at {
        None => true,
        Some(last) => {
            now.signed_duration_since(last) >= chrono::Duration::seconds(interval_secs as i64)
        }
    }
}

/// Whether a task belongs in this agent's autonomy context.
///
/// Assigned tasks are visible only to their assignee; unassigned tasks are
/// visible only when the agent claims unowned work.
pub fn task_visible_to_agent(task: &Task, agent_id: &str, claim_unowned: bool) -> bool {
    match task.assigned_agent_id.as_deref() {
        Some(assigned) => assigned == agent_id,
        None => claim_unowned,
    }
}

/// Cortex-tick check: start an autonomy run when one is due.
///
/// The run row is inserted before the run task is spawned so the next tick's
/// `has_active_run` guard sees it — the cortex tick is serial, which makes
/// this the single-flight gate.
pub async fn maybe_run_autonomy(deps: &AgentDeps) {
    let config = **deps.runtime_config.autonomy.load();
    // The instance ceiling caps the per-agent dial without overwriting it:
    // the run executes at the intersection of the two levels.
    let config = AutonomyConfig {
        level: config.level.min(**deps.autonomy_ceiling.load()),
        ..config
    };
    if config.level == AutonomyLevel::Off {
        return;
    }

    // A pause is an emergency stop on new work, and a self-directed run is
    // the most new work the agent can start.
    if deps.pause_reason().is_some() {
        return;
    }

    let stale_after_secs = config.timeout_secs.saturating_mul(2).max(60);
    match deps
        .autonomy_run_store
        .has_active_run(stale_after_secs)
        .await
    {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(%error, "failed to check for active autonomy run");
            return;
        }
    }

    let last_run_started_at = match deps.autonomy_run_store.last_run_started_at().await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "failed to read last autonomy run start");
            return;
        }
    };
    let pending_wake_events = match deps.wake_event_store.pending_count().await {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(%error, "failed to count pending wake events");
            return;
        }
    };
    let (current_hour, _timezone) =
        crate::cron::scheduler::current_hour_and_timezone(&deps.runtime_config);

    if !autonomy_run_due(
        config.level,
        chrono::Utc::now(),
        last_run_started_at,
        pending_wake_events,
        config.active_hours,
        current_hour,
        config.interval_secs,
    ) {
        return;
    }

    let run_id = match deps.autonomy_run_store.begin_run().await {
        Ok(run_id) => run_id,
        Err(error) => {
            tracing::warn!(%error, "failed to begin autonomy run");
            return;
        }
    };

    tracing::info!(
        agent_id = %deps.agent_id,
        run_id = %run_id,
        level = %config.level,
        pending_wake_events,
        "starting autonomy run"
    );

    let deps = deps.clone();
    tokio::spawn(async move {
        if let Err(error) = run_autonomy_channel(&deps, run_id.clone(), config).await {
            tracing::error!(%error, run_id = %run_id, "autonomy run failed");
            match deps
                .autonomy_run_store
                .finish_run_status(
                    &run_id,
                    AutonomyRunStatus::Failed,
                    Some(&format!("run failed: {error}")),
                )
                .await
            {
                Ok(true) => {
                    publish_terminal_summary(&deps, &run_id, &format!("run failed: {error}"))
                }
                Ok(false) => {}
                Err(finish_error) => {
                    tracing::warn!(%finish_error, run_id = %run_id, "failed to record autonomy run failure");
                }
            }
        }
    });
}

/// Handle an external wake by checking whether an autonomy run is due.
pub async fn wake_one(deps: &AgentDeps) {
    maybe_run_autonomy(deps).await;
}

/// Execute a single autonomy run: consume pending wake events, assemble the
/// run briefing, drive the channel to completion under the soft/hard timeout,
/// and record the outcome.
pub async fn run_autonomy_channel(
    deps: &AgentDeps,
    run_id: String,
    config: AutonomyConfig,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(config.timeout_secs);
    // Consume pending wake events at run start. A crash after this point does
    // not replay events — crash semantics for tasks are handled by task
    // status, not event replay.
    let pending_events = deps
        .wake_event_store
        .pending(WAKE_EVENT_BATCH_LIMIT)
        .await?;
    let event_ids: Vec<String> = pending_events
        .iter()
        .map(|event| event.id.clone())
        .collect();
    if !event_ids.is_empty() {
        deps.wake_event_store.consume(&event_ids, &run_id).await?;
        deps.autonomy_run_store
            .set_wake_events(&run_id, &event_ids)
            .await?;
    }

    let briefing = build_run_briefing(deps, &config, &pending_events).await?;

    // Timeout prompts are rendered before the channel spawns so a template
    // failure surfaces immediately instead of mid-run. Config validation
    // guarantees warn < timeout.
    let remaining_secs = config.timeout_secs.saturating_sub(config.warn_secs).max(1);
    let (soft_warning_prompt, hard_timeout_prompt) = {
        let prompt_engine = deps.runtime_config.prompts.load();
        let soft = prompt_engine
            .render_system_autonomy_soft_warning(remaining_secs.div_ceil(60).max(1))
            .map_err(|error| anyhow::anyhow!("failed to render autonomy soft warning: {error}"))?;
        let hard = prompt_engine
            .render_system_autonomy_hard_timeout()
            .map_err(|error| anyhow::anyhow!("failed to render autonomy hard timeout: {error}"))?;
        (soft, hard)
    };

    let channel_id: crate::ChannelId = Arc::from(AUTONOMY_CONVERSATION_ID);
    let (response_tx, mut response_rx) = tokio::sync::mpsc::channel::<RoutedResponse>(32);
    // The autonomy channel has no delivery target — drain and drop anything
    // the channel tries to send so the bounded channel never backs up.
    tokio::spawn(async move { while response_rx.recv().await.is_some() {} });
    let event_rx = deps.event_tx.subscribe();

    // Direct tool access: the autonomy channel does not branch — it has no
    // user-facing context to protect, so it uses memory/execution tools
    // directly.
    let resolved_settings = ResolvedConversationSettings {
        delegation: DelegationMode::Direct,
        ..ResolvedConversationSettings::default()
    };

    let handle = AutonomyRunHandle::new(run_id.clone(), deps.autonomy_run_store.clone());

    let screenshot_dir = deps
        .runtime_config
        .workspace_dir
        .join(".spacebot")
        .join("screenshots");
    let logs_dir = deps
        .runtime_config
        .workspace_dir
        .join(".spacebot")
        .join("logs");

    let (channel, channel_tx) = Channel::new(
        channel_id.clone(),
        ChannelKind::Autonomy,
        deps.clone(),
        response_tx,
        event_rx,
        screenshot_dir,
        logs_dir,
        None, // autonomy channels don't share live transcript cache
        resolved_settings,
        None, // no cron outcome — delivery is not a concept here
        Some(handle.clone()),
    );

    let mut channel_handle = tokio::spawn(channel.run());

    let message = InboundMessage {
        id: uuid::Uuid::new_v4().to_string(),
        source: "autonomy".into(),
        adapter: None,
        conversation_id: AUTONOMY_CONVERSATION_ID.to_string(),
        sender_id: "system".into(),
        agent_id: Some(deps.agent_id.clone()),
        content: MessageContent::Text(briefing),
        timestamp: chrono::Utc::now(),
        metadata: HashMap::new(),
        formatted_author: None,
    };

    if let Err(error) = channel_tx.send(message).await {
        channel_handle.abort();
        anyhow::bail!("failed to send autonomy briefing to channel: {error}");
    }

    // Soft warning at warn_secs, hard timeout at timeout_secs.
    let warn_after = Duration::from_secs(config.warn_secs.min(config.timeout_secs).max(1));
    let mut timed_out = false;
    let first_phase = tokio::time::timeout(warn_after, &mut channel_handle).await;
    let join_result = match first_phase {
        Ok(join_result) => Some(join_result),
        Err(_elapsed) => {
            tracing::info!(
                run_id = %run_id,
                remaining_secs,
                "autonomy run reached soft warning, injecting wrap-up notice"
            );
            if channel_tx
                .send(system_message(deps, soft_warning_prompt))
                .await
                .is_err()
            {
                tracing::debug!(
                    run_id = %run_id,
                    "soft warning not delivered; autonomy channel already exited"
                );
            }

            match tokio::time::timeout(Duration::from_secs(remaining_secs), &mut channel_handle)
                .await
            {
                Ok(join_result) => Some(join_result),
                Err(_elapsed) => {
                    // Hard timeout: give the LLM one direct turn to record the
                    // run, mirroring the cron wrap-up pattern.
                    timed_out = true;
                    tracing::warn!(run_id = %run_id, "autonomy run hit hard timeout, sending wrap-up prompt");
                    channel_tx
                        .send(system_message(deps, hard_timeout_prompt))
                        .await
                        .ok();
                    drop(channel_tx);

                    let grace = Duration::from_secs(HARD_TIMEOUT_GRACE_SECS);
                    match tokio::time::timeout(grace, &mut channel_handle).await {
                        Ok(join_result) => Some(join_result),
                        Err(_elapsed) => {
                            channel_handle.abort();
                            if let Err(join_error) = (&mut channel_handle).await
                                && !join_error.is_cancelled()
                            {
                                tracing::warn!(
                                    run_id = %run_id,
                                    %join_error,
                                    "autonomy channel task failed after abort"
                                );
                            }
                            None
                        }
                    }
<<<<<<< ours
                }
            }
        }
    };

    let channel_failed = match join_result {
        Some(Ok(Ok(()))) => None,
        Some(Ok(Err(error))) => Some(format!("autonomy channel failed: {error}")),
        Some(Err(join_error)) => Some(format!("autonomy channel join failed: {join_error}")),
        None => None,
    };

    // A finish request closes dispatch before the channel exits. Wait for the
    // durable child rows instead of cancelling useful work to make closure fit
    // the parent deadline.
    let settled = settle_owned_children(&handle, deadline).await?;
=======
                },
            }
        }
    }

    let interrupted = active_epoch
        .as_ref()
        .map(|epoch| epoch.handle.clone())
        .or_else(|| run_slot.current());
    if deps.autonomy_control.should_preserve_idle_workers() {
        if let Some(handle) = &interrupted {
            for child in handle.active_children() {
                match child {
                    AutonomyChild::Branch(branch_id) => {
                        channel_control
                            .cancel_branch_with_reason(branch_id, "daemon restarting")
                            .await;
                    }
                    AutonomyChild::WorkerOperation { .. } => {}
                }
            }
        }
    } else {
        for child in interrupted
            .as_ref()
            .map(|handle| handle.active_children())
            .unwrap_or_default()
        {
            if let AutonomyChild::Branch(branch_id) = child {
                channel_control
                    .cancel_branch_with_reason(branch_id, "autonomy supervisor shutting down")
                    .await;
            }
        }
    }
>>>>>>> theirs

    // Record the run outcome after owned noninteractive children settled, or
    // after the configured hard deadline. The latter retains child attribution
    // and lets their normal terminal paths finish without parent cancellation.
    if let Some(request) = handle.finish_request() {
        let recorded = deps
            .autonomy_run_store
            .complete_run_if_children_settled(&run_id, &request.summary, &request.actions, !settled)
            .await?;
        if recorded {
            handle.mark_completed();
            publish_terminal_summary(deps, &run_id, &request.summary);
        }
    } else if !handle.completed() {
        if let Some(failure) = &channel_failed {
            let recorded = deps
                .autonomy_run_store
                .finish_run_status(&run_id, AutonomyRunStatus::Failed, Some(failure))
                .await?;
            if recorded {
                publish_terminal_summary(deps, &run_id, failure);
            }
        } else if timed_out {
            let recorded = deps
                .autonomy_run_store
                .finish_run_status(
                    &run_id,
                    AutonomyRunStatus::Timeout,
                    Some(AUTONOMY_FALLBACK_SUMMARY),
                )
                .await?;
            if recorded {
                publish_terminal_summary(deps, &run_id, AUTONOMY_FALLBACK_SUMMARY);
            }
        } else {
            // The channel-side contract retries were exhausted without a
            // completion call — record the run with a synthesized summary.
            let recorded = deps
                .autonomy_run_store
                .complete_run(&run_id, AUTONOMY_FALLBACK_SUMMARY, &[])
                .await?;
            if recorded {
                publish_terminal_summary(deps, &run_id, AUTONOMY_FALLBACK_SUMMARY);
            }
        }
    }

    if let Err(error) = deps
        .wake_event_store
        .prune_consumed(WAKE_EVENT_RETENTION_DAYS)
        .await
    {
        tracing::warn!(%error, "failed to prune consumed wake events");
    }

    if let Some(failure) = channel_failed {
        anyhow::bail!(failure);
    }

    tracing::info!(run_id = %run_id, timed_out, completed = handle.completed(), "autonomy run finished");
    Ok(())
}

/// The run table transition is the idempotency boundary. Only the caller that
/// changes a run from `running` may add its conclusion to the channel record.
fn publish_terminal_summary(deps: &AgentDeps, run_id: &str, summary: &str) {
    let channel_id: crate::ChannelId = Arc::from(AUTONOMY_CONVERSATION_ID);
    crate::conversation::ConversationLogger::new(deps.sqlite_pool.clone()).log_bot_message_with_id(
        &channel_id,
        &format!("autonomy-outcome:{run_id}"),
        summary,
    );
    if let Err(error) = deps
        .event_tx
        .send(crate::ProcessEvent::ChannelAssistantMessage {
            agent_id: deps.agent_id.clone(),
            channel_id,
            text: summary.to_string(),
        })
    {
        tracing::debug!(%error, "failed to emit autonomy outcome for live timeline");
    }
}

async fn settle_owned_children(
    handle: &AutonomyRunHandle,
    deadline: tokio::time::Instant,
) -> anyhow::Result<bool> {
    loop {
        if !handle
            .store
            .has_active_owned_children(&handle.run_id)
            .await?
        {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn system_message(deps: &AgentDeps, text: String) -> InboundMessage {
    InboundMessage {
        id: uuid::Uuid::new_v4().to_string(),
        source: "system".into(),
        adapter: None,
        conversation_id: AUTONOMY_CONVERSATION_ID.to_string(),
        sender_id: "system".into(),
        agent_id: Some(deps.agent_id.clone()),
        content: MessageContent::Text(text),
        timestamp: chrono::Utc::now(),
        metadata: HashMap::new(),
        formatted_author: None,
    }
}

/// Assemble the run briefing rendered from `autonomy_channel.md.j2`.
async fn build_run_briefing(
    deps: &AgentDeps,
    config: &AutonomyConfig,
    wake_events: &[crate::wakes::WakeEvent],
) -> anyhow::Result<String> {
    let agent_name = deps
        .agent_names
        .get(deps.agent_id.as_ref())
        .cloned()
        .unwrap_or_else(|| deps.agent_id.to_string());

    // Wake definitions supply each event's name and instructions.
    // Instructions apply only within the wake's min_level; events from a wake
    // above the current level are rendered as observations. An event whose
    // definition is gone falls back to its wake id.
    let wake_defs: HashMap<String, crate::wakes::WakeDef> = if wake_events.is_empty() {
        HashMap::new()
    } else {
        deps.wake_def_store
            .list()
            .await?
            .into_iter()
            .map(|def| (def.id.clone(), def))
            .collect()
    };

    let wake_event_views: Vec<AutonomyWakeEventView> = wake_events
        .iter()
        .map(|event| {
            let def = wake_defs.get(&event.wake_id);
            AutonomyWakeEventView {
                wake_id: event.wake_id.clone(),
                name: def
                    .map(|def| def.name.clone())
                    .unwrap_or_else(|| event.wake_id.clone()),
                instructions: def
                    .filter(|def| def.min_level <= config.level)
                    .map(|def| def.instructions.clone()),
                gated: def.is_some_and(|def| def.min_level > config.level),
                fired_at: event.fired_at.clone(),
                delivery_count: event.delivery_count,
                payload: compact_payload(&event.payload),
            }
        })
        .collect();

    let run_history_views: Vec<AutonomyRunHistoryView> = deps
        .autonomy_run_store
        .recent(config.run_history_count.max(1))
        .await?
        .into_iter()
        .filter(|run| run.status != AutonomyRunStatus::Running)
        .map(|run| AutonomyRunHistoryView {
            started_at: run.started_at,
            status: run.status.as_str().to_string(),
            summary: run
                .summary
                .unwrap_or_else(|| "no summary recorded".to_string()),
            woken_by: run.wake_event_ids.len(),
        })
        .collect();

    let (task_state, has_tasks) = render_task_state(deps, config.claim_unowned).await?;
    let active_goals = crate::goals::render_active_goals_extended(&deps.goal_store).await?;
    let active_workers = render_active_workers(deps).await?;

    // Nothing to survey and no direction to work from. The run needs different
    // instructions, not a shorter version of the same ones.
    //
    // A wake event or a running worker is direction: the run has a reason to
    // exist and a bounded turn to spend on it, which cold-start discovery
    // would spend on the workspace instead.
    let instance_is_empty = !has_tasks
        && active_goals.is_empty()
        && wake_event_views.is_empty()
        && active_workers.is_none();

    let prompt_engine = deps.runtime_config.prompts.load();
    prompt_engine
        .render_autonomy_channel_prompt(
            &agent_name,
            config.level.as_str(),
            wake_event_views,
            run_history_views,
            &task_state,
            (!active_goals.is_empty()).then_some(active_goals.as_str()),
            active_workers.as_deref(),
            config.max_tasks_per_run,
            config.warn_secs.div_ceil(60).max(1),
            config.claim_unowned,
            instance_is_empty,
        )
        .map_err(|error| anyhow::anyhow!("failed to render autonomy channel prompt: {error}"))
}

/// One-line JSON payload preview, truncated for prompt hygiene.
fn compact_payload(payload: &serde_json::Value) -> String {
    if payload.as_object().is_some_and(serde_json::Map::is_empty) {
        return String::new();
    }
    crate::tools::truncate_utf8_ellipsis(&payload.to_string(), 400)
}

/// Render the full task survey: pending_approval, ready, in_progress, backlog.
/// Returns the rendered survey and whether any task was visible in it.
async fn render_task_state(
    deps: &AgentDeps,
    claim_unowned: bool,
) -> anyhow::Result<(String, bool)> {
    let sections: [(TaskStatus, &str); 4] = [
        (
            TaskStatus::PendingApproval,
            "Pending approval (enrich these; never execute them)",
        ),
        (TaskStatus::Ready, "Ready (user-approved)"),
        (TaskStatus::InProgress, "In progress"),
        (TaskStatus::Backlog, "Backlog"),
    ];

    let mut output = String::new();
    let mut any = false;
    for (status, label) in sections {
        let tasks = deps
            .task_store
            .list(TaskListFilter {
                status: Some(status),
                limit: Some(200),
                ..Default::default()
            })
            .await?;
        let visible: Vec<Task> = tasks
            .into_iter()
            .filter(|task| task_visible_to_agent(task, &deps.agent_id, claim_unowned))
            .collect();
        if visible.is_empty() {
            continue;
        }

        any = true;

        // What has already been tried on these tasks, in one query. A run that
        // cannot see prior attempts repeats failed work and never escalates.
        let numbers: Vec<i64> = visible.iter().map(|task| task.task_number).collect();
        let attempts = deps
            .task_store
            .prior_attempt_summaries(&numbers)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "failed to load task attempt history for the board");
                std::collections::HashMap::new()
            });

        output.push_str(&format!("### {label}\n"));
        for task in visible {
            output.push_str(&render_task_line(
                &task,
                &deps.agent_id,
                attempts.get(&task.task_number).map(String::as_str),
            ));
        }
        output.push('\n');
    }

    if !any {
        output.push_str("No active tasks.\n");
    }
    Ok((output, any))
}

fn render_task_line(task: &Task, agent_id: &str, prior_attempts: Option<&str>) -> String {
    let ownership = match task.assigned_agent_id.as_deref() {
        Some(assigned) if assigned == agent_id => String::new(),
        Some(assigned) => format!(" (assigned to {assigned})"),
        None => " (unowned)".to_string(),
    };
    let mut line = format!(
        "- #{} [{}] {}{}",
        task.task_number,
        task.priority.as_str(),
        task.title,
        ownership
    );
    if let Some(description) = task
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        line.push_str(" — ");
        line.push_str(&crate::tools::truncate_utf8_ellipsis(
            &description.split_whitespace().collect::<Vec<_>>().join(" "),
            300,
        ));
    }
    // Surface the execution plan so runs execute tasks as configured instead
    // of re-deciding where and how the work happens.
    let plan = crate::tasks::ExecutionPlan::resolve(task, None);
    if !plan.is_empty() {
        line.push_str(&format!(" [{}]", plan.summary()));
    }
    // Surface the task autonomy mode so the executing agent honors its
    // contract: plan = research only (propose, execute nothing), yolo =
    // guardrails off, act without approval (explicit user request).
    if let Some(mode) = task.mode() {
        line.push_str(&format!(" [mode: {mode}: {}]", mode.description()));
    }
    // Surface ordering so runs don't re-derive the pipeline from prose.
    let blocked_by = task.blocked_by();
    if !blocked_by.is_empty() {
        let numbers: Vec<String> = blocked_by.iter().map(|n| format!("#{n}")).collect();
        line.push_str(&format!(" [blocked by {}]", numbers.join(", ")));
    }
    if let Some(parent) = task.stack_parent() {
        line.push_str(&format!(" [stacks on #{parent}]"));
    }
    // What has already been tried, so a run does not repeat failed work.
    if let Some(attempts) = prior_attempts {
        line.push_str(&format!(" [{attempts}]"));
    }
    line.push('\n');
    line
}

/// Render currently running workers so the run doesn't duplicate in-flight work.
async fn render_active_workers(deps: &AgentDeps) -> anyhow::Result<Option<String>> {
    let live_workers = deps.process_control_registry.list_worker_snapshots().await;
    let live_ids = live_workers
        .iter()
        .map(|worker| worker.worker_id.to_string())
        .collect::<HashSet<_>>();
    let logger = crate::conversation::ProcessRunLogger::new(deps.sqlite_pool.clone());
    let (workers, _total) = logger
        .list_worker_runs(&deps.agent_id, 20, 0, Some("running"))
        .await?;
<<<<<<< ours
    if workers.is_empty() {
=======
    let workers: Vec<_> = workers
        .into_iter()
        .filter(|worker| {
            !matches!(
                worker.lifecycle.as_str(),
                "succeeded" | "partial" | "cancelled" | "timed_out" | "blocked" | "failed"
            )
        })
        .collect();
    if workers.is_empty() && live_workers.is_empty() {
>>>>>>> theirs
        return Ok(None);
    }

    let mut output = String::new();
    for worker in live_workers {
        let task_line = crate::summarize_first_non_empty_line(&worker.provenance.task, 160);
        output.push_str(&format!(
            "- {} [{}; {}; runtime attached{}] {}\n",
            worker.worker_id,
            worker.backend,
            worker.state,
            if worker.interactive {
                ", interactive"
            } else {
                ""
            },
            task_line,
        ));
    }
    for worker in workers {
        if live_ids.contains(&worker.id) {
            continue;
        }
        let task_line = crate::summarize_first_non_empty_line(&worker.task, 160);
        output.push_str(&format!(
<<<<<<< ours
            "- {} [{}] {}\n",
            worker.id, worker.worker_type, task_line
=======
            "- {} [{}; {}{}{}; unavailable] {}\n",
            worker.id, worker.worker_type, worker.lifecycle, interaction, ownership, task_line
>>>>>>> theirs
        ));
    }
    Ok(Some(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn utc(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn due_requires_level_on() {
        assert!(!autonomy_run_due(
            AutonomyLevel::Off,
            utc(10_000),
            None,
            5,
            None,
            12,
            1800
        ));
        assert!(autonomy_run_due(
            AutonomyLevel::Observe,
            utc(10_000),
            None,
            0,
            None,
            12,
            1800
        ));
    }

    #[test]
    fn due_respects_active_hours() {
        // Window 8-22: hour 3 is outside even with pending events.
        assert!(!autonomy_run_due(
            AutonomyLevel::Act,
            utc(10_000),
            None,
            5,
            Some((8, 22)),
            3,
            1800
        ));
        assert!(autonomy_run_due(
            AutonomyLevel::Act,
            utc(10_000),
            None,
            5,
            Some((8, 22)),
            9,
            1800
        ));
        // Midnight-wrapping window 22-6: hour 23 is inside.
        assert!(autonomy_run_due(
            AutonomyLevel::Act,
            utc(10_000),
            None,
            0,
            Some((22, 6)),
            23,
            1800
        ));
    }

    #[test]
    fn pending_wake_events_pull_the_run_forward() {
        let now = utc(10_000);
        let recent_run = Some(utc(9_900)); // 100s ago, interval 1800s
        assert!(!autonomy_run_due(
            AutonomyLevel::Suggest,
            now,
            recent_run,
            0,
            None,
            12,
            1800
        ));
        assert!(autonomy_run_due(
            AutonomyLevel::Suggest,
            now,
            recent_run,
            1,
            None,
            12,
            1800
        ));
    }

    #[test]
    fn interval_elapse_makes_the_run_due() {
        let now = utc(10_000);
        assert!(!autonomy_run_due(
            AutonomyLevel::Act,
            now,
            Some(utc(10_000 - 1799)),
            0,
            None,
            12,
            1800
        ));
        assert!(autonomy_run_due(
            AutonomyLevel::Act,
            now,
            Some(utc(10_000 - 1800)),
            0,
            None,
            12,
            1800
        ));
        // Never ran before: immediately due.
        assert!(autonomy_run_due(
            AutonomyLevel::Act,
            now,
            None,
            0,
            None,
            12,
            1800
        ));
    }

    fn task_with_assignment(assigned: Option<&str>) -> Task {
        Task {
            id: "task-1".to_string(),
            task_number: 1,
            title: "test".to_string(),
            description: None,
            status: TaskStatus::Ready,
            priority: crate::tasks::TaskPriority::Medium,
            owner_agent_id: "owner".to_string(),
            assigned_agent_id: assigned.map(str::to_string),
            subtasks: Vec::new(),
            metadata: serde_json::json!({}),
            goal_id: None,
            source_memory_id: None,
            worker_id: None,
            worker_type: None,
            project_id: None,
            repo_id: None,
            worktree_mode: None,
            worktree_id: None,
            required_skills: Vec::new(),
            depends_on: Vec::new(),
            revision: 1,
            created_by: "user".to_string(),
            approved_at: None,
            approved_by: None,
            created_at: String::new(),
            updated_at: String::new(),
            completed_at: None,
        }
    }

    #[test]
    fn task_visibility_follows_assignment_and_claim_flag() {
        let mine = task_with_assignment(Some("agent-a"));
        let theirs = task_with_assignment(Some("agent-b"));
        let unowned = task_with_assignment(None);

        assert!(task_visible_to_agent(&mine, "agent-a", false));
        assert!(!task_visible_to_agent(&theirs, "agent-a", true));
        assert!(task_visible_to_agent(&unowned, "agent-a", true));
        assert!(!task_visible_to_agent(&unowned, "agent-a", false));
    }
<<<<<<< ours
=======

    #[tokio::test]
    async fn finish_waits_for_owned_children() {
        let store = run_store().await;
        let run_id = store.begin_run().await.unwrap();
        let handle = AutonomyRunHandle::new(run_id, 1, store);
        let worker_id = crate::WorkerId::new_v4();
        let child = AutonomyChild::WorkerOperation {
            worker_id,
            operation_id: crate::agent::process_control::WorkerOperationId::new(),
        };

        assert!(handle.register_child(child));
        assert_eq!(
            handle.request_finish(AutonomyFinishRequest {
                summary: "worker still active".to_string(),
                actions: Vec::new(),
            }),
            Err(1)
        );
        handle.settle_child(child);
        assert_eq!(
            handle.request_finish(AutonomyFinishRequest {
                summary: "worker result incorporated".to_string(),
                actions: Vec::new(),
            }),
            Ok(true)
        );
        assert!(!handle.register_child(AutonomyChild::WorkerOperation {
            worker_id: crate::WorkerId::new_v4(),
            operation_id: crate::agent::process_control::WorkerOperationId::new(),
        }));
    }

    #[tokio::test]
    async fn stale_worker_operation_cannot_settle_later_child() {
        let store = run_store().await;
        let run_id = store.begin_run().await.unwrap();
        let handle = AutonomyRunHandle::new(run_id, 1, store);
        let worker_id = crate::WorkerId::new_v4();
        let stale = AutonomyChild::WorkerOperation {
            worker_id,
            operation_id: crate::agent::process_control::WorkerOperationId::new(),
        };
        let current = AutonomyChild::WorkerOperation {
            worker_id,
            operation_id: crate::agent::process_control::WorkerOperationId::new(),
        };
        assert!(handle.register_child(current));

        handle.settle_child(stale);

        assert!(handle.owns_child(current));
        assert!(handle.has_active_children());
    }

    #[tokio::test]
    async fn stale_generation_cannot_clear_current_epoch() {
        let store = run_store().await;
        let slot = AutonomyRunSlot::default();
        let first = slot.begin("first".to_string(), store.clone());
        assert!(slot.clear_if_current(first.generation));
        let second = slot.begin("second".to_string(), store);

        assert!(!slot.clear_if_current(first.generation));
        assert_eq!(slot.current().unwrap().run_id, "second");
        assert!(slot.clear_if_current(second.generation));
        assert!(slot.current().is_none());
    }

    #[tokio::test]
    async fn autonomy_doorbell_coalesces_while_pending() {
        let control = AutonomyControl::default();
        let (check_tx, mut check_rx) = mpsc::channel(1);
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        control.attach(check_tx, shutdown_tx);

        assert!(!control.is_ready());
        control.activate();
        assert!(control.is_ready());
        control.request_check();

        assert_eq!(check_rx.recv().await, Some(()));
        assert!(check_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn wake_claim_rolls_back_when_epoch_is_not_active() {
        let pool = test_pool().await;
        let wake_store = crate::wakes::WakeEventStore::new(pool.clone());
        wake_store
            .enqueue("task.approved", "task:7", &serde_json::json!({"task": 7}))
            .await
            .unwrap();
        let event = wake_store.pending(1).await.unwrap().remove(0);

        assert!(
            commit_wake_claim(
                &pool,
                "missing-run",
                std::slice::from_ref(&event.id),
                std::slice::from_ref(&event.id),
            )
            .await
            .is_err()
        );
        assert_eq!(wake_store.pending_count().await.unwrap(), 1);
    }
>>>>>>> theirs
}
