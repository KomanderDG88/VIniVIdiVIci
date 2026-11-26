"""Summarize complete logged frame batches after a known observation time.
Not proof of playability: screenshots and input checks remain separate.
"""
import argparse, datetime as dt, json, re, statistics
from pathlib import Path

PAT = re.compile(r'^(\S+).*frame times: (\d+) frames in ([\d.]+) s:.*each: (.*)$')
def summarize(path, start, minutes):
    buckets = {}
    for line in Path(path).open(encoding='utf-8', errors='replace'):
        m = PAT.search(line)
        if not m:
            continue
        try:
            end = dt.datetime.fromisoformat(m[1].replace('Z', '+00:00'))
            count, seconds = int(m[2]), float(m[3])
            values = [int(x) for x in m[4].split()]
        except ValueError:
            continue
        begin = end - dt.timedelta(seconds=seconds)
        if begin < start or len(values) != count or seconds <= 0:
            continue
        bucket = int((begin - start).total_seconds() // (minutes * 60))
        boundary = start + dt.timedelta(minutes=minutes * (bucket + 1))
        if end > boundary:
            continue
        row = buckets.setdefault(bucket, {'frames': 0, 'logged_seconds': 0.0, 'batches': 0, 'times_ms': []})
        row['frames'] += count
        row['logged_seconds'] += seconds
        row['batches'] += 1
        row['times_ms'].extend(values)
    result = []
    for bucket, row in sorted(buckets.items()):
        vals = sorted(row.pop('times_ms'))
        window_start = start + dt.timedelta(minutes=minutes * bucket)
        window_end = window_start + dt.timedelta(minutes=minutes)
        row.update(start=window_start.isoformat(), end=window_end.isoformat(),
                   complete_window=dt.datetime.now(dt.timezone.utc) >= window_end,
                   logged_seconds_pct_of_window=round(100 * row['logged_seconds'] / (minutes * 60), 1),
                   fps=round(row['frames'] / row['logged_seconds'], 3),
                   median_ms=statistics.median(vals),
                   p95_ms=vals[max(0, int((len(vals)-1)*.95))],
                   logged_seconds=round(row['logged_seconds'], 1))
        result.append(row)
    return result

if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('log'); ap.add_argument('start'); ap.add_argument('--minutes', type=int, default=5)
    a = ap.parse_args()
    start = dt.datetime.fromisoformat(a.start.replace('Z', '+00:00'))
    if start.tzinfo is None or a.minutes < 1:
        ap.error('start needs timezone; minutes must be positive')
    print(json.dumps({'caveat': 'Logged batches only; FPS uses rounded batch durations, may omit unlogged time. Scene and control require independent evidence.', 'intervals': summarize(a.log, start, a.minutes)}, indent=2))
