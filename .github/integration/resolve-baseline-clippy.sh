#!/usr/bin/env bash
set -euo pipefail

# Discard the carrier PR delta. This resolution script uses the controlled
# port engine only as a Git executor and applies the baseline repair directly
# onto the requested target branch.
git reset --hard "origin/${PORT_TARGET}"

python3 - <<'PY'
from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one match, found {count}")
    p.write_text(text.replace(old, new, 1))

replace_once(
    "src/acp/worker.rs",
    '''    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR") {\n        if !dir.is_empty() {\n            return Some(PathBuf::from(dir));\n        }\n    }\n''',
    '''    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR")\n        && !dir.is_empty()\n    {\n        return Some(PathBuf::from(dir));\n    }\n''',
)

replace_once(
    "src/agent/channel_dispatch.rs",
    '''pub async fn spawn_worker_from_state(\n''',
    '''// The dispatch boundary intentionally carries the complete worker launch contract.\n// #653 will replace this channel-owned path with an agent-owned registry.\n#[allow(clippy::too_many_arguments)]\npub async fn spawn_worker_from_state(\n''',
)
replace_once(
    "src/agent/channel_dispatch.rs",
    '''async fn spawn_worker_inner(\n''',
    '''#[allow(clippy::too_many_arguments)]\nasync fn spawn_worker_inner(\n''',
)

replace_once(
    "src/api/state.rs",
    '''const MAX_COMPLETED_PROCESS_TOMBSTONES: usize = 4_096;\n''',
    '''const MAX_COMPLETED_PROCESS_TOMBSTONES: usize = 4_096;\n\npub type DetachedWorkerRegistry = Arc<\n    tokio::sync::RwLock<\n        HashMap<crate::WorkerId, crate::agent::channel_dispatch::WorkerTaskControl>,\n    >,\n>;\n''',
)
replace_once(
    "src/api/state.rs",
    '''    pub detached_worker_registries: RwLock<\n        HashMap<\n            String,\n            Arc<\n                tokio::sync::RwLock<\n                    HashMap<crate::WorkerId, crate::agent::channel_dispatch::WorkerTaskControl>,\n                >,\n            >,\n        >,\n    >,\n''',
    '''    pub detached_worker_registries: RwLock<HashMap<String, DetachedWorkerRegistry>>,\n''',
)

replace_once(
    "src/conversation/settings.rs",
    '''        let response_mode = channel.response_mode.or_else(|| {\n            if channel.listen_only_mode {\n                Some(ResponseMode::Observe)\n            } else {\n                None\n            }\n        });\n''',
    '''        let response_mode = channel.response_mode.or({\n            if channel.listen_only_mode {\n                Some(ResponseMode::Observe)\n            } else {\n                None\n            }\n        });\n''',
)

replace_once(
    "src/llm/model.rs",
    '''        serde_json::Value::Array(items) => items.iter_mut().fold(false, |changed, item| {\n            sanitize_tool_arguments(item) || changed\n        }),\n        serde_json::Value::Object(fields) => fields.values_mut().fold(false, |changed, field| {\n            sanitize_tool_arguments(field) || changed\n        }),\n''',
    '''        serde_json::Value::Array(items) => {\n            let mut changed = false;\n            for item in items {\n                changed |= sanitize_tool_arguments(item);\n            }\n            changed\n        }\n        serde_json::Value::Object(fields) => {\n            let mut changed = false;\n            for field in fields.values_mut() {\n                changed |= sanitize_tool_arguments(field);\n            }\n            changed\n        }\n''',
)

replace_once(
    "src/secrets/scrub.rs",
    '''#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]\n''',
    '''#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]\n''',
)
replace_once(
    "src/secrets/scrub.rs",
    '''    /// Regex layer active everywhere (default — today's behavior).\n    Strict,\n''',
    '''    /// Regex layer active everywhere (default — today's behavior).\n    #[default]\n    Strict,\n''',
)
replace_once(
    "src/secrets/scrub.rs",
    '''\nimpl Default for SecretScanMode {\n    fn default() -> Self {\n        Self::Strict\n    }\n}\n''',
    '''\n''',
)

replace_once(
    "src/tools/memory_save.rs",
    '''        if memory_type != MemoryType::Human {\n            if let Some(existing_id) = store\n                .find_exact_duplicate(&args.content, memory_type)\n                .await\n                .map_err(|e| {\n                    MemorySaveError(format!("Failed to check for duplicate memory: {e}"))\n                })?\n            {\n                if let Some(contract_state) = &self.contract_state {\n                    contract_state.record_saved_memory_id(existing_id.clone());\n                }\n                return Ok(MemorySaveOutput {\n                    memory_id: existing_id,\n                    success: true,\n                    message:\n                        "memory already exists - reused existing record (no duplicate created)"\n                            .to_string(),\n                    consolidation: None,\n                });\n            }\n        }\n''',
    '''        if memory_type != MemoryType::Human\n            && let Some(existing_id) = store\n                .find_exact_duplicate(&args.content, memory_type)\n                .await\n                .map_err(|e| {\n                    MemorySaveError(format!("Failed to check for duplicate memory: {e}"))\n                })?\n        {\n            if let Some(contract_state) = &self.contract_state {\n                contract_state.record_saved_memory_id(existing_id.clone());\n            }\n            return Ok(MemorySaveOutput {\n                memory_id: existing_id,\n                success: true,\n                message: "memory already exists - reused existing record (no duplicate created)"\n                    .to_string(),\n                consolidation: None,\n            });\n        }\n''',
)

replace_once(
    "src/tools/spawn_worker.rs",
    '''            .insert(worker_id.clone(), worker_control);\n''',
    '''            .insert(worker_id, worker_control);\n''',
)
replace_once(
    "src/tools/spawn_worker.rs",
    '''            let cleanup_worker_id = worker_id.clone();\n''',
    '''            let cleanup_worker_id = worker_id;\n''',
)
PY

cargo fmt --all
git add src/acp/worker.rs src/agent/channel_dispatch.rs src/api/state.rs \
  src/conversation/settings.rs src/llm/model.rs src/secrets/scrub.rs \
  src/tools/memory_save.rs src/tools/spawn_worker.rs
git diff --cached --check
