#!/usr/bin/env bash
set -euo pipefail

# The fork has a newer durable worker lifecycle than upstream #551. Preserve it
# and port only the remaining bug: a fresh builtin worker that already signaled
# a terminal outcome must not transition to WaitingForInput.
git restore --source=HEAD --staged --worktree -- \
  src/agent/worker.rs \
  src/conversation/history.rs

python3 - <<'PY'
from pathlib import Path

path = Path('src/agent/worker.rs')
text = path.read_text()
old = '''                // Fresh worker: persist transcript and signal idle for the first time.\n                // Resumed workers already did this in the preamble above.\n                self.state = WorkerState::WaitingForInput;\n                self.persist_transcript(&compacted_history, &history).await;\n                self.persist_lifecycle_transition(\n                    WorkerLifecycle::Running,\n                    WorkerLifecycle::WaitingForInput,\n                )\n                .await;\n                self.hook.send_status("waiting for input");\n                self.hook.send_worker_idle();\n'''
new = '''                // A terminal outcome means the initial task is complete. Do not\n                // advertise the worker as idle or block forever waiting for a\n                // follow-up that can only be released after WorkerComplete.\n                if self.hook.outcome_signaled() {\n                    tracing::info!(\n                        worker_id = %self.id,\n                        "terminal outcome already signaled; skipping follow-up idle state"\n                    );\n                    input_rx.close();\n                    while input_rx.try_recv().is_ok() {}\n                } else {\n                    // Fresh non-terminal interactive worker: persist transcript\n                    // and signal idle for the first time. Resumed workers already\n                    // did this in the preamble above.\n                    self.state = WorkerState::WaitingForInput;\n                    self.persist_transcript(&compacted_history, &history).await;\n                    self.persist_lifecycle_transition(\n                        WorkerLifecycle::Running,\n                        WorkerLifecycle::WaitingForInput,\n                    )\n                    .await;\n                    self.hook.send_status("waiting for input");\n                    self.hook.send_worker_idle();\n                }\n'''
if old not in text:
    raise SystemExit('expected fresh-worker idle block not found in current fork')
text = text.replace(old, new, 1)
path.write_text(text)
PY

git add src/agent/worker.rs
