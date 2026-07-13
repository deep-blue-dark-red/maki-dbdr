# Skill Testing Architecture

Maki skills are markdown playbooks that shape LLM behavior. This document covers how to write automated behavioral tests for them, the architecture behind the test runner, and lessons learned from the first implementation.

---

## Overview

A skill test answers one question: **given this skill as context, does the LLM produce output that respects the skill's instructions?**

Tests are:
- **Behavioral** — they check LLM output, not code correctness
- **Substring-based** — `expect_contains` / `expect_not_contains` matches, case-insensitive
- **Deterministic** — the LLM is the only variable; no outer agent, no hallucination risk
- **Self-contained** — test cases live in the SKILL.md frontmatter alongside the skill they test

---

## Test case format

Test cases live in the `tests:` key of a SKILL.md's YAML frontmatter:

```yaml
---
name: my-skill
description: What this skill does.
tests:
  - prompt: "A natural-language question a user might ask when invoking this skill"
    expect_contains:
      - "term that must appear in the LLM response"
      - "another required term"
    expect_not_contains:
      - "term that must NOT appear"
---

# Skill body starts here...
```

### Writing good test cases

**Prompts should be specific enough to have a deterministic signal.**
Bad: `"How do I make a plugin?"` — the LLM can answer from training data without reading the skill.
Good: `"What Rust file do I edit to register a new bundled plugin in Maki?"` — only answerable correctly from the skill.

**`expect_contains` terms should be exact tokens from the skill body.**
If the skill says use `maki.api.register_tool`, test for `maki.api.register_tool`. If it says edit `loader.rs`, test for `loader.rs`. Terms that appear verbatim in the skill will reliably appear in a response that actually followed the skill.

**`expect_not_contains` guards against wrong-path answers.**
Example: if the skill documents that `permission_scope` must reference an existing schema field, test that a response doesn't include `permission_scope = "url"` when the schema has no `url` field.

**One concern per test case.** Don't bundle unrelated assertions — a multi-concern failure is harder to interpret and fix.

---

## How tests run

### Direct invocation (shell test scripts)

The primary test mechanism bypasses the LLM agent entirely and calls `maki --print` directly with the skill body injected as a system prompt:

```
┌──────────────────────────────────────────────┐
│  tests/agent/skill-test-<name>.sh            │
│                                              │
│  1. Read SKILL.md, strip YAML frontmatter    │
│  2. For each test case:                      │
│     maki "<prompt>"                          │
│       --print --yolo                         │
│       --max-turns 1                          │
│       --append-system-prompt "<skill-body>"  │
│       --output-format json                   │
│  3. Parse JSON: .result, .is_error           │
│  4. grep -qi each expect_contains term       │
│  5. Report PASS / FAIL, exit 1 on failure    │
└──────────────────────────────────────────────┘
```

`--max-turns 1` means a single LLM call with no tool use. The LLM reads the skill body (injected via `--append-system-prompt`) and answers the prompt. No outer agent, no possibility of hallucinating a test result.

Key flags:
| Flag | Purpose |
|------|---------|
| `--print` | Headless mode, no TUI |
| `--yolo` | Skip permission prompts |
| `--max-turns 1` | Single LLM call, no tool-use loop |
| `--append-system-prompt` | Inject skill body after the default system prompt |
| `--output-format json` | Machine-parseable result with `is_error` field |

### `skill_test` agent tool

The `skill_test` tool (registered in `plugins/skill/init.lua`) provides the same capability through the agent. When invoked, it:

1. Discovers the named skill using the standard skill search path
2. Reads and parses the SKILL.md frontmatter with `maki.yaml.decode`
3. Extracts the `tests:` array
4. For each test case, spawns a `maki --print` subprocess using `maki.fn.jobstart`
5. Parses the JSON result and checks `expect_contains` / `expect_not_contains`
6. Returns a markdown report with per-test pass/fail status

Usage from within maki:
```
use skill_test to test the create-plugin skill
```

The tool uses `maki.fn.jobwait` to block the Lua handler until all subprocesses complete (60s timeout per test). It sets `is_error = true` in its output when any test fails, so the agent can surface the failure clearly.

The tool finds the maki binary by scanning for `target/release/maki` up the directory tree from cwd, falling back to `maki` on PATH.

---

## Architecture: `--append-system-prompt` in headless mode

During implementation, `--append-system-prompt` and `--system-prompt` were discovered to be **silently ignored** in `--print` mode. They were only wired into SDK mode (`--input-format stream-json`). This was fixed by:

**`maki-agent/src/headless.rs`** — Added two fields to `HeadlessParams`:
```rust
pub system_prompt_override: Option<String>,
pub append_system_prompt: Option<String>,
```
The `spawn()` function now checks these before calling `agent::build_system_prompt()`. If `system_prompt_override` is set, it replaces the default entirely. If `append_system_prompt` is set, it appends after the default.

**`src/print.rs`** — Extended `print::run()` signature to accept these fields and pass them through to `HeadlessParams`.

**`src/cmd/tui.rs`** — Forwarded `cli.system_prompt` and `cli.append_system_prompt` into the `print::run()` call.

This means `--append-system-prompt` now works as documented in both `--print` and SDK modes.

---

## File layout

```
maki/
  plugins/skill/init.lua          ← skill_test tool (registered alongside skill tool)
  scripts/agent-test.sh           ← thin wrapper: invokes maki --print, checks output
  tests/agent/
    skill-test-create-plugin.sh   ← test script for the create-plugin skill
  Makefile                        ← make test-agent runs all tests/agent/*.sh
  SKILL_TESTING.md                ← this document

~/.config/maki/skills/
  create-plugin/SKILL.md          ← has tests: frontmatter block
  create-skill/SKILL.md           ← documents the tests: format
```

---

## Running tests

```bash
# Run a single skill test
bash tests/agent/skill-test-create-plugin.sh

# Run all agent tests
make test-agent

# Run skill_test from within maki (interactive or headless)
maki "use skill_test to test the create-plugin skill" --print --yolo
```

`MAKI` env var overrides the binary path:
```bash
MAKI=/usr/local/bin/maki bash tests/agent/skill-test-create-plugin.sh
```

---

## Writing a new skill test script

```bash
#!/usr/bin/env bash
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
MAKI="${MAKI:-$REPO/target/release/maki}"
SKILL_FILE="${HOME}/.config/maki/skills/<name>/SKILL.md"

if [[ ! -f "$SKILL_FILE" ]]; then
  echo "SKIP: skill file not found: $SKILL_FILE" >&2; exit 0
fi

SKILL_BODY=$(awk 'BEGIN{n=0} /^---/{n++; if(n>=2){found=1}; next} found{print}' "$SKILL_FILE")
FAIL=0

run_test() {
  local num="$1" prompt="$2" expect="$3" expect_not="${4:-}"
  printf "\nTest %d: %s...\n" "$num" "${prompt:0:70}"
  local result text is_err
  result=$("$MAKI" "$prompt" --print --yolo --max-turns 1 \
    --append-system-prompt "$SKILL_BODY" --output-format json 2>/dev/null)
  text=$(echo "$result"   | jq -r '.result // empty')
  is_err=$(echo "$result" | jq -r '.is_error')
  [[ "$is_err" == "true" ]] && { echo "  ERROR: $text"; FAIL=1; return; }
  local ok=1
  IFS='|' read -ra pats <<< "$expect"
  for pat in "${pats[@]}"; do
    echo "$text" | grep -qi "$pat" && echo "  ✓ $pat" || { echo "  ✗ missing: $pat"; ok=0; FAIL=1; }
  done
  if [[ -n "$expect_not" ]]; then
    IFS='|' read -ra pats <<< "$expect_not"
    for pat in "${pats[@]}"; do
      echo "$text" | grep -qi "$pat" && { echo "  ✗ forbidden: $pat"; ok=0; FAIL=1; } || echo "  ✓ not: $pat"
    done
  fi
  [[ "$ok" == "1" ]] && echo "  → PASS" || echo "  → FAIL"
}

run_test 1 "your prompt here" "expected|terms|pipe-separated" "forbidden|terms"
# add more run_test calls...

echo ""
[[ "$FAIL" == "0" ]] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
```

---

## Lessons learned

### The outer agent hallucination trap

The first iteration of `agent-test.sh` asked an outer maki agent to "use skill_test to test the create-plugin skill" and checked the output for the word "passed." The agent hallucinated convincing fake test results — fabricating test names like "Verify plugin API compatibility with neovim" — and the check accepted them.

**Root cause:** Asking an agent to voluntarily call a tool and then checking its narrative output is not a test. The agent can satisfy the string check without touching the tool.

**Fix:** Remove the outer agent from the test loop entirely. Call `maki --print --max-turns 1` directly so the LLM produces one response with no tool-use opportunity. Inject the skill as context. Check the actual LLM response.

### `--append-system-prompt` was silently ignored in `--print` mode

The CLI accepted the flag but `print.rs` never forwarded it to `HeadlessParams`. The LLM fell back to its training data and produced plausible-sounding but wrong answers that didn't mention any Maki-specific APIs.

**Fix:** Wire both `system_prompt_override` and `append_system_prompt` through `HeadlessParams` → `spawn()` in headless mode, matching the existing handling in interactive/SDK mode.

### `expect_contains` terms must be traceable to the skill body

A term like "passed" or "handler" can appear in any plausible response, making the test vacuous. Terms should be specific enough that they would only naturally appear if the LLM actually read and followed the skill — e.g., `maki.api.register_tool`, `BUNDLED_PLUGINS`, `loader.rs`.

### Tests are probabilistic, not deterministic

LLM outputs vary across runs. A test that passes once may fail on retry if the model phrases things differently. Mitigations:
- Use terms that appear verbatim in the skill body (the LLM tends to echo back exact names from its context)
- Avoid testing for prose phrasing; test for identifiers, file names, function names
- Accept occasional flakiness — a behavioral smoke test at 95% reliability is still valuable
