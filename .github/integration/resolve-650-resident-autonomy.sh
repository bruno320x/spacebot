#!/usr/bin/env bash
set -euo pipefail

# #650 conflicts only in channel_dispatch.rs because this fork already extends
# the dispatcher with ACP, secret scanning, task context and richer lifecycle
# handling. Keep the fork's #649 version of this file, then port the resident
# autonomy ownership semantics across builtin, OpenCode and ACP backends.
git checkout --ours -- src/agent/channel_dispatch.rs

python3 - <<'PY'
from pathlib import Path

path = Path('src/agent/channel_dispatch.rs')
text = path.read_text()

def replace_once(old: str, new: str, label: str) -> None:
    global text
    n = text.count(old)
    if n != 1:
        raise SystemExit(f'{label}: expected one match, found {n}')
    text = text.replace(old, new, 1)

# Branch admission: preserve the fork's secret-scan capture while switching to
# the generation-scoped AutonomyRunSlot introduced by #650.
replace_once(
'''    // Captured before tokio::spawn — `state` is borrowed and cannot be read
    // inside the spawned future.
    let secret_scan_mode = state.deps.runtime_config.sandbox.load().secret_scanner;

    if state
        .autonomy_run
        .as_ref()
        .is_some_and(crate::agent::autonomy::AutonomyRunHandle::finish_requested)
    {
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn branch: autonomy run is settling"
        )));
    }
''',
'''    // Captured before tokio::spawn — `state` is borrowed and cannot be read
    // inside the spawned future.
    let secret_scan_mode = state.deps.runtime_config.sandbox.load().secret_scanner;

    let autonomy_run = state.autonomy_run();
    if state.kind == crate::agent::channel::ChannelKind::Autonomy && autonomy_run.is_none() {
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn branch: no active autonomy epoch"
        )));
    }
    if autonomy_run
        .as_ref()
        .is_some_and(crate::agent::autonomy::AutonomyRunHandle::finish_requested)
    {
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn branch: autonomy epoch is settling"
        )));
    }
''',
'branch admission',
)

# Register the branch before durable logging so autonomy_complete cannot race a
# spawn. Roll ownership back if the durable run row cannot be written.
replace_once(
'''    let prompt = prompt.to_owned();

    state
        .process_run_logger
        .log_branch_started(
            &state.channel_id,
            branch_id,
            description,
            &prompt,
            &profile_name,
            &model_name,
            branch_max_turns,
            state
                .autonomy_run
                .as_ref()
                .map(|autonomy_run| autonomy_run.run_id.as_str()),
        )
        .await
        .map_err(|error| AgentError::Other(anyhow::anyhow!(error)))?;
''',
'''    let prompt = prompt.to_owned();

    if let Some(run) = &autonomy_run
        && !run.register_child(crate::agent::autonomy::AutonomyChild::Branch(branch_id))
    {
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn branch: autonomy epoch is finishing"
        )));
    }

    if let Err(error) = state
        .process_run_logger
        .log_branch_started(
            &state.channel_id,
            branch_id,
            description,
            &prompt,
            &profile_name,
            &model_name,
            branch_max_turns,
            autonomy_run.as_ref().map(|run| run.run_id.as_str()),
        )
        .await
    {
        if let Some(run) = &autonomy_run {
            run.settle_child(crate::agent::autonomy::AutonomyChild::Branch(branch_id));
        }
        return Err(AgentError::Other(anyhow::anyhow!(error)));
    }
''',
'branch ownership',
)

# Builtin worker admission uses the same epoch slot and refuses autonomous work
# if no epoch is active.
replace_once(
'''    if state
        .autonomy_run
        .as_ref()
        .is_some_and(crate::agent::autonomy::AutonomyRunHandle::finish_requested)
    {
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn worker: autonomy run is settling"
        )));
    }
''',
'''    let autonomy_run = state.autonomy_run();
    if state.kind == crate::agent::channel::ChannelKind::Autonomy && autonomy_run.is_none() {
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn worker: no active autonomy epoch"
        )));
    }
    if autonomy_run
        .as_ref()
        .is_some_and(crate::agent::autonomy::AutonomyRunHandle::finish_requested)
    {
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn worker: autonomy epoch is settling"
        )));
    }
''',
'builtin admission',
)

# Builtin child ownership. Preserve WorkerTaskContext, task_type routing,
# transcript/evidence plumbing and fork-specific cleanup.
replace_once(
'''    let worker_id = worker.id;
    let transcript_snapshot = worker.transcript_snapshot();

    state
        .process_run_logger
        .log_worker_started(
            Some(&state.channel_id),
            worker_id,
            task,
            "builtin",
            &state.deps.agent_id,
            interactive,
            None,
            state
                .autonomy_run
                .as_ref()
                .map(|autonomy_run| autonomy_run.run_id.as_str()),
            task_context.origin_branch_id,
        )
        .await
        .map_err(|error| AgentError::Other(anyhow::anyhow!(error)))?;
''',
'''    let worker_id = worker.id;
    let transcript_snapshot = worker.transcript_snapshot();
    let autonomy_run = state.autonomy_run();
    if let Some(run) = &autonomy_run
        && !run.register_child(crate::agent::autonomy::AutonomyChild::Worker(worker_id))
    {
        state.cleanup_worker_routing(worker_id).await;
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn worker: autonomy epoch is finishing"
        )));
    }

    if let Err(error) = state
        .process_run_logger
        .log_worker_started(
            Some(&state.channel_id),
            worker_id,
            task,
            "builtin",
            &state.deps.agent_id,
            interactive,
            None,
            autonomy_run.as_ref().map(|run| run.run_id.as_str()),
            task_context.origin_branch_id,
        )
        .await
    {
        if let Some(run) = &autonomy_run {
            run.settle_child(crate::agent::autonomy::AutonomyChild::Worker(worker_id));
        }
        state.cleanup_worker_routing(worker_id).await;
        return Err(AgentError::Other(anyhow::anyhow!(error)));
    }
''',
'builtin ownership',
)

# OpenCode ownership from upstream #650, adapted to #649 WorkerTaskContext.
replace_once(
'''    let worker_id = worker.id;

    state
        .process_run_logger
        .log_worker_started(
            Some(&state.channel_id),
            worker_id,
            &format!("[opencode] {task}"),
            "opencode",
            &state.deps.agent_id,
            interactive,
            Some(&persist_directory),
            state
                .autonomy_run
                .as_ref()
                .map(|autonomy_run| autonomy_run.run_id.as_str()),
            task_context.origin_branch_id,
        )
        .await
        .map_err(|error| AgentError::Other(anyhow::anyhow!(error)))?;
''',
'''    let worker_id = worker.id;
    let autonomy_run = state.autonomy_run();
    if let Some(run) = &autonomy_run
        && !run.register_child(crate::agent::autonomy::AutonomyChild::Worker(worker_id))
    {
        state.cleanup_worker_routing(worker_id).await;
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn worker: autonomy epoch is finishing"
        )));
    }

    if let Err(error) = state
        .process_run_logger
        .log_worker_started(
            Some(&state.channel_id),
            worker_id,
            &format!("[opencode] {task}"),
            "opencode",
            &state.deps.agent_id,
            interactive,
            Some(&persist_directory),
            autonomy_run.as_ref().map(|run| run.run_id.as_str()),
            task_context.origin_branch_id,
        )
        .await
    {
        if let Some(run) = &autonomy_run {
            run.settle_child(crate::agent::autonomy::AutonomyChild::Worker(worker_id));
        }
        state.cleanup_worker_routing(worker_id).await;
        return Err(AgentError::Other(anyhow::anyhow!(error)));
    }
''',
'opencode ownership',
)

# ACP does not exist upstream. Give it exactly the same generation-scoped child
# ownership contract as OpenCode so its initial/follow-up results are accepted
# by the resident autonomy channel instead of being discarded as stale.
replace_once(
'''    let worker =
        worker.with_secret_scan_mode(state.deps.runtime_config.sandbox.load().secret_scanner);

    state
        .process_run_logger
        .log_worker_started(
            Some(&state.channel_id),
            worker_id,
            &format!("[acp] {task}"),
            "acp",
            &state.deps.agent_id,
            interactive,
            Some(&persist_directory),
            state
                .autonomy_run
                .as_ref()
                .map(|autonomy_run| autonomy_run.run_id.as_str()),
            task_context.origin_branch_id,
        )
        .await
        .map_err(|error| AgentError::Other(anyhow::anyhow!(error)))?;
''',
'''    let worker =
        worker.with_secret_scan_mode(state.deps.runtime_config.sandbox.load().secret_scanner);

    let autonomy_run = state.autonomy_run();
    if let Some(run) = &autonomy_run
        && !run.register_child(crate::agent::autonomy::AutonomyChild::Worker(worker_id))
    {
        state.cleanup_worker_routing(worker_id).await;
        return Err(AgentError::Other(anyhow::anyhow!(
            "can't spawn worker: autonomy epoch is finishing"
        )));
    }

    if let Err(error) = state
        .process_run_logger
        .log_worker_started(
            Some(&state.channel_id),
            worker_id,
            &format!("[acp] {task}"),
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
            run.settle_child(crate::agent::autonomy::AutonomyChild::Worker(worker_id));
        }
        state.cleanup_worker_routing(worker_id).await;
        return Err(AgentError::Other(anyhow::anyhow!(error)));
    }
''',
'acp ownership',
)

# The #650 ChannelState conversion must leave no direct access to the old
# Option<AutonomyRunHandle> representation in this dispatcher.
if '.autonomy_run\n' in text or '.autonomy_run\r\n' in text:
    raise SystemExit('old direct autonomy_run field access remains')

path.write_text(text)
PY

git add src/agent/channel_dispatch.rs

git diff --cached --check
if git grep -n -E '^(<<<<<<<|=======|>>>>>>>)' -- src/agent/channel_dispatch.rs; then
  echo "conflict markers remain in channel_dispatch.rs" >&2
  exit 1
fi
