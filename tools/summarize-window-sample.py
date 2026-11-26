"""Summarize sampled window-copy metrics; medians are per log batch, not per frame."""
import argparse
import datetime as dt
import json
import re
import statistics
from pathlib import Path

PATTERN = re.compile(r"^(\S+).*?title memory windows with no card side took per frame: ([\d.]+) MB copied, ([\d.]+) MB left out; of what they took, ([\d.]+) MB held the bytes .*?, ([\d.]+) MB other bytes, ([\d.]+) MB nothing to compare with;.*?VICTORY_WINDOW_SKIP_SAME=sample")

if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("run_dir", type=Path)
    p.add_argument("start", type=float)
    p.add_argument("end", type=float)
    a = p.parse_args()
    start_time = dt.datetime.fromisoformat(json.loads((a.run_dir / "launch.json").read_text(encoding="utf-8-sig"))["started"])
    rows = []
    with (a.run_dir / "stderr.log").open(encoding="utf-8", errors="replace") as source:
        for line in source:
            match = PATTERN.search(line)
            if match:
                elapsed = (dt.datetime.fromisoformat(match[1].replace("Z", "+00:00")) - start_time).total_seconds()
                if a.start + 5 <= elapsed <= a.end - 5:
                    rows.append(tuple(float(x) for x in match.groups()[1:]))
    names = ("copied", "left_out", "same_estimate", "different_estimate", "unknown_estimate")
    print(json.dumps({"window_seconds": [a.start, a.end], "margin_seconds": 5, "log_batches": len(rows), "median_mb_per_frame_by_batch": {name: statistics.median(r[i] for r in rows) for i, name in enumerate(names)} if rows else {}, "caveat": "Fixed one-in-16 address sample, extrapolated; medians by log batch, not weighted by frame count. Batches can straddle window boundary; no FPS benefit established."}, indent=2))
