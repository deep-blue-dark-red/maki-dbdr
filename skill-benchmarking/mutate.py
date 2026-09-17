#!/usr/bin/env python3
"""GEPA-lite: reflective prompt evolution for the maki harness.

One generation:
  1. pick the current Pareto-best variant (the "parent")
  2. gather its worst transcripts + counter summary per model
  3. ask a strong reflector model for a diagnosis + ONE minimal diff to a
     single config file (system.md or a tool description)
  4. materialize the diff as a new variant dir
  5. evaluate parent + child on the same matrix (runner.py)
  6. compare (report.py gates); accept into the archive or reject
  7. append lineage to variants/manifest.json

Human-in-the-loop by default: the proposed diff is shown for approval before
any evaluation budget is spent. --auto N runs N generations unattended.

STATUS: SKETCH. The reflect() call and diff application are stubbed where noted.
Wire `reflect()` to maki itself (maki -p, cheap for reflection) or the Anthropic
API before running --auto.

Design choices worth keeping:
  - Mutate ONE file per generation. Prompt rules interact; batching changes
    destroys attribution of which change moved the metric.
  - Archive is a Pareto front over (mean success, mean tokens-on-success),
    not a single "best". A change that trades a little success for a lot of
    token savings stays as a separate non-dominated point; the human picks
    the shipping tradeoff.
  - Reflector sees COMPRESSED transcripts (tool names + truncated inputs +
    error excerpts), never raw file dumps. Keeps the reflection prompt small
    and focused on behavior, which is what we can actually change.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sqlite3
import subprocess
import sys
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path

import mine

BENCH = Path(__file__).resolve().parent
VARIANTS = BENCH / "variants"
MANIFEST = VARIANTS / "manifest.json"
DB_PATH = BENCH / "results" / "bench.sqlite"

# Files a mutation is allowed to touch. Anything else is out of scope for the
# automated loop (fixtures, runner, this file).
MUTABLE_GLOBS = ("system.md", "skills/*/SKILL.md", "lua/*.lua")

WORST_TRANSCRIPTS_PER_MODEL = 4
MAX_TOOL_INPUT_CHARS = 200
MAX_RESULT_CHARS = 300


# --------------------------------------------------------------------------
# archive / lineage
# --------------------------------------------------------------------------

@dataclass
class VariantStats:
    name: str
    n: int = 0
    passes: float = 0.0
    tokens_ok: list[int] = field(default_factory=list)
    counters: dict[str, float] = field(default_factory=dict)

    @property
    def success(self) -> float:
        return self.passes / self.n if self.n else 0.0

    @property
    def mean_tokens(self) -> float:
        return sum(self.tokens_ok) / len(self.tokens_ok) if self.tokens_ok else float("inf")


def load_stats(db_path: Path) -> dict[str, VariantStats]:
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    stats: dict[str, VariantStats] = {}
    for r in conn.execute("SELECT * FROM runs WHERE status != 'infra_error'"):
        s = stats.setdefault(r["variant"], VariantStats(r["variant"]))
        s.n += 1
        if r["status"] == "pass":
            s.passes += 1
            s.tokens_ok.append((r["input_tokens"] or 0) + (r["output_tokens"] or 0))
    ctr = conn.execute(
        "SELECT v.variant, c.name, AVG(c.value) FROM counters c "
        "JOIN runs v ON v.run_id = c.run_id "
        "WHERE v.status != 'infra_error' GROUP BY v.variant, c.name"
    )
    for variant, name, avg in ctr:
        if variant in stats:
            stats[variant].counters[name] = avg
    conn.close()
    return stats


def pareto_front(stats: dict[str, VariantStats]) -> list[str]:
    """Non-dominated variants on (success high, tokens low)."""
    names = [s for s in stats.values() if s.n]
    front = []
    for a in names:
        dominated = any(
            b.name != a.name
            and b.success >= a.success
            and b.mean_tokens <= a.mean_tokens
            and (b.success > a.success or b.mean_tokens < a.mean_tokens)
            for b in names
        )
        if not dominated:
            front.append(a.name)
    return front


def select_parent(stats: dict[str, VariantStats]) -> str:
    """Pick the Pareto point with the highest success (ties: fewer tokens)."""
    front = pareto_front(stats)
    if not front:
        sys.exit("no evaluated variants to mutate; run the baseline first")
    return max(front, key=lambda n: (stats[n].success, -stats[n].mean_tokens))


# --------------------------------------------------------------------------
# reflection input: dominant failure + compressed transcripts
# --------------------------------------------------------------------------

COUNTER_TO_RULE_HINT = {
    "edit_fail": "how to construct old_string so it matches on the first try",
    "write_fallback": "forbidding write as a fallback after a failed edit",
    "reread_after_edit": "not re-reading a file after a successful edit",
    "identical_repeat": "not repeating an identical failed tool call",
    "bash_file_ops": "using read/grep/glob tools instead of bash cat/ls/grep",
    "phantom_tool_call": "emitting real tool calls, never tool JSON as text",
    "path_404": "searching for a path before reading it",
    "narration_turn": "acting with tools instead of narrating intent",
    "full_read_no_index": "indexing large files before reading a line range",
}


def dominant_counter(parent: VariantStats) -> str | None:
    ranked = sorted(
        ((k, v) for k, v in parent.counters.items() if k in COUNTER_TO_RULE_HINT),
        key=lambda kv: kv[1], reverse=True,
    )
    return ranked[0][0] if ranked and ranked[0][1] > 0 else None


def worst_transcripts(db_path: Path, variant: str) -> list[dict]:
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    rows = [dict(r) for r in conn.execute(
        "SELECT * FROM runs WHERE variant = ? AND status != 'infra_error'", (variant,)
    )]
    conn.close()
    picked: list[dict] = []
    by_model: dict[str, list[dict]] = {}
    for r in rows:
        by_model.setdefault(r["model"], []).append(r)
    for model, rs in by_model.items():
        rs.sort(key=lambda r: (r["status"] == "pass", r["num_turns"] or 0), reverse=False)
        picked.extend(rs[:WORST_TRANSCRIPTS_PER_MODEL])
    return picked


def compress_transcript(path: Path) -> list[str]:
    """Flatten a transcript into short behavioral lines for the reflector."""
    lines: list[str] = []
    by_id: dict[str, str] = {}
    for msg in mine._events(path):
        t = msg.get("type")
        if t == "assistant":
            for b in msg.get("message", {}).get("content", []) or []:
                if not isinstance(b, dict):
                    continue
                if b.get("type") == "text" and b.get("text", "").strip():
                    lines.append(f"  say: {b['text'].strip()[:MAX_RESULT_CHARS]}")
                elif b.get("type") == "tool_use":
                    inp = json.dumps(b.get("input", {}), default=str)[:MAX_TOOL_INPUT_CHARS]
                    lines.append(f"  call {b.get('name')}({inp})")
                    if b.get("id"):
                        by_id[b["id"]] = b.get("name", "?")
        elif t == "user":
            for b in msg.get("message", {}).get("content", []) or []:
                if isinstance(b, dict) and b.get("type") == "tool_result":
                    txt = mine._result_text(b)
                    tag = "ERR" if b.get("is_error") else "ok"
                    name = by_id.get(b.get("tool_use_id"), "?")
                    lines.append(f"  -> {name} [{tag}]: {txt.strip()[:MAX_RESULT_CHARS]}")
    return lines


def build_reflection_prompt(parent_name: str, parent_dir: Path,
                            counter: str | None, transcripts: list[dict]) -> str:
    target_file = parent_dir / "system.md"
    current = target_file.read_text()

    hint = COUNTER_TO_RULE_HINT.get(counter or "", "the most common failure you observe")
    blocks = []
    for r in transcripts:
        tp = mine.resolve_transcript(Path(r["transcript"]))
        body = "\n".join(compress_transcript(tp)) if tp.exists() else "  (transcript missing)"
        blocks.append(
            f"### {r['task']} on {r['model'].rsplit('/', 1)[-1]} [{r['status']}]\n{body}"
        )
    transcript_text = "\n\n".join(blocks)

    return f"""You are tuning the system prompt of a CLI coding agent (maki) so that
weaker open models (DeepSeek, Qwen, GLM) use its tools more effectively.

The dominant measured failure mode is: {counter or 'unknown'}.
Focus your change on: {hint}.

Below are compressed transcripts of the worst runs. Each line is a tool call,
its result (ok/ERR), or agent text. Diagnose the ROOT behavioral cause.

<transcripts>
{transcript_text}
</transcripts>

Here is the current system prompt you may edit (this file only):

<system_prompt>
{current}
</system_prompt>

Respond with a JSON object, nothing else:
{{
  "diagnosis": "one paragraph naming the failure and its root cause",
  "target_file": "system.md",
  "search": "exact substring to replace (must appear verbatim above)",
  "replace": "the replacement text",
  "rationale": "why this single change should reduce {counter or 'the failure'}"
}}

Change ONE thing. Keep every {{{{slot}}}} marker intact. Prefer editing or adding
a single rule over rewriting sections."""


def reflect(prompt: str, reflector_model: str, maki_bin: str) -> dict:
    """Call the reflector model, return the parsed mutation proposal.

    SKETCH: shells out to `maki -p` for a one-shot completion. Swap for a direct
    API call if you want structured-output guarantees. Must return a dict with
    keys: diagnosis, target_file, search, replace, rationale.
    """
    proc = subprocess.run(
        [maki_bin, "-p", "--output-format", "text", "-m", reflector_model, prompt],
        capture_output=True, text=True, timeout=300,
    )
    out = proc.stdout.strip()
    start, end = out.find("{"), out.rfind("}")
    if start < 0 or end < 0:
        raise ValueError(f"reflector returned no JSON object:\n{out[:500]}")
    return json.loads(out[start:end + 1])


# --------------------------------------------------------------------------
# apply / evaluate / accept
# --------------------------------------------------------------------------

def next_variant_name(parent: str) -> str:
    existing = {d.name for d in VARIANTS.iterdir() if d.is_dir()}
    n = 1
    while f"v{n:02d}-gen" in existing:
        n += 1
    return f"v{n:02d}-gen"


def materialize(parent_dir: Path, name: str, proposal: dict) -> Path:
    new_dir = VARIANTS / name
    shutil.copytree(parent_dir, new_dir)
    target = new_dir / proposal["target_file"]
    if not any(target.match(g) or target.name == g for g in ("system.md",)) \
            and proposal["target_file"] not in _mutable_files(new_dir):
        raise ValueError(f"target_file {proposal['target_file']} is not mutable")
    text = target.read_text()
    search, replace = proposal["search"], proposal["replace"]
    if search not in text:
        raise ValueError("proposal.search not found verbatim in target file")
    if text.count(search) > 1:
        raise ValueError("proposal.search is ambiguous (matches multiple times)")
    target.write_text(text.replace(search, replace, 1))
    _assert_slots_intact(parent_dir / "system.md", new_dir / "system.md")
    return new_dir


def _mutable_files(variant_dir: Path) -> set[str]:
    out = set()
    for g in MUTABLE_GLOBS:
        out.update(str(p.relative_to(variant_dir)) for p in variant_dir.glob(g))
    return out


def _assert_slots_intact(before: Path, after: Path) -> None:
    import re
    slots_before = set(re.findall(r"\{\{(\w+)\}\}", before.read_text()))
    slots_after = set(re.findall(r"\{\{(\w+)\}\}", after.read_text()))
    missing = slots_before - slots_after
    if missing:
        raise ValueError(f"mutation dropped prompt slots: {missing}")


def evaluate(names: list[str], extra_runner_args: list[str]) -> None:
    subprocess.run(
        [sys.executable, str(BENCH / "runner.py"), "--variants", *names, *extra_runner_args],
        check=True,
    )


def gate(parent: str, child: str, counter: str | None) -> bool:
    cmd = [sys.executable, str(BENCH / "report.py"), "compare",
           "--baseline", parent, "--variant", child]
    if counter:
        cmd += ["--target-counter", counter]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    print(proc.stdout)
    return proc.stdout.strip().endswith("ACCEPT")


def record_lineage(child: str, parent: str, proposal: dict, accepted: bool) -> None:
    manifest = json.loads(MANIFEST.read_text()) if MANIFEST.exists() else {}
    manifest[child] = {
        "parent": parent,
        "created": date.today().isoformat(),
        "diagnosis": proposal.get("diagnosis", ""),
        "rationale": proposal.get("rationale", ""),
        "target_file": proposal.get("target_file", ""),
        "status": "accepted" if accepted else "rejected",
    }
    MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n")


def generation(cfg: dict, auto: bool, extra_runner_args: list[str]) -> bool:
    stats = load_stats(DB_PATH)
    parent = select_parent(stats)
    parent_dir = VARIANTS / parent
    counter = dominant_counter(stats[parent])
    print(f"parent: {parent} (success {stats[parent].success:.0%}, "
          f"dominant counter: {counter or 'none'})", file=sys.stderr)

    transcripts = worst_transcripts(DB_PATH, parent)
    prompt = build_reflection_prompt(parent, parent_dir, counter, transcripts)

    reflector = cfg.get("mutate", {}).get("reflector_model", "openrouter/anthropic/claude-fable-5")
    maki_bin = cfg.get("run", {}).get("maki_bin", "maki")
    proposal = reflect(prompt, reflector, maki_bin)

    print("\n=== proposed mutation ===", file=sys.stderr)
    print(f"diagnosis: {proposal['diagnosis']}", file=sys.stderr)
    print(f"file:      {proposal['target_file']}", file=sys.stderr)
    print(f"- {proposal['search'][:200]}", file=sys.stderr)
    print(f"+ {proposal['replace'][:200]}", file=sys.stderr)
    print(f"rationale: {proposal['rationale']}\n", file=sys.stderr)

    if not auto:
        if input("apply and evaluate this mutation? [y/N] ").strip().lower() != "y":
            print("skipped", file=sys.stderr)
            return False

    child = next_variant_name(parent)
    child_dir = materialize(parent_dir, child, proposal)
    print(f"materialized {child}", file=sys.stderr)

    evaluate([parent, child], extra_runner_args)
    accepted = gate(parent, child, counter)
    record_lineage(child, parent, proposal, accepted)

    if not accepted:
        print(f"REJECTED {child} (kept for inspection under variants/)", file=sys.stderr)
    else:
        print(f"ACCEPTED {child}", file=sys.stderr)
    return accepted


def main() -> None:
    import tomllib
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--auto", type=int, default=0,
                    help="Run N generations without approval prompts")
    ap.add_argument("--runner-args", nargs=argparse.REMAINDER, default=[],
                    help="Everything after this is passed to runner.py (e.g. --reps 3)")
    args = ap.parse_args()

    with open(BENCH / "bench.toml", "rb") as f:
        cfg = tomllib.load(f)

    if args.auto:
        for i in range(args.auto):
            print(f"\n===== generation {i + 1}/{args.auto} =====", file=sys.stderr)
            generation(cfg, auto=True, extra_runner_args=args.runner_args)
    else:
        generation(cfg, auto=False, extra_runner_args=args.runner_args)


if __name__ == "__main__":
    main()
