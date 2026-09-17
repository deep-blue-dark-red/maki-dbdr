#!/usr/bin/env bash
set -euo pipefail
grep -q '_backoff_delay' "$RESULT_FILE"
grep -Eqi 'doubl|exponential|2 ?\*\*|\*\* ?attempt' "$RESULT_FILE"
