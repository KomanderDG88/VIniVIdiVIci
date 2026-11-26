"""Summarize top-five CPU telemetry over a fully enclosed elapsed-time window.

Values are lower bounds: a process absent from the top five can still consume CPU.
The process identities/names remain in the private run directory.
"""
import argparse
import json
from collections import defaultdict
from pathlib import Path


def summarize(path, start, end):
    totals = defaultdict(lambda: [0.0, 0])
    intervals = 0
    seconds = 0.0
    for line in Path(path).read_text(encoding="utf-8-sig").splitlines():
        row = json.loads(line)
        finish = float(row["seconds"])
        duration = float(row["interval_seconds"])
        if duration <= 0 or finish - duration < start or finish > end:
            continue
        intervals += 1
        seconds += duration
        for proc in row["top"]:
            entry = totals[proc["name"]]
            entry[0] += proc["logical_cores"] * duration
            entry[1] += 1
    return {
        "window_seconds": [start, end], "samples": intervals,
        "sampled_seconds": round(seconds, 2),
        "caveat": "Top-five censored; omitted processes may consume CPU, values are lower bounds; names aggregate processes.",
        "processes": sorted([
            {"name": name, "mean_logical_cores_lower_bound": round(value / seconds, 3), "top_five_appearances": count}
            for name, (value, count) in totals.items()
        ], key=lambda row: row["mean_logical_cores_lower_bound"], reverse=True) if seconds else [],
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path")
    parser.add_argument("start", type=float)
    parser.add_argument("end", type=float)
    args = parser.parse_args()
    if args.end <= args.start:
        parser.error("end must exceed start")
    print(json.dumps(summarize(args.path, args.start, args.end), indent=2))
