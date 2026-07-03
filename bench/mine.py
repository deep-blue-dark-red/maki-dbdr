#!/usr/bin/env python3
"""Mine maki stream-json transcripts into harness failure-mode counters.

Usage:
  mine.py transcript.jsonl            # print counters as JSON
  mine.py --db results/bench.sqlite   # re-mine counters for every stored run
"""

from __future__ import annotations

import argparse
import gzip
import json
import re
import sqlite3
import sys
from collections import defaultdict
from compression import zstd
from pathlib import Path

EDIT_FAIL_PATTERNS = (
    "old_string not found",
    "matches multiple locations",
    "old_string must not be empty",
)
PATH_404_RE = re.compile(r"(?i)no such file|not found|does not exist")
BASH_FILE_OPS_RE = re.compile(r"^\s*(cat|ls|head|tail|find)\b|^\s*grep\b|^\s*sed\s+-n")
BASH_RG_RE = re.compile(r"(?:^|\|)\s*rg\b")
PHANTOM_RES = [
    re.compile(p)
    for p in (
        r"<tool_call>",
        r'"tool_calls"\s*:',
        r'"function_call"',
        r'```json\s*\{[^`]{0,400}"name"\s*:',
        r'\{"tool"\s*:',
    )
]
FULL_READ_LINES = 120
EDIT_RECOVERY_WINDOW = 3


def _norm(name: str) -> str:
    return (name or "").lower().replace("_", "")


def _result_text(block: dict) -> str:
    content = block.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            b.get("text", "")
            for b in content
            if isinstance(b, dict) and b.get("type") == "text"
        )
    return ""


def resolve_transcript(path: Path) -> Path:
    """Find a transcript that may have been compressed after its path was stored."""
    if path.exists():
        return path
    for suffix in (".zst", ".gz"):
        cand = path.with_name(path.name + suffix)
        if cand.exists():
            return cand
    return path


def _zstd_dict(path: Path):
    """Load the corpus dictionary if compact.py repack created one alongside."""
    dict_path = path.parent / "_dict.zstd"
    if not dict_path.exists():
        return None
    try:
        return zstd.ZstdDict(dict_path.read_bytes())
    except Exception:
        return None


def _open_text(path: Path):
    if path.suffix == ".zst":
        zdict = _zstd_dict(path)
        kw = {"zstd_dict": zdict} if zdict is not None else {}
        return zstd.open(path, "rt", encoding="utf-8", errors="replace", **kw)
    if path.suffix == ".gz":
        return gzip.open(path, "rt", encoding="utf-8", errors="replace")
    return open(path, encoding="utf-8", errors="replace")


def _events(path: Path):
    with _open_text(resolve_transcript(path)) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                continue


def mine_transcript(path: Path) -> dict[str, float]:
    calls: list[dict] = []
    by_id: dict[str, dict] = {}
    assistant_events: list[tuple[bool, bool, list[str]]] = []

    for msg in _events(path):
        mtype = msg.get("type")
        if mtype == "assistant":
            content = msg.get("message", {}).get("content") or []
            if not isinstance(content, list):
                continue
            has_tool = False
            texts: list[str] = []
            for b in content:
                if not isinstance(b, dict):
                    continue
                if b.get("type") == "tool_use":
                    has_tool = True
                    entry = {
                        "id": b.get("id"),
                        "name": _norm(b.get("name")),
                        "input": b.get("input") or {},
                        "result": "",
                        "is_error": False,
                    }
                    calls.append(entry)
                    if entry["id"]:
                        by_id[entry["id"]] = entry
                elif b.get("type") == "text" and b.get("text"):
                    texts.append(b["text"])
            assistant_events.append((bool(texts), has_tool, texts))
        elif mtype == "user":
            content = msg.get("message", {}).get("content")
            if not isinstance(content, list):
                continue
            for b in content:
                if isinstance(b, dict) and b.get("type") == "tool_result":
                    entry = by_id.get(b.get("tool_use_id"))
                    if entry is not None:
                        entry["result"] = _result_text(b)
                        entry["is_error"] = bool(b.get("is_error"))

    n: dict[str, float] = defaultdict(float)
    seen_inputs: set[tuple[str, str]] = set()
    failed_edit_paths: set[str] = set()
    last_success_edit: dict[str, int] = {}
    indexed_paths: set[str] = set()
    pending_fails: list[tuple[str, int]] = []

    for i, e in enumerate(calls):
        name, inp, res = e["name"], e["input"], e["result"] or ""
        n["tool_calls_total"] += 1
        n[f"tool.{name}"] += 1

        if name == "batch":
            for sub in inp.get("tool_calls") or []:
                if isinstance(sub, dict):
                    subname = _norm(sub.get("tool") or sub.get("name") or "")
                    if subname:
                        n["tool_calls_total"] += 1
                        n[f"tool.{subname}"] += 1

        key = (name, json.dumps(inp, sort_keys=True, default=str))
        if key in seen_inputs:
            n["identical_repeat"] += 1
        else:
            seen_inputs.add(key)

        path = inp.get("path") or inp.get("file_path") or ""
        err = e["is_error"] or res.startswith("error")

        if name in ("edit", "multiedit"):
            fail = err or any(p in res for p in EDIT_FAIL_PATTERNS)
            if fail:
                n["edit_fail"] += 1
                if path:
                    failed_edit_paths.add(path)
                    pending_fails.append((path, i))
                    last_success_edit.pop(path, None)
            elif path:
                last_success_edit[path] = i
                if any(p == path and i - j <= EDIT_RECOVERY_WINDOW for p, j in pending_fails):
                    n["edit_recovered"] += 1
                pending_fails = [
                    (p, j) for p, j in pending_fails
                    if not (p == path and i - j <= EDIT_RECOVERY_WINDOW)
                ]

        if name == "write" and path in failed_edit_paths:
            n["write_fallback"] += 1

        if name == "read":
            if path in last_success_edit:
                n["reread_after_edit"] += 1
            windowed = inp.get("offset") is not None or inp.get("limit") is not None
            if not windowed and res.count("\n") + 1 > FULL_READ_LINES and path not in indexed_paths:
                n["full_read_no_index"] += 1

        if name == "index" and path:
            indexed_paths.add(path)

        if name == "bash":
            cmd = inp.get("command", "")
            if BASH_RG_RE.search(cmd):
                n["bash_rg"] += 1
            elif BASH_FILE_OPS_RE.search(cmd):
                n["bash_file_ops"] += 1

        if name in ("read", "index", "edit", "multiedit") and err and PATH_404_RE.search(res):
            n["path_404"] += 1

        if name == "todowrite":
            n["todo_writes"] += 1

        if name == "skill":
            n["skill_loads"] += 1
            sk = (inp.get("name") or "").strip()
            if sk:
                n[f"skill.{sk}"] += 1

    n["turns_assistant"] = float(len(assistant_events))
    for k, (has_text, has_tool, texts) in enumerate(assistant_events):
        if has_text and not has_tool and k < len(assistant_events) - 1:
            n["narration_turn"] += 1
        if any(rx.search(t) for t in texts for rx in PHANTOM_RES):
            n["phantom_tool_call"] += 1

    n["todo_used"] = 1.0 if n.get("todo_writes", 0) >= 1 else 0.0
    return dict(n)


def remine_db(db_path: Path) -> None:
    conn = sqlite3.connect(db_path)
    rows = conn.execute("SELECT run_id, transcript FROM runs").fetchall()
    updated = 0
    for run_id, transcript in rows:
        tp = resolve_transcript(Path(transcript))
        if not tp.exists():
            print(f"skip {run_id}: transcript missing", file=sys.stderr)
            continue
        counters = mine_transcript(tp)
        conn.execute("DELETE FROM counters WHERE run_id = ?", (run_id,))
        conn.executemany(
            "INSERT INTO counters (run_id, name, value) VALUES (?, ?, ?)",
            [(run_id, k, v) for k, v in counters.items()],
        )
        updated += 1
    conn.commit()
    conn.close()
    print(f"re-mined {updated}/{len(rows)} runs", file=sys.stderr)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("transcript", nargs="?", help="Path to a stream-json transcript")
    ap.add_argument("--db", help="Re-mine all runs in this sqlite database")
    args = ap.parse_args()

    if args.db:
        remine_db(Path(args.db))
    elif args.transcript:
        print(json.dumps(mine_transcript(Path(args.transcript)), indent=2, sort_keys=True))
    else:
        ap.error("give a transcript path or --db")


if __name__ == "__main__":
    main()
