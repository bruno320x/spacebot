#!/usr/bin/env bash
set -euo pipefail

cp .github/integration/resolve-649-worker-context.sh /tmp/resolve-649-base.sh
python3 - <<'PY'
from pathlib import Path
p = Path('/tmp/resolve-649-base.sh')
text = p.read_text()
needle = '''def replace_once(path: str, old: str, new: str) -> None:\n    p = Path(path)\n    text = p.read_text()\n    n = text.count(old)\n    if n != 1:\n        raise SystemExit(f"{path}: expected one match, found {n}")\n    p.write_text(text.replace(old, new, 1))\n'''
replacement = needle + '''\ndef replace_first(path: str, old: str, new: str) -> None:\n    p = Path(path)\n    text = p.read_text()\n    if old not in text:\n        raise SystemExit(f"{path}: expected at least one positional match")\n    p.write_text(text.replace(old, new, 1))\n'''
if text.count(needle) != 1:
    raise SystemExit('resolver helper marker mismatch')
text = text.replace(needle, replacement, 1)
old_call = '''replace_once(\n    path,\n    ''' + "'''" + '''        worker_context,\\n        origin_branch_id,\\n    )\\n''' + "'''" + ''',\n    ''' + "'''" + '''        worker_context,\\n        task_context,\\n    )\\n''' + "'''" + ''',\n)'''
new_call = old_call.replace('replace_once(', 'replace_first(', 1)
if text.count(old_call) != 1:
    raise SystemExit(f'ambiguous-call resolver marker mismatch: {text.count(old_call)}')
text = text.replace(old_call, new_call, 1)
p.write_text(text)
PY
chmod +x /tmp/resolve-649-base.sh
exec /tmp/resolve-649-base.sh
