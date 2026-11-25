"""Repeatable local checks; Python standard library only. Never publishes results."""
import argparse
import contextlib
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import uuid


def write_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(value, indent=2), encoding='utf-8')
    temporary.replace(path)


def outside_repository(path, repo):
    path, repo = Path(path).resolve(), Path(repo).resolve()
    if path == repo or repo in path.parents:
        raise ValueError('Reports and runtime data must be outside the repository')
    if any((parent / '.git').exists() for parent in (path, *path.parents)):
        raise ValueError('Output is inside a Git checkout')
    return path


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def clean_environment():
    return {key: value for key, value in os.environ.items()
            if not key.upper().startswith('VICTORY_')}


def log_lines(path):
    with Path(path).open(encoding='utf-8', errors='replace') as stream:
        yield from stream


def run_command(command, cwd, output, timeout):
    """No shell expansion; timeout stops only this command's process tree."""
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    result = {'command': command, 'cwd': str(cwd), 'timeout_seconds': timeout,
              'started': dt.datetime.now(dt.timezone.utc).isoformat()}
    start = time.monotonic()
    options = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {'start_new_session': True}
    with (output / 'stdout.log').open('wb') as stdout, (output / 'stderr.log').open('wb') as stderr:
        try:
            child = subprocess.Popen(command, cwd=cwd, env=clean_environment(),
                                     stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, **options)
            result['pid'] = child.pid
            try:
                result['exit_code'] = child.wait(timeout=timeout)
                result['status'] = 'passed' if child.returncode == 0 else 'failed'
            except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
                result['status'] = 'timeout' if isinstance(error, subprocess.TimeoutExpired) else 'interrupted'
                if child.poll() is None:
                    if os.name == 'nt':
                        subprocess.run(['taskkill', '/PID', str(child.pid), '/T', '/F'],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)
                    else:
                        import signal
                        os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=20)
                result['exit_code'] = child.returncode
        except OSError as error:
            result.update(status='error', error=str(error), exit_code=None)
    result['seconds'] = round(time.monotonic() - start, 3)
    write_json(output / 'result.json', result)
    return result


def rust_results(paths):
    summaries = []
    for path in paths:
        for line in Path(path).read_text(encoding='utf-8', errors='replace').splitlines():
            match = re.search(r'test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored', line)
            if match:
                summaries.append(dict(zip(('outcome', 'passed', 'failed', 'ignored'),
                                          [match[1], *map(int, match.groups()[1:])])) )
    return {'passed': sum(row['passed'] for row in summaries),
            'failed': sum(row['failed'] for row in summaries),
            'ignored': sum(row['ignored'] for row in summaries), 'summaries': len(summaries)}


@contextlib.contextmanager
def gpu_lease(script, owner):
    if not script or not Path(script).is_file():
        raise ValueError('GPU checks require --lease-script (the shared claim/release helper)')
    command = [sys.executable, str(Path(script).resolve())]
    claimed = subprocess.run(command + ['claim', '--owner', owner, '--work', 'automated lab suite'],
                             capture_output=True, text=True, timeout=20)
    if claimed.returncode:
        raise RuntimeError('GPU reservation refused: ' + claimed.stderr.strip())
    try:
        yield
    finally:
        released = subprocess.run(command + ['release', '--owner', owner],
                                  capture_output=True, text=True, timeout=20)
        if released.returncode:
            raise RuntimeError('GPU release failed: ' + released.stderr.strip())


def runtime_summary(run):
    """Log-derived candidates are never treated as proof of picture or control."""
    run = Path(run)
    launch = json.loads((run / 'launch.json').read_text(encoding='utf-8-sig'))
    end = json.loads((run / 'result.json').read_text(encoding='utf-8-sig'))
    first_scene = None
    intervals = []
    malformed = 0
    dropped_max = 0
    for line in log_lines(run / 'stderr.log'):
        try:
            stamp = dt.datetime.fromisoformat(line.split()[0].replace('Z', '+00:00'))
        except (ValueError, IndexError):
            continue
        host = re.search(r'(\d+) draws/frame, dropped (\d+)', line)
        if host:
            dropped_max = max(dropped_max, int(host[2]))
            if int(host[1]) >= 500 and first_scene is None:
                first_scene = stamp
        batch = re.search(r'frame times: (\d+) frames in ([\d.]+) s:.*each: (.*)', line)
        if batch and first_scene is not None:
            # Exclude the batch straddling the first high-draw marker.
            if stamp - dt.timedelta(seconds=float(batch[2])) < first_scene:
                continue
            try:
                values = [float(value) for value in batch[3].split()]
            except ValueError:
                malformed += 1
                continue
            if len(values) != int(batch[1]) or any(not math.isfinite(x) or x <= 0 for x in values):
                malformed += 1
                continue
            intervals.extend(values)
    memory = [json.loads(line) for line in (run / 'memory.jsonl').read_text(encoding='utf-8-sig').splitlines() if line.strip()]
    ordered = sorted(intervals)
    def percentile(p):
        if not ordered:
            return None
        position = (len(ordered) - 1) * p
        lo = int(position)
        hi = min(lo + 1, len(ordered) - 1)
        return round(ordered[lo] + (ordered[hi] - ordered[lo]) * (position - lo), 3)
    return {'run_dir': str(run), 'commit': launch['commit'], 'binary_sha256': launch['sha256'],
            'console': launch.get('console'), 'settings': launch.get('settings'),
            'profile': launch.get('profile', False), 'end': end,
            'high_draw_scene_candidate': first_scene.isoformat() if first_scene else None,
            'visual_quality': 'unverified', 'player_control': 'unverified',
            'frames_sampled_after_candidate': len(intervals), 'malformed_batches': malformed,
            'logged_frame_fps': round(1000 * len(intervals) / sum(intervals), 3) if intervals else None,
            'frame_ms_p50': percentile(.5), 'frame_ms_p95': percentile(.95),
            'max_dropped_draws': dropped_max,
            'min_available_ram_mb': min((x['ram_mb'] for x in memory), default=None),
            'max_private_gib': max((x['process_private_gib'] for x in memory), default=None),
            'comparison_eligible': False,
            'caveats': ['High draw count does not prove visible or playable content.',
                        'Sampled log intervals may span changing scenes and omit unlogged time.',
                        'Profiling, cache state and competing tasks affect timings.']}


def code_checks(args, directory, owner):
    cases = [('policy', ['cargo', 'xtask', 'policy'], False),
             ('runner-selftests', [sys.executable, '-B', '-m', 'unittest', 'discover',
                                  '-s', 'tests', '-p', 'test_lab_suite.py', '-v'], False)]
    for crate in ('victory-core', 'victory-pe', 'victory-formats', 'victory-loader'):
        cases.append((crate, ['cargo', 'test', '--locked', '-p', crate, '--lib', '--', '--test-threads=1'], True))
    if args.gpu:
        for crate in ('victory-gpu', 'victory-shader', 'victory-hle'):
            cases.append((crate, ['cargo', 'test', '--locked', '-p', crate, '--lib', '--', '--test-threads=1'], True))
    results = []
    lease = gpu_lease(args.lease_script, owner) if args.gpu else contextlib.nullcontext()
    with lease:
        for repetition in range(args.repeat):
            for name, command, rust in cases:
                folder = directory / f'{repetition + 1:02d}-{name}'
                row = run_command(command, args.repo, folder, args.timeout)
                row.update(name=name, repetition=repetition + 1)
                if rust:
                    row['tests'] = rust_results([folder / 'stdout.log', folder / 'stderr.log'])
                    if row['status'] == 'passed' and (row['tests']['passed'] == 0 or row['tests']['failed']):
                        row['status'] = 'failed'
                        row['reason'] = 'No passing Rust tests, or a failing test summary'
                results.append(row)
                write_json(directory / 'progress.json', results)
                if row['status'] != 'passed':
                    return results
    return results


# Switches another case key or the runtime adapter already governs: a case's
# `settings` may not set them (a second source would make the run ambiguous).
RESERVED_SETTINGS = {'VICTORY_HEAP_SYNC_TABLES', 'VICTORY_INDIRECT_WINDOW', 'VICTORY_PROFILE',
                     'VICTORY_FRAME_LOG', 'VICTORY_CAPTURE', 'VICTORY_KEYS', 'VICTORY_CARD_METER',
                     'VICTORY_STACKS'}


def case_settings(case):
    """A case's optional `settings` ({"VICTORY_X": value}), validated, as name -> text.

    Names are VICTORY_ switches; values are strings or integers whose text has no
    spaces, quotes or semicolons (it travels in one `-Settings "N=V;..."` argument).
    """
    settings = case.get('settings', {})
    if not isinstance(settings, dict):
        raise ValueError('settings must be an object of VICTORY_ switches: ' + case['name'])
    checked = {}
    for name, value in settings.items():
        if not isinstance(name, str) or not re.fullmatch(r'VICTORY_[A-Z0-9_]+', name):
            raise ValueError(f'Invalid setting name {name!r} in case {case["name"]}')
        if name in RESERVED_SETTINGS or name.startswith('VICTORY_PROFILE_'):
            raise ValueError(f'Setting {name} is governed by another case key or the runner: {case["name"]}')
        if isinstance(value, bool) or not isinstance(value, (str, int)):
            raise ValueError(f'Setting {name} must be a string or an integer: {case["name"]}')
        text = str(value)
        if not re.fullmatch(r'[A-Za-z0-9_.,:-]{0,64}', text):
            raise ValueError(f'Invalid value {text!r} for setting {name} in case {case["name"]}')
        checked[name] = text
    return dict(sorted(checked.items()))


def settings_argument(settings):
    """`N1=V1;N2=V2` ordered by name, or None when a case asks for none."""
    return ';'.join(f'{name}={value}' for name, value in sorted(settings.items())) or None


def settings_mismatch(requested, launched):
    """Requested settings the runner did not declare as applied (launch.json `settings`)."""
    launched = launched if isinstance(launched, dict) else {}
    return [f'{name} (asked {value!r}, launch.json {launched.get(name)!r})'
            for name, value in requested.items() if str(launched.get(name)) != value]


def runtime_checks(args, directory, owner):
    manifest = json.loads(Path(args.manifest).read_text(encoding='utf-8-sig'))
    runner = Path(manifest['runner']).resolve()
    results = []
    cases = manifest['cases']
    if args.case:
        cases = [case for case in cases if case['name'] == args.case]
    if not cases:
        raise ValueError('No runtime cases selected')
    # Every case's settings are checked before any GPU reservation or launch.
    requested = {case['name']: case_settings(case) for case in cases}
    for index, case in enumerate(cases):
        if not re.fullmatch(r'[a-z0-9-]+', case['name']):
            raise ValueError('Case names use lowercase letters, digits and hyphens')
        if sha256(case['binary']).lower() != case['sha256'].lower():
            raise ValueError('Runtime binary hash mismatch: ' + case['name'])
        label = f'auto-{index}-{case["name"]}'
        run_root = directory / 'runtime'
        folder = directory / label
        duration = int(case.get('max_seconds', 600))
        if not 120 <= duration <= 3600:
            raise ValueError('Runtime duration must be between 120 and 3600 seconds')
        command = ['powershell.exe', '-NoProfile', '-File', str(runner),
                   '-Binary', case['binary'], '-Commit', case['commit'],
                   '-ExpectedSha256', case['sha256'], '-Label', label,
                   '-HeapSyncTables', str(case.get('heap_sync_tables', 1)),
                   '-ConsoleMode', case.get('console', 'series-x'),
                   '-MaxSeconds', str(duration), '-RunRoot', str(run_root), '-LeaseOwner', owner,
                   '-LeaseManagedExternally']
        if case.get('profile'):
            command.append('-Profile')
        if case.get('indirect_window'):
            command.append('-IndirectWindow')
        settings = requested[case['name']]
        if settings:
            command.extend(['-Settings', settings_argument(settings)])
        with gpu_lease(manifest['lease_script'], owner):
            row = run_command(command, args.repo, folder, duration + 120)
        row.update(name=case['name'], runner_sha256=sha256(runner), requested_settings=settings)
        runs = list(run_root.glob(label + '-*')) if run_root.exists() else []
        if row['status'] == 'passed' and len(runs) == 1:
            try:
                row['runtime'] = runtime_summary(runs[0])
                end = row['runtime']['end']
                failed = end['reason'] not in ('time_limit', 'exited') or (end['reason'] == 'exited' and end['exit_code'] != 0)
                row['status'] = 'failed' if failed else 'inconclusive'
                # A setting asked for and not applied cannot pass for measured.
                missing = settings_mismatch(settings, row['runtime'].get('settings'))
                if missing:
                    row.update(status='failed', reason='Requested settings not applied by the runner: ' + ', '.join(missing))
            except (OSError, ValueError, KeyError) as error:
                row.update(status='failed', error='Missing or invalid runtime evidence: ' + str(error))
        elif row['status'] == 'passed':
            row.update(status='failed', error='Expected exactly one fresh runtime artifact directory')
        results.append(row)
        write_json(directory / 'progress.json', results)
        if row['status'] not in ('passed', 'inconclusive'):
            break
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['code', 'runtime'])
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--out', type=Path, required=True, help='Private output directory outside any checkout')
    parser.add_argument('--gpu', action='store_true')
    parser.add_argument('--lease-script', type=Path)
    parser.add_argument('--repeat', type=int, default=1)
    parser.add_argument('--timeout', type=int, default=900)
    parser.add_argument('--manifest', type=Path)
    parser.add_argument('--case')
    args = parser.parse_args()
    args.repo = args.repo.resolve()
    if args.repeat < 1 or args.timeout < 1:
        parser.error('repeat and timeout must be positive')
    if args.mode == 'runtime' and not args.manifest:
        parser.error('runtime requires a private --manifest')
    root = outside_repository(args.out, args.repo)
    run_id = dt.datetime.now().strftime('%Y%m%d-%H%M%S') + '-' + uuid.uuid4().hex[:8]
    directory = root / run_id
    directory.mkdir(parents=True, exist_ok=False)
    owner = 'suite-' + run_id
    identity = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=args.repo, capture_output=True, text=True)
    dirty = subprocess.run(['git', 'status', '--porcelain'], cwd=args.repo, capture_output=True, text=True)
    report = {'mode': args.mode, 'run_id': run_id, 'repository_commit': identity.stdout.strip(),
              'dirty_worktree': bool(dirty.stdout.strip()), 'suite_sha256': sha256(__file__),
              'directory': str(directory), 'results': []}
    try:
        report['results'] = (code_checks if args.mode == 'code' else runtime_checks)(args, directory, owner)
        statuses = {row['status'] for row in report['results']}
        report['status'] = 'failed' if statuses - {'passed', 'inconclusive'} else ('inconclusive' if 'inconclusive' in statuses else 'passed')
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.SubprocessError) as error:
        report.update(status='error', error=str(error))
    except KeyboardInterrupt:
        report.update(status='interrupted', error='Interrupted by operator')
    write_json(directory / 'report.json', report)
    lines = ['# Local test results', '', f'Status: **{report["status"]}**', '',
             f'Repository: `{report["repository_commit"]}`; dirty: `{report["dirty_worktree"]}`.', '',
             '| Check | Result | Seconds |', '| --- | --- | ---: |']
    lines.extend(f'| {row["name"]} | {row["status"]} | {row["seconds"]} |' for row in report['results'])
    lines.extend(['', 'Runtime image quality and player control require separate evidence; frame count is insufficient.', '', report.get('error', '')])
    (directory / 'report.md').write_text('\n'.join(lines), encoding='utf-8')
    print(json.dumps({'status': report['status'], 'report': str(directory / 'report.json')}))
    return 0 if report['status'] == 'passed' else (2 if report['status'] == 'inconclusive' else 1)


if __name__ == '__main__':
    raise SystemExit(main())
