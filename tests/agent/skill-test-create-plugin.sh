#!/usr/bin/env bash
# Tests the create-plugin skill by running each test prompt directly via maki --print.
# Bypasses the outer agent entirely (--no-plugins, --max-turns 1) to avoid hallucination.
# Test cases mirror the `tests:` block in ~/.config/maki/skills/create-plugin/SKILL.md.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
MAKI="${MAKI:-$REPO/target/release/maki}"
SKILL_FILE="${HOME}/.config/maki/skills/create-plugin/SKILL.md"

if [[ ! -f "$SKILL_FILE" ]]; then
  echo "SKIP: skill file not found: $SKILL_FILE" >&2
  exit 0
fi

# Strip YAML frontmatter to get the skill body
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
    echo "  ERROR: $text"
    FAIL=1
    return
  fi

  local ok=1
  IFS='|' read -ra pats <<< "$expect"
  for pat in "${pats[@]}"; do
    if echo "$text" | grep -qi "$pat"; then
      echo "  ✓ contains: $pat"
    else
      echo "  ✗ missing:  $pat"
      ok=0; FAIL=1
    fi
  done

  if [[ -n "$expect_not" ]]; then
    IFS='|' read -ra pats <<< "$expect_not"
    for pat in "${pats[@]}"; do
      if echo "$text" | grep -qi "$pat"; then
        echo "  ✗ forbidden: $pat"
        ok=0; FAIL=1
      else
        echo "  ✓ not present: $pat"
      fi
    done
  fi

  [[ "$ok" == "1" ]] && echo "  → PASS" || echo "  → FAIL"
}

run_test 1 \
  "Show me how to create a basic Lua plugin for Maki that fetches data from an external API." \
  "maki.api.register_tool|maki.net.request|init.lua|handler" \
  "permission_scope = \"url\""

run_test 2 \
  "What is the permission_scope field for in maki.api.register_tool, and when should I omit it?" \
  "schema|string" \
  ""

run_test 3 \
  "How do I wire a new plugin into the Maki binary after writing init.lua?" \
  "loader.rs|BUNDLED_PLUGINS|DEFAULT_BUILTINS" \
  ""

echo ""
[[ "$FAIL" == "0" ]] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
