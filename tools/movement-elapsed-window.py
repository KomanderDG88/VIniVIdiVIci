"""Map a real scripted key event to the runner's elapsed-time coordinates."""
import argparse
import datetime as dt
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location('movement', Path(__file__).with_name('movement-frame-summary.py'))
movement = importlib.util.module_from_spec(spec)
spec.loader.exec_module(movement)


def window(run, key=76, scheduled=540, duration=280):
    run = Path(run)
    launch = json.loads((run / 'launch.json').read_text(encoding='utf-8-sig'))
    started = dt.datetime.fromisoformat(launch['started'])
    event = movement.find_start(run / 'stderr.log', key, scheduled)
    seconds = (event - started).total_seconds()
    if seconds < 0 or duration <= 0:
        raise ValueError('invalid event timing or duration')
    return {'start_seconds': round(seconds, 3), 'end_seconds': round(seconds + duration, 3),
            'event_time': event.isoformat(), 'runner_start': started.isoformat(),
            'caveat': 'Input reads and comparable scene still require separate verification.'}


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('run', type=Path)
    p.add_argument('--key', type=int, default=76)
    p.add_argument('--at', type=float, default=540)
    p.add_argument('--duration', type=float, default=280)
    a = p.parse_args()
    print(json.dumps(window(a.run, a.key, a.at, a.duration), indent=2))
