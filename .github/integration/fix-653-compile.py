from pathlib import Path
import re


def replace_exact(path: str, old: str, new: str, count: int = 1):
    p = Path(path)
    text = p.read_text()
    actual = text.count(old)
    if actual != count:
        raise SystemExit(f"{path}: expected {count} exact matches, found {actual}")
    p.write_text(text.replace(old, new, count))


def sub_exact(path: str, pattern: str, repl: str, count: int = 1, flags: int = 0):
    p = Path(path)
    text = p.read_text()
    new, actual = re.subn(pattern, repl, text, count=count, flags=flags)
    if actual != count:
        raise SystemExit(f"{path}: expected {count} regex matches, found {actual}: {pattern}")
    p.write_text(new)


# #653 makes ProcessControlRegistry the only worker-control authority. Remove
# the fork's pre-#653 detached WorkerTaskControl maps, but keep main.rs's local
# `detached_workers: Vec<_>` used during idle-worker reconciliation.
sub_exact(
    "src/lib.rs",
    r"\n    /// Live controls for channel-less \(cortex/autonomy\) workers, keyed by\n"
    r"    /// worker id\. Lets the cancel API abort detached workers that no channel\n"
    r"    /// owns \(#653\); entries are removed when their task finishes\.\n"
    r"    pub detached_workers: Arc<\n"
    r"        tokio::sync::RwLock<\n"
    r"            std::collections::HashMap<WorkerId, crate::agent::channel_dispatch::WorkerTaskControl>,\n"
    r"        >,\n"
    r"    >,",
    "",
)

sub_exact(
    "src/api/state.rs",
    r"\n    /// Agent-level registries of channel-less \(cortex/autonomy\) worker\n"
    r"    /// controls, keyed by agent id\. Registered when an agent starts; lets the\n"
    r"    /// cancel API reach detached workers no channel owns \(#653\)\.\n"
    r"    pub detached_worker_registries: RwLock<\n"
    r"        HashMap<\n"
    r"            String,\n"
    r"            Arc<\n"
    r"                tokio::sync::RwLock<\n"
    r"                    HashMap<crate::WorkerId, crate::agent::channel_dispatch::WorkerTaskControl>,\n"
    r"                >,\n"
    r"            >,\n"
    r"        >,\n"
    r"    >,",
    "",
)
replace_exact(
    "src/api/state.rs",
    "            detached_worker_registries: RwLock::new(HashMap::new()),\n",
    "",
)
sub_exact(
    "src/api/state.rs",
    r"\n    /// Register an agent's detached-worker registry so the cancel API can\n"
    r"    /// abort channel-less workers spawned by that agent \(#653\)\.\n"
    r"    pub async fn register_detached_workers\([\s\S]*?\n    }\n\n"
    r"    /// Cancel a channel-less worker that is still live in this process\. Looks\n"
    r"    /// across every agent's detached-worker registry\. Returns true when found\n"
    r"    /// and aborted\.\n"
    r"    pub async fn cancel_detached_worker\([\s\S]*?\n    }\n",
    "",
)

replace_exact(
    "src/main.rs",
    "                    api_state.register_detached_workers(\n"
    "                        agent.deps.agent_id.to_string(),\n"
    "                        agent.deps.detached_workers.clone(),\n"
    "                    ).await;\n",
    "",
)
replace_exact(
    "src/main.rs",
    "            detached_workers: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),\n",
    "",
)

sub_exact(
    "src/api/agents.rs",
    r"\n                detached_workers: Arc::new\(tokio::sync::RwLock::new\(\n"
    r"                    std::collections::HashMap::new\(\),\n"
    r"                \)\),",
    "",
)
replace_exact(
    "src/api/agents.rs",
    "        detached_workers: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),\n",
    "",
)

# ACP permissions use the same registration-aware interaction routing as
# OpenCode. The active operation is authoritative for the delivery target.
replace_exact(
    "src/acp/worker.rs",
    "    WorkerCallbackContext, WorkerFollowUp, WorkerOperationContext,\n",
    "    WorkerCallbackContext, WorkerFollowUp, WorkerOperationContext, WorkerResultTarget,\n",
)
replace_exact(
    "src/acp/worker.rs",
    "                let _ = self.event_tx.send(ProcessEvent::WorkerPermission {\n"
    "                    agent_id: self.agent_id.clone(),\n"
    "                    worker_id: self.id,\n"
    "                    channel_id: self.channel_id.clone(),\n"
    "                    permission_id: permission.permission_id.clone(),\n"
    "                    description: description.to_string(),\n"
    "                    patterns: Vec::new(),\n"
    "                });\n",
    "                let interaction_target = self\n"
    "                    .process_control_registry\n"
    "                    .worker_snapshot_for_callback(self.callback)\n"
    "                    .await\n"
    "                    .and_then(|snapshot| {\n"
    "                        snapshot\n"
    "                            .active_operation\n"
    "                            .map(|operation| operation.result_target)\n"
    "                    })\n"
    "                    .unwrap_or(WorkerResultTarget::None);\n"
    "                let event_tx = self.event_tx.clone();\n"
    "                let event = ProcessEvent::WorkerPermission {\n"
    "                    agent_id: self.agent_id.clone(),\n"
    "                    worker_id: self.id,\n"
    "                    worker_registration_id: self.callback.registration_id,\n"
    "                    interaction_target,\n"
    "                    channel_id: self.channel_id.clone(),\n"
    "                    permission_id: permission.permission_id.clone(),\n"
    "                    description: description.to_string(),\n"
    "                    patterns: Vec::new(),\n"
    "                };\n"
    "                self.process_control_registry\n"
    "                    .run_if_worker_state(\n"
    "                        self.callback,\n"
    "                        crate::agent::process_control::WorkerRuntimeState::Running,\n"
    "                        move || {\n"
    "                            event_tx.send(event).ok();\n"
    "                        },\n"
    "                    )\n"
    "                    .await;\n",
)

# WorkerOperationResult supersedes WorkerInitialResult in #653. Keep the
# operation completion/result block below; remove only the obsolete duplicate.
sub_exact(
    "src/agent/worker.rs",
    r"\n                if !result\.trim\(\)\.is_empty\(\) \{\n"
    r"                    let scrubbed = if let Some\(store\) =[\s\S]*?"
    r"                    let _ = self\n"
    r"                        \.deps\n"
    r"                        \.event_tx\n"
    r"                        \.send\(crate::ProcessEvent::WorkerInitialResult \{[\s\S]*?"
    r"                        \}\);\n"
    r"                \}\n",
    "\n",
)
replace_exact(
    "src/agent/worker.rs",
    "        // For interactive workers, deliver the initial result to the channel\n"
    "        // before entering the follow-up loop. Without this, a fresh worker\n"
    "        // that completes its task (outcome signaled) stalls in\n"
    "        // `input_rx.recv()` forever and its initial result never reaches the\n"
    "        // channel — run() cannot return until the follow-up loop exits, the\n"
    "        // loop only exits when the sender is dropped, and the sender is only\n"
    "        // dropped on WorkerComplete. ACP/OpenCode workers already send\n"
    "        // WorkerInitialResult for the initial task; builtin workers did not.\n",
    "        // For interactive workers, commit the initial operation result before\n"
    "        // entering the follow-up loop. #653 routes this through the registry's\n"
    "        // registration-aware WorkerOperationResult contract for every backend.\n",
)

# Three registry tests call spawn_worker_task directly. Production callsites
# already pass the configured scan mode.
replace_exact(
    "src/agent/channel_dispatch.rs",
    "            None,\n            None,\n            None,\n            \"builtin\",\n",
    "            None,\n            None,\n            None,\n            crate::secrets::scrub::SecretScanMode::Strict,\n            \"builtin\",\n",
    count=3,
)

# Resumed OpenCode workers are immediately passed through with_secret_scan_mode
# using the live sandbox config in channel_dispatch; initialize the field safely
# so the constructor remains total before the builder override.
replace_exact(
    "src/opencode/worker.rs",
    "            secrets_store: None,\n            sqlite_pool: None,\n",
    "            secrets_store: None,\n            secret_scan_mode: SecretScanMode::Strict,\n            sqlite_pool: None,\n",
)

# ACP is now a first-class backend in the operation-result marker path.
replace_exact(
    "src/agent/process_control.rs",
    "        WorkerBackend::OpenCode => {\n"
    "            \"OpenCode operation completed without a textual result.\".to_string()\n"
    "        }\n",
    "        WorkerBackend::OpenCode => {\n"
    "            \"OpenCode operation completed without a textual result.\".to_string()\n"
    "        }\n"
    "        WorkerBackend::Acp => \"ACP operation completed without a textual result.\".to_string(),\n",
)

# Keep the evidence contract explicit in the stale-callback test.
replace_exact(
    "src/tools/set_status.rs",
    "            tool.call(SetStatusArgs {\n"
    "                status: \"stale\".to_string(),\n"
    "                kind: StatusKind::Progress,\n"
    "            })\n",
    "            tool.call(SetStatusArgs {\n"
    "                status: \"stale\".to_string(),\n"
    "                kind: StatusKind::Progress,\n"
    "                evidence: None,\n"
    "            })\n",
)

# Hard invariants: no compile-time legacy control type or removed event remains.
combined = "\n".join(Path(p).read_text() for p in [
    "src/lib.rs",
    "src/api/state.rs",
    "src/agent/worker.rs",
])
if "WorkerTaskControl" in combined:
    raise SystemExit("legacy WorkerTaskControl still present")
if "WorkerInitialResult" in Path("src/agent/worker.rs").read_text():
    raise SystemExit("legacy WorkerInitialResult still present")

print("#653 compile-contract reconciliation applied")
