#!/usr/bin/env python3
"""Compress transcripts and repack them against a corpus-trained zstd dictionary.

Streaming per-run compression (runner.py) can't share a dictionary because the
corpus doesn't exist yet mid-run. Transcripts are highly redundant *across*
files (same system prompt, same tool schemas in every one), so a dictionary
trained over the whole corpus compresses far better than per-file zstd.

  compact.py migrate     # gzip/plain .jsonl -> .jsonl.zst, drop originals
  compact.py repack      # train a dict over all transcripts, recompress with it
  compact.py stats       # show on-disk footprint

The dictionary is stored at results/transcripts/_dict.zstd. mine.py reads plain
zstd frames without it; repacked frames embed the dict id, so keep the file.
"""

from __future__ import annotations

import argparse
import sys
from compression import zstd
from pathlib import Path

TRANSCRIPTS = Path(__file__).resolve().parent / "results" / "transcripts"
DICT_PATH = TRANSCRIPTS / "_dict.zstd"
DICT_SIZE = 64 * 1024


def _read_any(path: Path) -> bytes:
    if path.suffix == ".zst":
        return zstd.open(path, "rb").read()
    import gzip
    if path.suffix == ".gz":
        return gzip.open(path, "rb").read()
    return path.read_bytes()


def migrate() -> None:
    moved = 0
    for path in list(TRANSCRIPTS.glob("*.jsonl")) + list(TRANSCRIPTS.glob("*.jsonl.gz")):
        raw = _read_any(path)
        base = path.name.split(".jsonl")[0]
        out = TRANSCRIPTS / f"{base}.jsonl.zst"
        with zstd.open(out, "wb") as f:
            f.write(raw)
        path.unlink()
        moved += 1
    print(f"migrated {moved} transcripts to .jsonl.zst", file=sys.stderr)


def repack() -> None:
    try:
        from compression.zstd import train_dict  # type: ignore
    except ImportError:
        sys.exit("this Python's compression.zstd has no train_dict; "
                 "skip repack, per-file .zst is already fine")

    samples = [_read_any(p) for p in TRANSCRIPTS.glob("*.jsonl.zst")]
    if len(samples) < 8:
        sys.exit(f"need >=8 transcripts to train a dictionary, have {len(samples)}")
    zdict = train_dict(samples, DICT_SIZE)
    DICT_PATH.write_bytes(zdict.dict_content if hasattr(zdict, "dict_content") else zdict)

    repacked = 0
    for p in TRANSCRIPTS.glob("*.jsonl.zst"):
        if p == DICT_PATH:
            continue
        raw = _read_any(p)
        with zstd.open(p, "wb", zstd_dict=zdict) as f:
            f.write(raw)
        repacked += 1
    print(f"trained {DICT_SIZE // 1024}KB dict, repacked {repacked} transcripts", file=sys.stderr)


def stats() -> None:
    files = [p for p in TRANSCRIPTS.glob("*.jsonl*") if p != DICT_PATH]
    total = sum(p.stat().st_size for p in files)
    print(f"{len(files)} transcripts, {total / 1024:.0f}KB on disk "
          f"({total / max(len(files), 1):.0f}b avg)", file=sys.stderr)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cmd", choices=("migrate", "repack", "stats"))
    args = ap.parse_args()
    {"migrate": migrate, "repack": repack, "stats": stats}[args.cmd]()


if __name__ == "__main__":
    main()
