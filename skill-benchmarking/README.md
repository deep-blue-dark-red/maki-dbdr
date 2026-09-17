# bench

Harness-optimization benchmark for maki. Measures how prompt/tool-description
variants affect open-model behavior on small graded tasks. See DESIGN.md for
the full rationale.

## Quickstart

```bash
cd bench

# Show the matrix without running anything
python3 runner.py --dry-run

# Smoke test: one task, one model, one rep
python3 runner.py --tasks search-find-def \
    --models openrouter/deepseek/deepseek-v4-flash --reps 1

# Full baseline evaluation (uses bench.toml matrix)
python3 runner.py

# Results
python3 report.py scorecard
python3 report.py compare --baseline baseline --variant v01-my-change \
    --target-counter edit_fail

# Re-mine counters after changing mine.py (no re-running needed)
python3 mine.py --db results/bench.sqlite

# Inspect a single transcript's counters
python3 mine.py results/transcripts/<run>.jsonl
```

## Adding a variant

```bash
cp -r variants/baseline variants/v01-my-change
$EDITOR variants/v01-my-change/system.md      # change ONE thing
python3 runner.py --variants baseline v01-my-change
python3 report.py compare --baseline baseline --variant v01-my-change
```

Variant dirs are overlays on top of the live `~/.config/maki`: any file present
in the variant replaces the live one for that run (via `XDG_CONFIG_HOME`).
Keep every prompt template slot (`{{identity}}`, `{{tool_usage}}`,
`{{conventions}}`, `{{instructions}}`, `{{after_instructions}}`, ...) — builtin
plugins target them and fail to load if a slot is missing.

Record the variant's intent in `variants/manifest.json`.

## Adding a task

```
tasks/<id>/
  task.toml    # [task] id, category, prompt, max_turns, timeout_s
  fixture/     # copied to a fresh git repo per run
  check.sh     # exit 0 = pass; env: WORKDIR, RESULT_FILE (agent's final text)
```

Categories: search, edit, comprehension, workflow, trap. Trap tasks are built
to trigger one specific failure mode (ambiguous old_string, tab-indented files,
gitignored answers, oversized files).
