# bench: a harness for optimizing the maki harness

Empirical loop for tuning `system.md`, prompt hints, skills, and tool descriptions
against open models (DeepSeek V4, Qwen 3.7, GLM 5.2). Measures *harness friction*,
not model capability: the same model, same tasks, different prompt/tool-description
variants, graded programmatically.

Builds on existing infra: `scripts/collect.py` (stream-json parsing, CSV append,
`--tag`), `scripts/analyze.py` (aggregation), `scripts/tbench_maki_agent.py`
(capability benchmarking stays in terminal-bench; this harness is for the config).

## Core mechanism: variant isolation via XDG_CONFIG_HOME

`maki_storage::paths` resolves the config dir through `etcetera`'s XDG strategy,
which honors `XDG_CONFIG_HOME`. The runner materializes each variant as a complete
config dir and launches:

    XDG_CONFIG_HOME=$RUN_TMP/config maki -p --yolo --verbose \
        --output-format stream-json --max-turns $N -m $MODEL "$PROMPT"

- `$RUN_TMP/config/maki/` = copy of the real `~/.config/maki` + variant overlay on top.
- Auth is in the data dir (not overridden), so credentials keep working.
- Parallel-safe: no shared mutable state between runs.
- Caveat: if `~/.maki` exists, `config_dir()` falls back to it and ignores XDG.
  The runner asserts `~/.maki` does not exist at startup.

## Directory layout

    bench/
      DESIGN.md
      bench.toml                  # models, reps, budgets, concurrency, provider pins
      tasks/
        <task-id>/
          task.toml               # prompt, category, limits, trap flags
          fixture/                # repo copied to a fresh workdir per run
          check.sh                # exit 0 = pass; runs in workdir after the agent
      variants/
        manifest.json             # lineage: variant -> parent, diff summary, status
        baseline/                 # config overlay (system.md, lua/, skills/)
        v01-edit-recovery/
          system.md
      runner.py                   # matrix executor
      mine.py                     # transcript -> failure-mode counters
      report.py                   # scorecards, paired deltas, accept/reject gates
      mutate.py                   # GEPA-lite mutation proposer (phase 3)
      results/
        bench.sqlite
        transcripts/<run_id>.jsonl
        workdirs/                 # kept on failure for debugging, else deleted

## Task specification

~20-30 tasks, small enough that a competent model finishes in <15 turns. Every
prompt rule being tested gets at least one *trap task* designed to trigger the
failure mode the rule targets.

```toml
# tasks/edit-ambiguous-oldstring/task.toml
[task]
id = "edit-ambiguous-oldstring"
category = "edit"        # search | edit | comprehension | workflow | trap
prompt = "In src/parser.rs, the third `advance()` call in `parse_block` must become `advance_n(2)`. Make that change."
max_turns = 15
timeout_s = 240

[check]
script = "check.sh"      # gets WORKDIR and RESULT_FILE env vars

[traps]                  # documents what the miner should watch for on this task
edit_multiple_matches = true
```

Task categories and examples:

| category      | example                                                | checker |
|---------------|--------------------------------------------------------|---------|
| search        | "report file:line where X is dispatched"               | regex on final result text |
| edit          | bug fix; ambiguous old_string; whitespace-sensitive hunk | `cargo test` / grep in workdir |
| comprehension | "how does session restore work" (3 key facts)          | keyword rubric or LLM judge |
| workflow      | 4-step task (does todo_write appear? all steps done?)  | file state + miner counters |
| trap          | 900-line file (index first?); info in gitignored dir (rg --no-ignore?) | miner counters + file state |

Fixtures are self-contained repos (`git init`-ed by the runner so `.gitignore`
semantics are real). The runner writes the agent's final `result` string to
`$WORKDIR/.bench/result.txt` for checkers that grade the response text.

Seed tasks from reality: mine past `runs.csv` / terminal-bench transcripts for
actual failures and reduce each to a minimal fixture.

## Runner (`runner.py`)

For each `(variant, model, task, rep)` cell:

1. Fresh workdir: copy fixture, `git init && git add -A && git commit`.
2. Materialize config: copy `~/.config/maki` -> tmp, overlay variant files.
3. Launch maki with the env/flags above; tee stdout to `transcripts/<run_id>.jsonl`.
4. Enforce `timeout_s` (SIGKILL process group; mark `timeout`).
5. Run `check.sh`; record pass/fail + checker stdout.
6. Insert row into sqlite; delete workdir on pass, keep on fail.

Run identity and provenance per row: `run_id`, variant name + content hash,
model string, task id, rep, `maki -V`, timestamp, temperature.

Matrix controls in `bench.toml`:

```toml
[matrix]
models = ["openrouter/deepseek/deepseek-v4", "openrouter/qwen/qwen-3.7", "openrouter/z-ai/glm-5.2"]
reps = 5
concurrency = 4

[budget]
max_usd_per_run = 0.50        # skip-remaining guard, checked from result events
max_usd_total = 25.0

[providers]
# OpenRouter routes across backends with different quantizations - the largest
# hidden variance source. Pin one provider per model or use direct endpoints.
"deepseek-v4" = { order = ["deepseek"], allow_fallbacks = false }
```

Rate limiting: exponential backoff on 429/5xx; a run that dies on transport
errors is retried once, then marked `infra_error` and excluded from stats
(never counted as task failure).

## Miner (`mine.py`)

Parses each transcript into counters. This is where prompt optimization gets
its signal — counters tell you *which rule to write next*. Definitions:

| counter | definition (from tool_use / tool_result pairs) |
|---|---|
| `edit_fail` | edit/multiedit result matches "not found" / "multiple locations" or `is_error` |
| `edit_recovered` | `edit_fail` followed by successful edit on same path within 3 calls |
| `write_fallback` | write to a path that had a prior `edit_fail` (destructive anti-pattern) |
| `reread_after_edit` | read(path) after successful edit(path), no intervening error on that path |
| `identical_repeat` | same (tool, canonicalized-input) called twice |
| `bash_file_ops` | bash command matching `^(cat|ls|head|tail|grep|sed -n|find)\b` |
| `bash_rg` | bash rg usage (tracked separately: legitimate after skill load) |
| `phantom_tool_call` | assistant *text* block containing tool-call-shaped JSON |
| `path_404` | read/edit/index result matching "not found" / "no such file" |
| `narration_turn` | non-final assistant turn with text but zero tool_use |
| `full_read_no_index` | read without offset/limit returning >100 lines, no prior index(path) |
| `todo_used` / `todo_updated` | todo_write called before work / updated per step (workflow tasks) |
| `skill_loaded` | skill calls by name |

Plus totals: turns, duration, tool calls per tool, tokens (in/out/cache), cost.
Stored long-format in a `counters(run_id, name, value)` table so new counters
never migrate the schema. `mine.py` is rerunnable over stored transcripts —
counters can be added later and backfilled.

## Report (`report.py`)

Per `(variant, model)` scorecard: success% (Wilson interval), mean cost and
median turns *on successes*, counter rates per run. Then paired comparison
vs baseline: pair runs by (task, rep), bootstrap the mean paired difference.

Acceptance gates (a variant is promotable iff, on **every** model):

1. success_delta >= -2pp (never trade correctness away), AND
2. cost_delta <= -10% on successes, OR the counter the variant targets drops >= 50%.

With 25 tasks x 5 reps = 125 paired points per model, ~10pp success effects and
~15% token effects are resolvable. Anything smaller: treat as noise, don't ship.

Output: markdown report per comparison, committed under `bench/results/reports/`
so the optimization history is reviewable.

## Mutation loop (`mutate.py`, phase 3 — GEPA-lite)

Automates step "read failing transcripts, write one rule":

1. Select current Pareto-best variant (success, cost).
2. Collect the K=5 worst transcripts per model + the counter summary.
   Compress transcripts: keep tool names, inputs truncated to 200 chars,
   result excerpts around errors; elide file dumps.
3. Ask a strong model (Opus/Fable) for: (a) one-paragraph diagnosis naming the
   dominant counter, (b) ONE minimal edit as a unified diff against the variant's
   `system.md` (or a tool description file).
4. Apply diff -> new variant dir -> full matrix -> gates.
5. Accept into the Pareto archive or reject; log lineage in `manifest.json`.

Default is human-in-the-loop: the diff is shown for approval before spending
the evaluation budget. `--auto N` runs N unattended generations.

## Tool-description variants (phase 4)

Tool descriptions are re-read by the model at every call decision — for open
models they outweigh the system prompt. Builtin descriptions live in compiled-in
Lua (`plugins/*/init.lua`), but the plugins config selects which builtins load
(`maki-config` `PluginsConfig`): a variant can disable a builtin and ship a
forked copy in its `lua/` overlay with a modified description. Same loop,
different mutation surface. (Verify the config knob name before building this.)

If per-model divergence shows up (a rule helps DeepSeek, hurts Qwen), prompt
hints can be made model-conditional via `register_prompt_hint` callbacks —
worth a maki feature request if callbacks can't currently see the model id.

## Cost envelope

25 tasks x 5 reps x 3 models ~ 375 runs x ~40k tokens ~ 15M tokens per variant
evaluation. On open-model pricing that's low single-digit dollars per iteration —
cheap enough to run nightly.

## Phasing

1. **Day 1**: 10 tasks, `runner.py`, sqlite, success/cost report. Already useful.
2. **Week 1**: full task suite, `mine.py` counters, paired report + gates.
3. **Later**: `mutate.py` loop; tool-description variants; per-model prompts.

## Verification checklist before building

- [ ] `XDG_CONFIG_HOME` override honored end-to-end (run `maki prompt system` with it set).
- [ ] `~/.maki` absent on the bench machine (else config fallback bypasses XDG).
- [ ] OpenRouter provider pinning available through maki's provider config;
      otherwise use direct DeepSeek/Qwen/GLM endpoints for benching.
- [ ] Temperature pinning per provider config; record actual value per run.
- [ ] stream-json `result` event includes usage for open-model providers (pricing
      table in `collect.py` is Anthropic-only; extend or compute from usage).
