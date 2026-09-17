#!/usr/bin/env bash
set -euo pipefail
cd "$WORKDIR"
root=$(git rev-list --max-parents=0 HEAD)
git diff --quiet "$root" -- tests/
test -f util/slug.py
python3 -m unittest discover -q
