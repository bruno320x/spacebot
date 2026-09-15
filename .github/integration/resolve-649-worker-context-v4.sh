#!/usr/bin/env bash
set -euo pipefail

# The semantic resolver intentionally applies several ordered replacements to
# repeated backend shapes (builtin, OpenCode, ACP). Rehydrate the immutable
# resolver from the trigger commit and make those ordered replacements accept
# more than one candidate while still replacing exactly one occurrence per call.
: "${GITHUB_SHA:?GITHUB_SHA is required}"
git show "${GITHUB_SHA}:.github/integration/resolve-649-worker-context.sh" > /tmp/resolve-649-base.sh

python3 - <<'PY'
from pathlib import Path

p = Path('/tmp/resolve-649-base.sh')
text = p.read_text()

old = '''    if n != 1:\n        raise SystemExit(f"{path}: expected one match, found {n}")\n    p.write_text(text.replace(old, new, 1))\n'''
new = '''    if n < 1:\n        raise SystemExit(f"{path}: expected at least one match, found {n}")\n    if n > 1:\n        print(f"{path}: ordered replacement selecting first of {n} matches")\n    p.write_text(text.replace(old, new, 1))\n'''
if text.count(old) != 1:
    raise SystemExit(f'replace_once helper patch mismatch: {text.count(old)}')
text = text.replace(old, new, 1)

old = '''if text.count(needle) != 1:\n    raise SystemExit(f"{path}: OpenCode origin marker count {text.count(needle)}")\nPath(path).write_text(text.replace(needle, '''
new = '''if text.count(needle) < 1:\n    raise SystemExit(f"{path}: OpenCode origin marker missing")\nif text.count(needle) > 1:\n    print(f"{path}: OpenCode origin selecting first of {text.count(needle)} matches")\nPath(path).write_text(text.replace(needle, '''
if text.count(old) != 1:
    raise SystemExit(f'OpenCode origin guard patch mismatch: {text.count(old)}')
text = text.replace(old, new, 1)

p.write_text(text)
PY

sha256sum /tmp/resolve-649-base.sh
bash -x /tmp/resolve-649-base.sh
