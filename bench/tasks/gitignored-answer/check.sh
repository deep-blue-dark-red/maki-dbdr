#!/usr/bin/env bash
set -euo pipefail
grep -q 'v7\.3\.1-nightly' "$RESULT_FILE"
