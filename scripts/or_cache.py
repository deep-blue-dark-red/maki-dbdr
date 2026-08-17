#!/usr/bin/env python3
"""Diagnose OpenRouter prompt-cache misses from maki's turn_stats.jsonl logs.

OpenRouter load balances across every upstream serving a model unless the
request pins one, and each upstream keeps its own prompt cache. The symptom is
a session whose `cache_read` collapses to zero, or to the size of a request
several turns old, even though the prompt prefix never changed.

Two modes, both run by default:

  staleness  Offline. A prefix cache only ever grows, so a `cache_read` below
             the previous request's total means a *different* cache served it.
             Matching each `cache_read` against earlier request totals dates
             the cache that answered, which is what distinguishes a routing
             bounce from a genuinely changed prompt prefix. Works on any log.

  upstream   Needs OPENROUTER_API_KEY and logs written by a maki that records
             `upstream.generation_id` (added alongside the routing config).
             Resolves each turn to the upstream that actually served it via
             /api/v1/generation, confirming the staleness inference directly.
"""

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from collections import Counter
from pathlib import Path

GENERATION_URL = "https://openrouter.ai/api/v1/generation?id={}"
# Prefix caches are quantized (128-token blocks here), so an exact match to an
# earlier total is never byte-exact. One block of slack keeps the attribution
# honest without matching unrelated sizes.
BLOCK = 128


def load(path):
    turns = []
    for n, line in enumerate(path.open(), 1):
        line = line.strip()
        if not line:
            continue
        try:
            turns.append(json.loads(line))
        except json.JSONDecodeError as e:
            print(f"warn: {path}:{n}: {e}", file=sys.stderr)
    return turns


def sessions(root):
    """Every session log under `root`, or `root` itself if it is one."""
    if root.is_file():
        return [root]
    return sorted(root.glob("*/turn_stats.jsonl"))


def analyze(turns):
    """Attribute each turn's cache_read to the request whose prefix it matches."""
    totals = [t["input"] + t["cache_read"] for t in turns]
    rows = []
    for i, t in enumerate(turns):
        cr = t["cache_read"]
        lag = match = None
        if cr == 0:
            kind = "cold"
        else:
            # Nearest earlier request whose total equals this cache_read.
            best = min(
                ((abs(cr - totals[j]), j) for j in range(i)),
                default=(None, None),
            )
            if best[0] is not None and best[0] <= BLOCK:
                match, lag = best[1], i - best[1]
                kind = "fresh" if lag <= 1 else "stale"
            else:
                kind = "partial"
        rows.append(
            {
                "event": t.get("event_id", i),
                "total": totals[i],
                "cache_read": cr,
                "input": t["input"],
                "kind": kind,
                "lag": lag,
                "match": match,
                "upstream": (t.get("upstream") or {}).get("name"),
                "generation_id": (t.get("upstream") or {}).get("generation_id"),
                "cost": t.get("cost") or 0.0,
            }
        )
    return rows


def recoverable(rows):
    """Tokens billed as fresh input that an unbounced cache would have served."""
    waste = 0
    for i, r in enumerate(rows[1:], 1):
        expected = min(rows[i - 1]["total"], r["total"])
        waste += max(0, expected - r["cache_read"])
    return waste


def fetch_upstreams(rows, key):
    """Resolve generation ids to upstream names. Fills `upstream` in place."""
    todo = [r for r in rows if r["generation_id"] and not r["upstream"]]
    if not todo:
        return 0
    filled = 0
    for r in todo:
        req = urllib.request.Request(
            GENERATION_URL.format(r["generation_id"]),
            headers={"Authorization": f"Bearer {key}"},
        )
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                data = json.load(resp)["data"]
        except (urllib.error.HTTPError, urllib.error.URLError, KeyError) as e:
            # 404 is normal for the first few seconds after a request.
            print(f"warn: generation {r['generation_id']}: {e}", file=sys.stderr)
            continue
        r["upstream"] = data.get("provider_name")
        filled += 1
    return filled


def report(path, rows, verbose):
    kinds = Counter(r["kind"] for r in rows)
    billed = sum(r["input"] for r in rows)
    waste = recoverable(rows)
    upstreams = Counter(r["upstream"] for r in rows if r["upstream"])

    print(f"\n{path}")
    print(f"  requests        {len(rows)}")
    print(
        "  cache           "
        f"{kinds['fresh']} fresh, {kinds['stale']} stale, "
        f"{kinds['partial']} partial, {kinds['cold']} cold"
    )
    print(f"  billed input    {billed:,} tok")
    if billed:
        print(f"  recoverable     {waste:,} tok ({waste / billed:.0%} of billed input)")
    print(f"  cost            ${sum(r['cost'] for r in rows):.4f}")
    if upstreams:
        served = ", ".join(f"{n} x{c}" for n, c in upstreams.most_common())
        print(f"  upstreams       {served}")
        if len(upstreams) > 1:
            print(
                "  -> routing is unpinned; set provider_order in providers.toml "
                "under [openrouter]"
            )
    elif kinds["stale"] or kinds["cold"] > 1:
        print(
            "  -> caches of differing ages served this session, the signature of "
            "unpinned routing (run a new session to capture upstream names)"
        )

    if not verbose:
        return
    print(f"\n  {'ev':>4} {'total':>8} {'cached':>8} {'kind':<8} {'served by':<24}")
    for r in rows:
        if r["kind"] == "stale":
            served = f"cache from ev{r['match']} ({r['lag']} turns old)"
        elif r["kind"] == "cold":
            served = "nothing cached"
        elif r["kind"] == "partial":
            served = "partial prefix"
        else:
            served = "previous turn"
        if r["upstream"]:
            served = f"{r['upstream']}: {served}"
        print(
            f"  {r['event']:>4} {r['total']:>8,} {r['cache_read']:>8,} "
            f"{r['kind']:<8} {served:<24}"
        )


def main():
    default_root = Path.home() / ".local/logs/maki"
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument(
        "path",
        nargs="?",
        type=Path,
        default=default_root,
        help=f"session dir, log root, or a turn_stats.jsonl (default: {default_root})",
    )
    p.add_argument("-v", "--verbose", action="store_true", help="per-request detail")
    p.add_argument(
        "--no-fetch",
        action="store_true",
        help="skip the /api/v1/generation lookups even if a key is set",
    )
    args = p.parse_args()

    if not args.path.exists():
        sys.exit(f"error: {args.path} does not exist")

    logs = sessions(args.path)
    if not logs:
        sys.exit(f"error: no turn_stats.jsonl under {args.path}")

    key = None if args.no_fetch else os.environ.get("OPENROUTER_API_KEY")
    for log in logs:
        turns = load(log)
        if not turns:
            continue
        rows = analyze(turns)
        if key:
            fetch_upstreams(rows, key)
        report(log, rows, args.verbose)


if __name__ == "__main__":
    main()
