#!/usr/bin/env bash
# Tests the ssh skill by running each test prompt directly via maki --print.
# Test cases mirror the `tests:` block in ~/.config/maki/skills/ssh/SKILL.md.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
MAKI="${MAKI:-$REPO/target/release/maki}"
SKILL_FILE="${HOME}/.config/maki/skills/ssh/SKILL.md"

if [[ ! -f "$SKILL_FILE" ]]; then
  echo "SKIP: skill file not found: $SKILL_FILE" >&2
  exit 0
fi

SKILL_BODY=$(awk 'BEGIN{n=0} /^---/{n++; if(n>=2){found=1}; next} found{print}' "$SKILL_FILE")
FAIL=0

run_test() {
  local num="$1" prompt="$2" expect="$3" expect_not="${4:-}"
  printf "\nTest %d: %s...\n" "$num" "${prompt:0:70}"

  local result text is_err
  result=$("$MAKI" "$prompt" \
    --print --yolo --max-turns 1 \
    --append-system-prompt "$SKILL_BODY" \
    --output-format json 2>/dev/null)

  text=$(echo "$result"   | jq -r '.result // empty')
  is_err=$(echo "$result" | jq -r '.is_error')

  if [[ "$is_err" == "true" ]]; then
    echo "  ERROR: $text"; FAIL=1; return
  fi

  local ok=1
  IFS='|' read -ra pats <<< "$expect"
  for pat in "${pats[@]}"; do
    if echo "$text" | grep -qi "$pat"; then
      echo "  ✓ contains: $pat"
    else
      echo "  ✗ missing:  $pat"; ok=0; FAIL=1
    fi
  done

  if [[ -n "$expect_not" ]]; then
    IFS='|' read -ra pats <<< "$expect_not"
    for pat in "${pats[@]}"; do
      if echo "$text" | grep -qi "$pat"; then
        echo "  ✗ forbidden: $pat"; ok=0; FAIL=1
      else
        echo "  ✓ not present: $pat"
      fi
    done
  fi

  [[ "$ok" == "1" ]] && echo "  → PASS" || echo "  → FAIL"
}

run_test 1 \
  "How do I run a one-off command on a remote machine?" \
  "ssh|user@host" \
  "webfetch|wget"

run_test 2 \
  "What is the right way to copy files to a remote machine?" \
  "rsync" \
  "webfetch"

run_test 3 \
  "How do I keep an SSH connection alive and reuse it for multiple commands?" \
  "ControlMaster|ControlPath" \
  ""

run_test 4 \
  "How do I connect to a machine that is only reachable through a bastion host?" \
  "ProxyJump" \
  ""

echo ""
[[ "$FAIL" == "0" ]] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
