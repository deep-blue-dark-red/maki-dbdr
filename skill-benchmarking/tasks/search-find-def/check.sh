#!/usr/bin/env bash
set -euo pipefail
grep -q 'config\.py' "$RESULT_FILE"
grep -Eq '\b30\b' "$RESULT_FILE"
