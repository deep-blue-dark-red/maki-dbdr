#!/usr/bin/env python3
"""Scorecards and paired variant comparison with acceptance gates.

Usage:
  report.py scorecard
  report.py compare --baseline baseline --variant v01-edit-recovery [--target-counter edit_fail]
"""

from __future__ import annotations

import argparse
import math
import random
import sqlite3
import sys
from collections import defaultdict
from pathlib import Path

DB_PATH = Path(__file__).resolve().parent / "results" / "bench.sqlite"

KEY_COUNTERS = (
    "edit_fail",
    "write_fallback",
    "reread_after_edit",
    "identical_repeat",
    "bash_file_ops",
    "phantom_tool_call",
    "path_404",
    "narration_turn",
    "full_read_no_index",
)
BOOTSTRAP_N = 10_000
Z95 = 1.96
GATE_SUCCESS_DROP_PP = -2.0
GATE_TOKEN_CUT = -0.10
GATE_COUNTER_CUT = -0.50


def load(db_path: Path) -> list[dict]:
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    runs = [dict(r) for r in conn.execute("SELECT * FROM runs WHERE status != 'infra_error'")]
    counters = defaultdict(dict)
    for run_id, name, value in conn.execute("SELECT run_id, name, value FROM counters"):
        counters[run_id][name] = value
    conn.close()
    for r in runs:
        r["counters"] = counters.get(r["run_id"], {})
        r["ok"] = 1.0 if r["status"] == "pass" else 0.0
        r["tokens"] = (r["input_tokens"] or 0) + (r["output_tokens"] or 0)
    return runs


def wilson(passes: float, n: int) -> tuple[float, float]:
    if n == 0:
        return 0.0, 0.0
    p = passes / n
    denom = 1 + Z95**2 / n
    center = (p + Z95**2 / (2 * n)) / denom
    margin = Z95 * math.sqrt(p * (1 - p) / n + Z95**2 / (4 * n**2)) / denom
    return max(0.0, center - margin), min(1.0, center + margin)


def fmt_table(headers: list[str], rows: list[list[str]]) -> str:
    widths = [max(len(str(c)) for c in [h] + [r[i] for r in rows]) for i, h in enumerate(headers)]
    out = [
        " | ".join(str(h).ljust(w) for h, w in zip(headers, widths)),
        "-|-".join("-" * w for w in widths),
    ]
    out += [" | ".join(str(c).ljust(w) for c, w in zip(r, widths)) for r in rows]
    return "\n".join(out)


def scorecard(runs: list[dict]) -> None:
    groups: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for r in runs:
        groups[(r["variant"], r["model"])].append(r)

    headers = ["variant", "model", "n", "pass%", "95% CI", "tok(ok)", "turns(ok)", "$/run"]
    rows = []
    for (variant, model), rs in sorted(groups.items()):
        n = len(rs)
        passes = sum(r["ok"] for r in rs)
        lo, hi = wilson(passes, n)
        oks = [r for r in rs if r["ok"]]
        tok = int(sum(r["tokens"] for r in oks) / len(oks)) if oks else 0
        turns = sorted(r["num_turns"] for r in oks)[len(oks) // 2] if oks else 0
        cost = sum(r["cost_usd"] or 0 for r in rs) / n
        rows.append([
            variant, model.rsplit("/", 1)[-1], n,
            f"{100 * passes / n:.0f}%", f"[{100 * lo:.0f},{100 * hi:.0f}]",
            f"{tok:,}", turns, f"{cost:.4f}",
        ])
    print("## Scorecard\n")
    print(fmt_table(headers, rows))

    print("\n## Failure-mode counter rates (mean per run)\n")
    headers = ["variant", "model"] + list(KEY_COUNTERS)
    rows = []
    for (variant, model), rs in sorted(groups.items()):
        vals = [
            f"{sum(r['counters'].get(c, 0) for r in rs) / len(rs):.2f}"
            for c in KEY_COUNTERS
        ]
        rows.append([variant, model.rsplit("/", 1)[-1]] + vals)
    print(fmt_table(headers, rows))


def by_task(runs: list[dict], variant: str | None) -> None:
    if variant:
        runs = [r for r in runs if r["variant"] == variant]
    models = sorted({r["model"] for r in runs})
    tasks = sorted({r["task"] for r in runs})
    headers = ["task"] + [m.rsplit("/", 1)[-1] for m in models]
    rows = []
    for task in tasks:
        row = [task]
        for model in models:
            rs = [r for r in runs if r["task"] == task and r["model"] == model]
            if not rs:
                row.append("-")
            else:
                row.append(f"{sum(r['ok'] for r in rs):.0f}/{len(rs)}")
        rows.append(row)
    print(f"## Pass rate by task{f' ({variant})' if variant else ''}\n")
    print(fmt_table(headers, rows))


def failures(runs: list[dict], variant: str | None, model: str | None,
             task: str | None, limit: int) -> None:
    bad = [
        r for r in runs
        if r["status"] != "pass"
        and (not variant or r["variant"] == variant)
        and (not model or model in r["model"])
        and (not task or r["task"] == task)
    ]
    bad.sort(key=lambda r: r["ts"], reverse=True)
    print(f"{len(bad)} non-pass runs" + (f", showing {limit}" if len(bad) > limit else ""))
    for r in bad[:limit]:
        print(f"\n### {r['run_id']}  [{r['status']}]")
        print(f"- transcript: {r['transcript']}")
        print(f"- artifacts:  results/runs/{r['run_id']}/")
        snippet = (r["check_output"] or "").strip()
        if snippet:
            print(f"- check output: {snippet[:400]}")
        hot = {k: v for k, v in r["counters"].items()
               if k in KEY_COUNTERS and v > 0}
        if hot:
            print(f"- counters: {hot}")


def bootstrap_ci(diffs: list[float]) -> tuple[float, float]:
    if not diffs:
        return 0.0, 0.0
    rng = random.Random(0)
    means = sorted(
        sum(rng.choices(diffs, k=len(diffs))) / len(diffs) for _ in range(BOOTSTRAP_N)
    )
    return means[int(0.025 * BOOTSTRAP_N)], means[int(0.975 * BOOTSTRAP_N)]


def compare(runs: list[dict], baseline: str, variant: str, target_counter: str | None) -> None:
    base = {(r["model"], r["task"], r["rep"]): r for r in runs if r["variant"] == baseline}
    var = {(r["model"], r["task"], r["rep"]): r for r in runs if r["variant"] == variant}
    keys = sorted(set(base) & set(var))
    if not keys:
        sys.exit("no paired runs between the two variants")

    models = sorted({k[0] for k in keys})
    print(f"## {variant} vs {baseline} ({len(keys)} paired runs)\n")

    all_pass = True
    for model in models:
        mkeys = [k for k in keys if k[0] == model]
        sdiffs = [var[k]["ok"] - base[k]["ok"] for k in mkeys]
        s_mean = 100 * sum(sdiffs) / len(sdiffs)
        s_lo, s_hi = bootstrap_ci(sdiffs)

        both_ok = [k for k in mkeys if base[k]["ok"] and var[k]["ok"]]
        if both_ok:
            tdiffs = [
                (var[k]["tokens"] - base[k]["tokens"]) / max(base[k]["tokens"], 1)
                for k in both_ok
            ]
            t_mean = sum(tdiffs) / len(tdiffs)
        else:
            t_mean = 0.0

        c_rel = None
        if target_counter:
            b_rate = sum(base[k]["counters"].get(target_counter, 0) for k in mkeys) / len(mkeys)
            v_rate = sum(var[k]["counters"].get(target_counter, 0) for k in mkeys) / len(mkeys)
            c_rel = (v_rate - b_rate) / b_rate if b_rate > 0 else 0.0

        gate_success = s_mean >= GATE_SUCCESS_DROP_PP
        gate_value = t_mean <= GATE_TOKEN_CUT or (c_rel is not None and c_rel <= GATE_COUNTER_CUT)
        ok = gate_success and gate_value
        all_pass = all_pass and ok

        print(f"### {model}")
        print(f"- success: {s_mean:+.1f}pp (95% CI [{100 * s_lo:+.1f}, {100 * s_hi:+.1f}]) "
              f"-> gate({GATE_SUCCESS_DROP_PP:+.0f}pp): {'PASS' if gate_success else 'FAIL'}")
        print(f"- tokens on joint successes: {100 * t_mean:+.1f}%")
        if c_rel is not None:
            print(f"- {target_counter}: {100 * c_rel:+.1f}%")
        print(f"- value gate (tokens<={100 * GATE_TOKEN_CUT:.0f}% or counter<={100 * GATE_COUNTER_CUT:.0f}%): "
              f"{'PASS' if gate_value else 'FAIL'}")
        print(f"- verdict: {'ACCEPT' if ok else 'REJECT'}\n")

    print(f"## Overall: {'ACCEPT' if all_pass else 'REJECT'} "
          f"(must pass on every model)")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=str(DB_PATH))
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("scorecard")
    tp = sub.add_parser("tasks")
    tp.add_argument("--variant")
    fp = sub.add_parser("failures")
    fp.add_argument("--variant")
    fp.add_argument("--model")
    fp.add_argument("--task")
    fp.add_argument("--limit", type=int, default=20)
    cp = sub.add_parser("compare")
    cp.add_argument("--baseline", required=True)
    cp.add_argument("--variant", required=True)
    cp.add_argument("--target-counter", help="Counter this variant is meant to reduce")
    args = ap.parse_args()

    runs = load(Path(args.db))
    if not runs:
        sys.exit("no runs in database")

    if args.cmd == "scorecard":
        scorecard(runs)
    elif args.cmd == "tasks":
        by_task(runs, args.variant)
    elif args.cmd == "failures":
        failures(runs, args.variant, args.model, args.task, args.limit)
    else:
        compare(runs, args.baseline, args.variant, args.target_counter)


if __name__ == "__main__":
    main()
