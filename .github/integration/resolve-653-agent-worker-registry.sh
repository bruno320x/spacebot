#!/usr/bin/env bash
set -euo pipefail

# Resolve upstream #653 semantically on top of the fork. The upstream worker
# registry becomes the runtime authority while fork-only contracts remain:
# task_type routing, evidence gates, configured secret scanning, TaskMode and
# task-attempt lifecycle. ACP is migrated to the registry in a follow-up patch
# in this same resolver after the textual conflicts are removed.

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


def channel_sig(ours, theirs):
    if 'task_type: Option<&str>' not in ours or 'PreparedWorkerSpawn' not in theirs:
        raise SystemExit('channel_dispatch conflict 1 shape changed')
    return ours.replace('std::result::Result<WorkerId, AgentError>',
                        'std::result::Result<PreparedWorkerSpawn, AgentError>')


def opencode_interactive(ours, theirs):
    if 'with_secret_scan_mode' not in ours or 'Some(input_tx)' not in theirs:
        raise SystemExit('channel_dispatch interactive OpenCode conflict shape changed')
    return '''        let worker =\n            worker.with_secret_scan_mode(state.deps.runtime_config.sandbox.load().secret_scanner);\n        (\n            worker.with_sqlite_pool(state.deps.sqlite_pool.clone()),\n            Some(input_tx),\n        )\n'''


def opencode_noninteractive(ours, theirs):
    if 'with_secret_scan_mode' not in ours or 'None' not in theirs:
        raise SystemExit('channel_dispatch noninteractive OpenCode conflict shape changed')
    return '''        let worker =\n            worker.with_secret_scan_mode(state.deps.runtime_config.sandbox.load().secret_scanner);\n        (\n            worker.with_sqlite_pool(state.deps.sqlite_pool.clone()),\n            None,\n        )\n'''

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
    return '''        .with_secret_scan_mode(deps.runtime_config.sandbox.load().secret_scanner)\n        .with_worker_registry(callback, deps.process_control_registry.clone());\n'''

resolve('src/agent/worker.rs', [worker_hook, 'theirs', 'theirs'])
worker = Path('src/agent/worker.rs')
text = worker.read_text()
old = 'crate::secrets::scrub::scrub_leaks(&scrubbed);'
new = 'crate::secrets::scrub::scrub_leaks_with_mode(&scrubbed, self.deps.runtime_config.sandbox.load().secret_scanner);'
if text.count(old) != 2:
    raise SystemExit(f'worker.rs: expected two registry result scrubs, found {text.count(old)}')
worker.write_text(text.replace(old, new))

# Registry-first cancellation supersedes channel-owned/detached runtime maps.
# Durable nonterminal/terminal fallback remains in upstream's NotFound branch.
resolve('src/api/channels.rs', ['theirs'])

resolve('src/hooks/spacebot.rs', ['theirs', 'theirs'])
hooks = Path('src/hooks/spacebot.rs')
text = hooks.read_text()
if text.count('crate::tools::MAX_TOOL_OUTPUT_BYTES') != 2:
    raise SystemExit('spacebot hook: expected two upstream fixed output limits')
text = text.replace('crate::tools::MAX_TOOL_OUTPUT_BYTES', 'crate::tools::tool_output_limit()')
hooks.write_text(text)

# #653 intentionally replaces channel reconstruction for idle workers with
# direct restoration into the agent-owned registry.
resolve('src/main.rs', ['theirs'])

resolve('src/opencode/worker.rs', ['theirs', 'theirs'])
opencode = Path('src/opencode/worker.rs')
text = opencode.read_text()
old = 'crate::secrets::scrub::scrub_leaks(&scrubbed);'
new = 'crate::secrets::scrub::scrub_leaks_with_mode(&scrubbed, self.secret_scan_mode);'
if text.count(old) != 2:
    raise SystemExit(f'opencode worker: expected two operation-result scrubs, found {text.count(old)}')
text = text.replace(old, new)
opencode.write_text(text)

# Keep fork evidence-gate tests and use the upstream name that reflects the
# actual once-only completion claim semantics.
def set_status_tests(ours, theirs):
    if 'outcome_without_evidence_is_rejected' not in ours or 'claims_completing_once' not in theirs:
        raise SystemExit('set_status test conflict shape changed')
    return ours.replace('non_interactive_outcome_claims_completing_idempotently',
                        'non_interactive_outcome_claims_completing_once')
resolve('src/tools/set_status.rs', [set_status_tests])

# The prepared-start gate and rollback logic is the core of #653. All worker
# backends, including ACP after the adaptation below, must use it.
resolve('src/tools/spawn_worker.rs', ['theirs', 'theirs', 'theirs', 'theirs'])

# Make ACP a first-class registry backend. Its runtime migration is applied
# below after the upstream registry type exists.
pc = Path('src/agent/process_control.rs')
text = pc.read_text()
old = '''pub enum WorkerBackend {\n    Builtin,\n    OpenCode,\n}\n'''
new = '''pub enum WorkerBackend {\n    Builtin,\n    OpenCode,\n    Acp,\n}\n'''
if text.count(old) != 1:
    raise SystemExit('process_control: WorkerBackend enum shape changed')
text = text.replace(old, new, 1)
old = '''            Self::Builtin => formatter.write_str("builtin"),\n            Self::OpenCode => formatter.write_str("opencode"),\n'''
new = '''            Self::Builtin => formatter.write_str("builtin"),\n            Self::OpenCode => formatter.write_str("opencode"),\n            Self::Acp => formatter.write_str("acp"),\n'''
if text.count(old) != 1:
    raise SystemExit('process_control: WorkerBackend Display shape changed')
text = text.replace(old, new, 1)
pc.write_text(text)
PY

git add \
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
