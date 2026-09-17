#!/usr/bin/env python3
"""Matrix executor: (variant x model x task x rep) -> sqlite + transcripts.

Each run gets an isolated config dir (XDG_CONFIG_HOME override) built from the
live ~/.config/maki plus the variant overlay, and a fresh git-inited copy of
the task fixture as its working directory. See DESIGN.md.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import threading
import time
import tomllib
from compression import zstd
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

import mine

BENCH = Path(__file__).resolve().parent
RESULTS = BENCH / "results"
TRANSCRIPTS = RESULTS / "transcripts"
RUNS = RESULTS / "runs"
TMP = RESULTS / "tmp"
DB_PATH = RESULTS / "bench.sqlite"
CHECK_TIMEOUT_S = 120

SCHEMA = """
CREATE TABLE IF NOT EXISTS runs (
  run_id TEXT PRIMARY KEY,
  ts TEXT,
  variant TEXT,
  variant_hash TEXT,
  model TEXT,
  task TEXT,
  category TEXT,
  rep INTEGER,
  status TEXT,
  check_rc INTEGER,
  check_output TEXT,
  num_turns INTEGER,
  duration_ms INTEGER,
  cost_usd REAL,
  input_tokens INTEGER,
  output_tokens INTEGER,
  cache_read_tokens INTEGER,
  cache_write_tokens INTEGER,
  maki_version TEXT,
  transcript TEXT
);
CREATE TABLE IF NOT EXISTS counters (
  run_id TEXT,
  name TEXT,
  value REAL,
  PRIMARY KEY (run_id, name)
);
"""


def load_cfg() -> dict:
    with open(BENCH / "bench.toml", "rb") as f:
        return tomllib.load(f)


def real_config_dir() -> Path:
    xdg = os.environ.get("XDG_CONFIG_HOME")
    base = Path(xdg) if xdg else Path.home() / ".config"
    return base / "maki"


def discover_tasks(only: list[str] | None) -> list[dict]:
    tasks = []
    for toml_path in sorted((BENCH / "tasks").glob("*/task.toml")):
        with open(toml_path, "rb") as f:
            spec = tomllib.load(f)["task"]
        spec["dir"] = toml_path.parent
        if only and spec["id"] not in only:
            continue
        tasks.append(spec)
    if only:
        found = {t["id"] for t in tasks}
        missing = set(only) - found
        if missing:
            sys.exit(f"unknown tasks: {', '.join(sorted(missing))}")
    return tasks


def discover_variants(only: list[str] | None) -> list[Path]:
    dirs = sorted(d for d in (BENCH / "variants").iterdir() if d.is_dir())
    if only:
        by_name = {d.name: d for d in dirs}
        missing = set(only) - set(by_name)
        if missing:
            sys.exit(f"unknown variants: {', '.join(sorted(missing))}")
        return [by_name[n] for n in only]
    return dirs


def variant_hash(d: Path) -> str:
    h = hashlib.sha256()
    for p in sorted(d.rglob("*")):
        if p.is_file():
            h.update(str(p.relative_to(d)).encode())
            h.update(p.read_bytes())
    return h.hexdigest()[:12]


def model_slug(model: str) -> str:
    return model.rsplit("/", 1)[-1].replace(".", "_")


def _git(work: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-c", "user.email=bench@local", "-c", "user.name=bench",
         "-c", "commit.gpgsign=false", *args],
        cwd=work, check=True, capture_output=True,
    )


class Budget:
    def __init__(self, max_total: float):
        self.max_total = max_total
        self.spent = 0.0
        self.lock = threading.Lock()
        self.stop = threading.Event()

    def add(self, cost: float) -> None:
        with self.lock:
            self.spent += cost
            if self.max_total > 0 and self.spent > self.max_total:
                self.stop.set()


class Db:
    def __init__(self, path: Path):
        self.conn = sqlite3.connect(path, check_same_thread=False)
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.executescript(SCHEMA)
        self.lock = threading.Lock()

    def insert(self, row: dict, counters: dict[str, float]) -> None:
        with self.lock:
            self.conn.execute(
                f"INSERT OR REPLACE INTO runs ({','.join(row)}) "
                f"VALUES ({','.join('?' * len(row))})",
                list(row.values()),
            )
            self.conn.execute("DELETE FROM counters WHERE run_id = ?", (row["run_id"],))
            self.conn.executemany(
                "INSERT INTO counters (run_id, name, value) VALUES (?, ?, ?)",
                [(row["run_id"], k, v) for k, v in counters.items()],
            )
            self.conn.commit()


def run_cell(
    cfg: dict,
    db: Db,
    budget: Budget,
    variant: Path,
    vhash: str,
    model: str,
    task: dict,
    rep: int,
    maki_version: str,
) -> tuple[str, str]:
    if budget.stop.is_set():
        return "skipped", f"{task['id']} r{rep}: skipped (budget exceeded)"

    run_cfg = cfg.get("run", {})
    maki_bin = run_cfg.get("maki_bin", "maki")
    max_turns = task.get("max_turns", run_cfg.get("default_max_turns", 20))
    timeout_s = task.get("timeout_s", run_cfg.get("default_timeout_s", 300))

    stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    run_id = f"{variant.name}--{model_slug(model)}--{task['id']}--r{rep}--{stamp}"
    tmp_run = TMP / run_id
    config_root = tmp_run / "config"
    work = tmp_run / "work"
    transcript_path = TRANSCRIPTS / f"{run_id}.jsonl.zst"
    artifacts = RUNS / run_id
    artifacts.mkdir(parents=True, exist_ok=True)

    shutil.copytree(real_config_dir(), config_root / "maki")
    shutil.copytree(variant, config_root / "maki", dirs_exist_ok=True)
    shutil.copytree(task["dir"] / "fixture", work)
    _git(work, "init", "-q")
    _git(work, "add", "-A")
    _git(work, "commit", "-qm", "fixture")

    env = os.environ.copy()
    env["XDG_CONFIG_HOME"] = str(config_root)

    cmd = [
        maki_bin, "-p", "--yolo", "--verbose",
        "--output-format", "stream-json",
        "--max-turns", str(max_turns),
        "-m", model,
        task["prompt"],
    ]

    result_event = None
    killed = threading.Event()
    start = time.monotonic()

    stderr_path = artifacts / "stderr.log"
    with open(stderr_path, "wb") as stderr_f:
        proc = subprocess.Popen(
            cmd, cwd=work, env=env,
            stdout=subprocess.PIPE, stderr=stderr_f,
            start_new_session=True,
        )

        def _kill() -> None:
            killed.set()
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

        timer = threading.Timer(timeout_s, _kill)
        timer.start()
        try:
            with zstd.open(transcript_path, "wt") as tf:
                assert proc.stdout is not None
                for raw in proc.stdout:
                    line = raw.decode("utf-8", "replace")
                    tf.write(line)
                    try:
                        msg = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    if msg.get("type") == "result":
                        result_event = msg
            proc.wait()
        finally:
            timer.cancel()

    wall_ms = int((time.monotonic() - start) * 1000)

    bench_dir = work / ".bench"
    bench_dir.mkdir(exist_ok=True)
    result_file = bench_dir / "result.txt"
    result_file.write_text((result_event or {}).get("result", "") or "")
    shutil.copy(result_file, artifacts / "result.txt")

    check_rc, check_output = -1, ""
    if result_event is not None or killed.is_set():
        try:
            check = subprocess.run(
                ["bash", str(task["dir"] / "check.sh")],
                cwd=work,
                env={**env, "WORKDIR": str(work), "RESULT_FILE": str(result_file)},
                capture_output=True, text=True, timeout=CHECK_TIMEOUT_S,
            )
            check_rc = check.returncode
            check_output = (check.stdout + check.stderr)[-2000:]
        except subprocess.TimeoutExpired:
            check_rc, check_output = -2, "check.sh timed out"
    (artifacts / "check_output.txt").write_text(check_output)

    if killed.is_set():
        status = "timeout"
    elif result_event is None:
        status = "infra_error"
        check_output = stderr_path.read_text(errors="replace")[-2000:]
    elif check_rc == 0:
        status = "pass"
    else:
        status = "fail"

    usage = (result_event or {}).get("usage", {})
    cost = float((result_event or {}).get("total_cost_usd") or 0.0)
    budget.add(cost)

    counters = mine_transcript_safe(transcript_path)

    row = {
            "run_id": run_id,
            "ts": datetime.now(timezone.utc).isoformat(),
            "variant": variant.name,
            "variant_hash": vhash,
            "model": model,
            "task": task["id"],
            "category": task.get("category", ""),
            "rep": rep,
            "status": status,
            "check_rc": check_rc,
            "check_output": check_output,
            "num_turns": (result_event or {}).get("num_turns", 0),
            "duration_ms": (result_event or {}).get("duration_ms", wall_ms),
            "cost_usd": cost,
            "input_tokens": usage.get("input_tokens", 0),
            "output_tokens": usage.get("output_tokens", 0),
            "cache_read_tokens": usage.get("cache_read_input_tokens", 0),
            "cache_write_tokens": usage.get("cache_creation_input_tokens", 0),
            "maki_version": maki_version,
            "transcript": str(transcript_path),
    }
    db.insert(row, counters)
    (artifacts / "meta.json").write_text(json.dumps(
        {**row, "cmd": cmd, "variant_dir": str(variant), "counters": counters}, indent=2
    ))

    keep = cfg.get("run", {}).get("keep_failed_workdirs", True) and status != "pass"
    if not keep:
        shutil.rmtree(tmp_run, ignore_errors=True)

    turns = (result_event or {}).get("num_turns", "?")
    return status, f"{run_id}: {status} (turns={turns}, ${cost:.4f})"


def mine_transcript_safe(path: Path) -> dict[str, float]:
    try:
        return mine.mine_transcript(path)
    except Exception as e:  # miner bugs must never lose a run row
        print(f"miner error on {path.name}: {e}", file=sys.stderr)
        return {"miner_error": 1.0}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--variants", nargs="*", help="Variant names (default: bench.toml)")
    ap.add_argument("--models", nargs="*", help="Model specs (default: bench.toml)")
    ap.add_argument("--tasks", nargs="*", help="Task ids (default: all)")
    ap.add_argument("--reps", type=int, help="Repetitions per cell (default: bench.toml)")
    ap.add_argument("--concurrency", type=int, help="Parallel runs (default: bench.toml)")
    ap.add_argument("--dry-run", action="store_true", help="Print the matrix and exit")
    ap.add_argument("--resume", action="store_true",
                    help="Skip cells already recorded as pass/fail/timeout")
    args = ap.parse_args()

    if (Path.home() / ".maki").exists():
        sys.exit("~/.maki exists: maki's config fallback would bypass XDG_CONFIG_HOME. Aborting.")

    cfg = load_cfg()
    matrix_cfg = cfg.get("matrix", {})
    models = args.models or matrix_cfg.get("models", [])
    reps = args.reps or matrix_cfg.get("reps", 5)
    concurrency = args.concurrency or matrix_cfg.get("concurrency", 3)
    variants = discover_variants(args.variants or matrix_cfg.get("variants"))
    tasks = discover_tasks(args.tasks)

    if not models or not variants or not tasks:
        sys.exit("empty matrix: need at least one model, variant, and task")

    cells = [
        (variant, model, task, rep)
        for variant in variants
        for model in models
        for task in tasks
        for rep in range(1, reps + 1)
    ]

    if args.resume and DB_PATH.exists():
        conn = sqlite3.connect(DB_PATH)
        done_keys = set(conn.execute(
            "SELECT variant, model, task, rep FROM runs "
            "WHERE status IN ('pass','fail','timeout')"
        ))
        conn.close()
        before = len(cells)
        cells = [
            c for c in cells
            if (c[0].name, c[1], c[2]["id"], c[3]) not in done_keys
        ]
        print(f"resume: skipping {before - len(cells)} completed cells", file=sys.stderr)

    print(
        f"matrix: {len(variants)} variant(s) x {len(models)} model(s) x "
        f"{len(tasks)} task(s) x {reps} rep(s) = {len(cells)} runs",
        file=sys.stderr,
    )
    if args.dry_run:
        for variant, model, task, rep in cells:
            print(f"  {variant.name} | {model} | {task['id']} | rep {rep}")
        return

    for d in (RESULTS, TRANSCRIPTS, RUNS, TMP):
        d.mkdir(parents=True, exist_ok=True)

    maki_bin = cfg.get("run", {}).get("maki_bin", "maki")
    maki_version = subprocess.run(
        [maki_bin, "-V"], capture_output=True, text=True
    ).stdout.strip()

    db = Db(DB_PATH)
    budget = Budget(cfg.get("budget", {}).get("max_usd_total", 0.0))
    vhashes = {v.name: variant_hash(v) for v in variants}

    done = 0
    tally: dict[str, int] = {}
    with ThreadPoolExecutor(max_workers=concurrency) as pool:
        futures = {
            pool.submit(
                run_cell, cfg, db, budget,
                variant, vhashes[variant.name], model, task, rep, maki_version,
            ): (variant.name, model, task["id"], rep)
            for variant, model, task, rep in cells
        }

        def collect(fut) -> None:
            nonlocal done
            done += 1
            cell = futures[fut]
            try:
                status, line = fut.result()
                tally[status] = tally.get(status, 0) + 1
                passes = tally.get("pass", 0)
                graded = sum(tally.get(s, 0) for s in ("pass", "fail", "timeout"))
                rate = f" | pass {passes}/{graded}" if graded else ""
                print(f"[{done}/{len(futures)}{rate}] {line}", file=sys.stderr)
            except Exception as e:
                tally["runner_error"] = tally.get("runner_error", 0) + 1
                print(f"[{done}/{len(futures)}] {cell}: RUNNER ERROR {e}", file=sys.stderr)

        try:
            budget_warned = False
            for fut in as_completed(futures):
                collect(fut)
                if budget.stop.is_set() and not budget_warned:
                    budget_warned = True
                    print(
                        f"budget exceeded (${budget.spent:.2f}): "
                        "queued runs will be skipped, in-flight runs finish",
                        file=sys.stderr,
                    )
        except KeyboardInterrupt:
            cancelled = sum(1 for f in futures if f.cancel())
            print(
                f"\ninterrupted: cancelled {cancelled} queued runs, "
                "draining in-flight (use --resume to continue later)",
                file=sys.stderr,
            )
            for fut in as_completed([f for f in futures if not f.cancelled()]):
                collect(fut)

    summary = ", ".join(f"{k}={v}" for k, v in sorted(tally.items()))
    print(f"done: {done} runs ({summary}), total spend ${budget.spent:.2f}", file=sys.stderr)


if __name__ == "__main__":
    main()
