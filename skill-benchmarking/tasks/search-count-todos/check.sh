#!/usr/bin/env bash
set -euo pipefail
grep -Eq '(^|[^0-9])7([^0-9]|$)' "$RESULT_FILE"
