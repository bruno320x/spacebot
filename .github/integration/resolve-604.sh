#!/usr/bin/env bash
set -euo pipefail

# #604 contains four fixes. The current fork already has retry/backoff/quarantine,
# ingest-file purge, and canonical memory merge. Keep those fork implementations
# and port only the missing deterministic chunk-completion behavior.

git restore --source=HEAD --staged --worktree -- \
  src/agent/ingestion.rs \
  src/api/ingest.rs \
  src/memory/maintenance.rs

git rm -f --ignore-unmatch migrations/20260623000001_ingestion_retry_budget.sql >/dev/null 2>&1 || true
rm -f migrations/20260623000001_ingestion_retry_budget.sql

python3 - <<'PY'
from pathlib import Path

path = Path('src/agent/ingestion.rs')
text = path.read_text()
old = '''    if !contract_state.has_terminal_outcome() {\n        return Err(anyhow::anyhow!(\n            "ingestion chunk {chunk_number}/{total_chunks} for {filename} completed without memory_persistence_complete signal"\n        ));\n    }\n'''
new = '''    // A chunk is complete when the LLM run itself returns Ok. Any memory_save\n    // calls have already committed by this point, so requiring the model to also\n    // emit memory_persistence_complete turns a missing advisory signal into a\n    // false failure and can cause duplicate retries. Keep the contract state for\n    // observability, but do not use it as a completion gate.\n    let saved = contract_state.saved_memory_ids().len();\n    tracing::info!(\n        file = %filename,\n        chunk = %format!("{chunk_number}/{total_chunks}"),\n        saved_memories = saved,\n        terminal_signal = contract_state.has_terminal_outcome(),\n        "chunk processed"\n    );\n'''
if old not in text:
    raise SystemExit('expected #604 completion-gate block not found in current fork')
text = text.replace(old, new, 1)
path.write_text(text)
PY

git add src/agent/ingestion.rs
