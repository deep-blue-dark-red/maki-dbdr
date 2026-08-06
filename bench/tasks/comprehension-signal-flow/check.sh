#!/usr/bin/env bash
set -euo pipefail
tr '\n' ' ' < "$RESULT_FILE" | grep -Eq 'drain_queue.*flush_metrics.*_shutdown'
