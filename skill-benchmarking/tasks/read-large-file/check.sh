#!/usr/bin/env bash
set -euo pipefail
grep -Eqi '\[\]|empty list|empty' "$RESULT_FILE"
