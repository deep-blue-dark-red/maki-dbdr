#!/usr/bin/env bash
# Run a headless maki agent and check its output.
# Usage: agent-test.sh <prompt> [--expect <pattern>] [--expect-not <pattern>]
# Exit 0 = pass, 1 = fail.
set -euo pipefail

PROMPT=""
EXPECT=()
EXPECT_NOT=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --expect)     EXPECT+=("$2");     shift 2 ;;
    --expect-not) EXPECT_NOT+=("$2"); shift 2 ;;
    *)            PROMPT="$1";        shift   ;;
  esac
done

if [[ -z "$PROMPT" ]]; then
  echo "Usage: agent-test.sh <prompt> [--expect <pattern>] [--expect-not <pattern>]" >&2
  exit 1
fi

MAKI="${MAKI:-maki}"
if ! command -v "$MAKI" &>/dev/null; then
  REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
  MAKI="$REPO_ROOT/target/release/maki"
fi

RESULT=$("$MAKI" "$PROMPT" --print --yolo --output-format json 2>/dev/null)
IS_ERROR=$(echo "$RESULT" | jq -r '.is_error')
TEXT=$(echo "$RESULT"     | jq -r '.result')

echo "─── agent output ───────────────────────────────────────────────"
echo "$TEXT"
echo "────────────────────────────────────────────────────────────────"

FAIL=0

if [[ "$IS_ERROR" == "true" ]]; then
  echo "FAIL: agent returned is_error=true"
  FAIL=1
fi

for pat in "${EXPECT[@]}"; do
  if ! echo "$TEXT" | grep -qi "$pat"; then
    echo "FAIL: expected pattern not found: $pat"
    FAIL=1
  fi
done

for pat in "${EXPECT_NOT[@]}"; do
  if echo "$TEXT" | grep -qi "$pat"; then
    echo "FAIL: forbidden pattern found: $pat"
    FAIL=1
  fi
done

if [[ "$FAIL" == "0" ]]; then
  echo "PASS"
else
  exit 1
fi
