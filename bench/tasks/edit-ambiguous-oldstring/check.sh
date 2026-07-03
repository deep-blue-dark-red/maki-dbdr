#!/usr/bin/env bash
set -euo pipefail
cd "$WORKDIR"
f=svc/client.py
[ "$(grep -c 'attempts += 2' "$f")" = 1 ]
[ "$(grep -c 'attempts += 1' "$f")" = 2 ]
awk '/^def fetch_with_retry/,/^def warm_cache/' "$f" | grep -q 'attempts += 2'
python3 -c "import ast; ast.parse(open('$f').read())"
