"""Compare two scripted 280-second windows; frames require separately verified scene.

Usage: python compare-movement.py <baseline-stderr.log> <candidate-stderr.log>
Counts only complete frame-log batches wholly within each aligned window.
"""
import datetime as dt
import importlib.util
import json
import re
import statistics
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location('movement', Path(__file__).with_name('movement-frame-summary.py'))
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
KEY = re.compile(r'let go after its hold .*key=(76|87) at=([\d.]+) reads=(\d+)')
LOAD = re.compile(r"processors (\d+(?:\.\d+)?) % busy, all programs; this process kept (\d+(?:\.\d+)?) of (\d+) processors busy")

def measure(path):
    start = mod.find_start(path, 76, 540)
    end_window = start + dt.timedelta(seconds=280)
    values, seconds, key_reads, other_cpu = [], 0.0, {'76': [], '87': []}, []
    latest_observation = None
    with Path(path).open(encoding='utf8', errors='replace') as f:
        for line in f:
            lm = LOAD.search(line)
            if lm:
                try:
                    when = dt.datetime.fromisoformat(line.split()[0].replace('Z', '+00:00'))
                    latest_observation = max(latest_observation, when) if latest_observation else when
                    if start <= when <= end_window:
                        other_cpu.append(max(0.0, float(lm[1]) * int(lm[3]) / 100 - float(lm[2])))
                except (ValueError, IndexError):
                    pass
            km = KEY.search(line)
            if km:
                when = dt.datetime.fromisoformat(line.split()[0].replace('Z', '+00:00'))
                if start <= when <= end_window:
                    key_reads[km[1]].append(int(km[3]))
            m = mod.mod.PAT.search(line)
            if not m:
                continue
            try:
                end = dt.datetime.fromisoformat(m[1].replace('Z', '+00:00'))
                latest_observation = max(latest_observation, end) if latest_observation else end
                count, duration = int(m[2]), float(m[3])
                frame_ms = [int(x) for x in m[4].split()]
            except ValueError:
                continue
            if duration > 0 and len(frame_ms) == count and end - dt.timedelta(seconds=duration) >= start and end <= end_window:
                values.extend(frame_ms)
                seconds += duration
    values.sort()
    run_dir = Path(path).parent
    mem_path, result_path = run_dir / 'memory.jsonl', run_dir / 'result.json'
    memory = []
    if mem_path.exists():
        with mem_path.open(encoding='utf-8-sig') as f:
            for line in f:
                if line.strip():
                    memory.append(json.loads(line))
    run_result = json.loads(result_path.read_text(encoding='utf-8-sig')) if result_path.exists() else {}
    return {'log': str(path), 'start': start.isoformat(), 'window_seconds': 280,
            'run_end_reason': run_result.get('reason'),
            'min_free_ram_mb': min((x['ram_mb'] for x in memory), default=None),
            'max_private_gib': max((x['process_private_gib'] for x in memory), default=None),
            'complete_window': latest_observation is not None and latest_observation >= end_window,
            'last_observation': latest_observation.isoformat() if latest_observation else None,
            'frames': len(values), 'logged_seconds': round(seconds, 1),
            'coverage_pct': round(100 * seconds / 280, 1),
            'fps': round(len(values)/seconds, 3) if seconds else None,
            'median_ms': statistics.median(values) if values else None,
            'p95_ms': values[int((len(values)-1)*.95)] if values else None,
            'other_cpu_logical_median': round(statistics.median(other_cpu), 2) if other_cpu else None,
            'other_cpu_logical_max': round(max(other_cpu), 2) if other_cpu else None,
            'other_cpu_samples': len(other_cpu),
            'other_cpu_warning': 'median above 2 logical processors; repeat under similar load' if other_cpu and statistics.median(other_cpu) > 2 else ('no load samples' if not other_cpu else None),
            'inputs': {key: {'events': len(v), 'missed': sum(x == 0 for x in v), 'minimum_reads': min(v) if v else None} for key,v in key_reads.items()}}

if __name__ == '__main__':
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    result = {'caveat': 'Rounded batch durations; excluded boundary batches; scene, cache and visual equivalence require independent verification.',
              'baseline': measure(sys.argv[1]), 'candidate': measure(sys.argv[2])}
    a, b = result['baseline']['fps'], result['candidate']['fps']
    result['fps_change_pct'] = round((b/a-1)*100, 2) if a and b else None
    print(json.dumps(result, indent=2))
