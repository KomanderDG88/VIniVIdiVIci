"""Summarize logged frame batches after a chosen scripted key-down event.
A key read and comparable scene must be verified independently.
"""
import argparse, datetime as dt, importlib.util, json, re
from pathlib import Path
spec = importlib.util.spec_from_file_location('world_frame_summary', Path(__file__).with_name('world-frame-summary.py'))
mod = importlib.util.module_from_spec(spec); spec.loader.exec_module(mod)
EVENT = re.compile(r'^(\S+).*scripted key down key=(\d+) at=([\d.]+)')
def find_start(log, key, at):
    matches = []
    with Path(log).open(encoding='utf-8', errors='replace') as f:
        for line in f:
            m = EVENT.search(line)
            if m and int(m[2]) == key and abs(float(m[3]) - at) <= 1:
                matches.append(dt.datetime.fromisoformat(m[1].replace('Z', '+00:00')))
    if len(matches) != 1:
        raise ValueError(f'Expected one key-down event for {key} at {at}s; found {len(matches)}')
    return matches[0]
if __name__ == '__main__':
    p = argparse.ArgumentParser(); p.add_argument('log'); p.add_argument('--key', type=int, default=76); p.add_argument('--at', type=float, default=540); p.add_argument('--minutes', type=int, default=1)
    a = p.parse_args()
    if a.minutes < 1: p.error('minutes must be positive')
    start = find_start(a.log, a.key, a.at)
    print(json.dumps({'start': start.isoformat(), 'key': a.key, 'scheduled_seconds': a.at,
        'caveat': 'Key-down alignment does not establish that the title read the key or that A/B scenes match. Logged batches exclude boundary overlaps and durations are rounded.',
        'intervals': mod.summarize(a.log, start, a.minutes)}, indent=2))
