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


# A command's processes are followed as a tree: on Windows a Job Object the
# child is put in before it runs (it is created suspended, so no descendant
# can start outside it; descendants inherit the job), elsewhere the child's
# own process group. Every process seen in the tree is remembered with its
# creation time, so that a reused PID is never taken for one of ours.
if os.name == 'nt':
    import ctypes
    from ctypes import wintypes

    _kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)
    _ntdll = ctypes.WinDLL('ntdll')
    _kernel32.CreateJobObjectW.restype = wintypes.HANDLE
    _kernel32.CreateJobObjectW.argtypes = (ctypes.c_void_p, wintypes.LPCWSTR)
    _kernel32.AssignProcessToJobObject.restype = wintypes.BOOL
    _kernel32.AssignProcessToJobObject.argtypes = (wintypes.HANDLE, wintypes.HANDLE)
    _kernel32.TerminateJobObject.restype = wintypes.BOOL
    _kernel32.TerminateJobObject.argtypes = (wintypes.HANDLE, wintypes.UINT)
    _kernel32.QueryInformationJobObject.restype = wintypes.BOOL
    _kernel32.QueryInformationJobObject.argtypes = (wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p,
                                                    wintypes.DWORD, ctypes.POINTER(wintypes.DWORD))
    _kernel32.OpenProcess.restype = wintypes.HANDLE
    _kernel32.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    _kernel32.GetProcessTimes.restype = wintypes.BOOL
    _kernel32.GetProcessTimes.argtypes = (wintypes.HANDLE,) + (ctypes.POINTER(wintypes.FILETIME),) * 4
    _kernel32.WaitForSingleObject.restype = wintypes.DWORD
    _kernel32.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
    _kernel32.CloseHandle.restype = wintypes.BOOL
    _kernel32.CloseHandle.argtypes = (wintypes.HANDLE,)
    _kernel32.K32GetProcessMemoryInfo.restype = wintypes.BOOL
    _kernel32.K32GetProcessMemoryInfo.argtypes = (wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD)
    _ntdll.NtResumeProcess.restype = ctypes.c_long
    _ntdll.NtResumeProcess.argtypes = (wintypes.HANDLE,)
    CREATE_SUSPENDED = 0x4
    JOB_PROCESS_ID_LIST = 3  # JobObjectBasicProcessIdList

    class _ProcessIds(ctypes.Structure):
        _fields_ = [('assigned', wintypes.DWORD), ('listed', wintypes.DWORD), ('ids', ctypes.c_size_t * 4096)]

    class _MemoryCounters(ctypes.Structure):  # PROCESS_MEMORY_COUNTERS
        _fields_ = [('cb', wintypes.DWORD), ('PageFaultCount', wintypes.DWORD)] + [
            (name, ctypes.c_size_t) for name in (
                'PeakWorkingSetSize', 'WorkingSetSize', 'QuotaPeakPagedPoolUsage', 'QuotaPagedPoolUsage',
                'QuotaPeakNonPagedPoolUsage', 'QuotaNonPagedPoolUsage', 'PagefileUsage', 'PeakPagefileUsage')]

# A process that has ended is gone once the memory it still holds is below this:
# the game's process keeps GB for a while after it has ended.
EXITED_MEMORY_LIMIT = 64 << 20


def process_state(pid):
    """(creation time, ended, memory held in bytes or None) of process `pid`;
    None if it cannot be opened (it is gone, or never was).

    A process that has ended can still be listed, held open by someone, while
    the system takes its memory back: see `still_there`. Elsewhere than on
    Windows: (True, False, None) while it exists."""
    if os.name != 'nt':
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return None
        except PermissionError:
            pass
        return True, False, None
    handle = _kernel32.OpenProcess(0x1000 | 0x00100000, False, pid)  # QUERY_LIMITED_INFORMATION | SYNCHRONIZE
    if not handle:
        return None
    try:
        times = [wintypes.FILETIME() for _ in range(4)]
        if not _kernel32.GetProcessTimes(handle, *(ctypes.byref(t) for t in times)):
            return None
        created = times[0].dwHighDateTime << 32 | times[0].dwLowDateTime
        ended = _kernel32.WaitForSingleObject(handle, 0) == 0  # signaled
        counters = _MemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        memory = None
        if _kernel32.K32GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            memory = max(counters.WorkingSetSize, counters.PagefileUsage)
        return created, ended, memory
    finally:
        _kernel32.CloseHandle(handle)


def process_created(pid):
    """Process `pid`'s creation time while it can be opened, else None."""
    state = process_state(pid)
    return state[0] if state else None


def still_there(ended, memory, limit=EXITED_MEMORY_LIMIT):
    """Why a process (one of ours, still openable) counts as present, or None if it is gone.

    Running: present. Ended but still holding `limit` bytes or more: present,
    the memory (and perhaps the card) is still taken. Ended with the memory
    unknown: present (we cannot tell). Ended with little memory but held open
    by someone else: gone, or a foreign handle would hold the lease for ever."""
    if not ended:
        return 'running'
    if memory is None:
        return 'exited, memory unknown'
    if memory >= limit:
        return f'exited, memory still held: {memory >> 20} MB'
    return None


class ProcessTree:
    """The processes of one command (see above)."""

    def __init__(self):
        self.job = None
        self.root = None
        self.seen = {}  # pid -> creation time

    def start(self, child):
        """With `child` just created (suspended, on Windows): puts it in a job, then lets it run."""
        self.root = child.pid
        if os.name != 'nt':
            self.seen[child.pid] = process_created(child.pid)
            return
        handle = int(child._handle)  # the process handle Popen holds
        job = _kernel32.CreateJobObjectW(None, None)
        if job and _kernel32.AssignProcessToJobObject(job, handle):
            self.job = job
        elif job:
            _kernel32.CloseHandle(job)
        self.collect()
        if _ntdll.NtResumeProcess(handle) != 0:
            child.kill()
            raise OSError('The command could not be resumed after being put in its job')

    def pids(self):
        if self.job is None:
            return [self.root] if self.root is not None else []
        ids = _ProcessIds()
        if not _kernel32.QueryInformationJobObject(self.job, JOB_PROCESS_ID_LIST, ctypes.byref(ids),
                                                   ctypes.sizeof(ids), None):
            return []
        return list(ids.ids[:ids.listed])

    def collect(self):
        """Remembers the processes in the tree now."""
        for pid in self.pids():
            if pid not in self.seen:
                created = process_created(pid)
                if created is not None:
                    self.seen[pid] = created

    def stop(self):
        """Ends every process of the tree (on a timeout or an interruption)."""
        self.collect()
        if os.name != 'nt':
            import signal
            with contextlib.suppress(ProcessLookupError):
                os.killpg(self.root, signal.SIGKILL)
        elif self.job is not None:
            _kernel32.TerminateJobObject(self.job, 1)
        elif self.root is not None:
            subprocess.run(['taskkill', '/PID', str(self.root), '/T', '/F'],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)

    def present(self):
        """{pid: why} for the processes seen that are still there as themselves (the
        same creation time): running, or ended with their memory still held."""
        if os.name != 'nt':
            if self.root is None:
                return {}
            try:
                os.killpg(self.root, 0)
            except ProcessLookupError:
                return {}
            except PermissionError:
                pass
            return {self.root: 'running'}
        still = {}
        for pid, created in sorted(self.seen.items()):
            state = process_state(pid)
            if state and state[0] == created:
                why = still_there(state[1], state[2])
                if why:
                    still[pid] = why
        return still

    def close(self):
        if self.job is not None:
            _kernel32.CloseHandle(self.job)  # no kill-on-close: closing ends nothing
            self.job = None


def wait_collecting(child, tree, timeout):
    """`child.wait(timeout)`, remembering the tree's processes while it runs."""
    deadline = time.monotonic() + timeout
    while True:
        tree.collect()
        left = deadline - time.monotonic()
        if left <= 0:
            raise subprocess.TimeoutExpired(child.args, timeout)
        try:
            return child.wait(timeout=min(0.25, left))
        except subprocess.TimeoutExpired:
            continue


def run_command(command, cwd, output, timeout, grace=180):
    """No shell expansion. The command runs as a process tree of its own; a
    timeout or an interruption ends that whole tree. Then every process seen in
    it must be gone, waiting up to `grace` seconds (a process of many GB takes
    a while to leave): `tree_gone` says whether they are."""
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    result = {'command': command, 'cwd': str(cwd), 'timeout_seconds': timeout,
              'started': dt.datetime.now(dt.timezone.utc).isoformat()}
    start = time.monotonic()
    tree = ProcessTree()
    options = ({'creationflags': subprocess.CREATE_NO_WINDOW | CREATE_SUSPENDED} if os.name == 'nt'
               else {'start_new_session': True})
    with (output / 'stdout.log').open('wb') as stdout, (output / 'stderr.log').open('wb') as stderr:
        try:
            child = subprocess.Popen(command, cwd=cwd, env=clean_environment(),
                                     stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, **options)
            result['pid'] = child.pid
            try:
                tree.start(child)
                result['exit_code'] = wait_collecting(child, tree, timeout)
                result['status'] = 'passed' if child.returncode == 0 else 'failed'
            except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
                result['status'] = 'timeout' if isinstance(error, subprocess.TimeoutExpired) else 'interrupted'
                tree.stop()
                child.wait(timeout=20)
                result['exit_code'] = child.returncode
        except OSError as error:
            result.update(status='error', error=str(error), exit_code=None)
    tree.collect()
    waiting = time.monotonic()
    still = tree.present()
    while still and time.monotonic() - waiting < grace:
        time.sleep(0.5)
        still = tree.present()
    result['tree'] = {'pids_seen': sorted(tree.seen), 'still_present': sorted(still),
                      'why': {str(pid): why for pid, why in sorted(still.items())},
                      'waited_seconds': round(time.monotonic() - waiting, 3), 'job': tree.job is not None}
    result['tree_gone'] = not still
    tree.close()
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


class LeaseHeld(RuntimeError):
    """The GPU reservation was left taken: a command's processes did not go.
    `results` carries the rows run so far into the report."""

    def __init__(self, message):
        super().__init__(message)
        self.results = []


class Lease:
    """The suite's GPU reservation while it is held; `keep` leaves it taken on the way out."""

    def __init__(self, command, owner):
        self.command, self.owner, self.kept = command, owner, None

    def keep(self, row):
        self.kept = row

    def release_by_hand(self):
        return subprocess.list2cmdline(self.command + ['release', '--owner', self.owner])


@contextlib.contextmanager
def gpu_lease(script, owner):
    """Claims the GPU and yields a `Lease`. On the way out it is released, unless a
    command's processes were still present (`Lease.keep`): then it stays taken, and
    `LeaseHeld` names the owner, the processes and how to release it by hand."""
    if not script or not Path(script).is_file():
        raise ValueError('GPU checks require --lease-script (the shared claim/release helper)')
    command = [sys.executable, str(Path(script).resolve())]
    claimed = subprocess.run(command + ['claim', '--owner', owner, '--work', 'automated lab suite'],
                             capture_output=True, text=True, timeout=20)
    if claimed.returncode:
        raise RuntimeError('GPU reservation refused: ' + claimed.stderr.strip())
    lease = Lease(command, owner)
    try:
        yield lease
    finally:
        if lease.kept is not None:
            tree = lease.kept.get('tree', {})
            raise LeaseHeld(f'GPU reservation LEFT TAKEN by {owner}: processes of '
                            f'{lease.kept.get("name") or lease.kept.get("command")} were still present after '
                            f'{tree.get("waited_seconds")} s ({tree.get("why") or tree.get("still_present")}). Make sure they are '
                            f'gone, then release it by hand: {lease.release_by_hand()}')
        released = subprocess.run(command + ['release', '--owner', owner],
                                  capture_output=True, text=True, timeout=20)
        if released.returncode:
            raise RuntimeError('GPU release failed: ' + released.stderr.strip())


def left_running(row, lease):
    """A command whose processes did not all go: an error, nothing more runs, and
    the GPU reservation (if any) is kept."""
    if row.get('tree_gone', True):
        return False
    tree = row.get('tree', {})
    still = tree.get('why') or tree.get('still_present')
    row.update(status='error', reason=f'Processes of the command still present after the exit grace: {still}')
    if lease is not None:
        lease.keep(row)
    return True


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
            'console': launch.get('console'), 'settings': launch.get('settings'), 'keys': launch.get('keys'),
            'run_folders': launch.get('run_folders'),
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
    grace = getattr(args, 'exit_grace', 180)
    try:
        with (gpu_lease(args.lease_script, owner) if args.gpu else contextlib.nullcontext()) as lease:
            for repetition in range(args.repeat):
                for name, command, rust in cases:
                    folder = directory / f'{repetition + 1:02d}-{name}'
                    row = run_command(command, args.repo, folder, args.timeout, grace)
                    row.update(name=name, repetition=repetition + 1)
                    if rust:
                        row['tests'] = rust_results([folder / 'stdout.log', folder / 'stderr.log'])
                        if row['status'] == 'passed' and (row['tests']['passed'] == 0 or row['tests']['failed']):
                            row['status'] = 'failed'
                            row['reason'] = 'No passing Rust tests, or a failing test summary'
                    gone = not left_running(row, lease)
                    results.append(row)
                    write_json(directory / 'progress.json', results)
                    if row['status'] != 'passed' or not gone:
                        return results
    except LeaseHeld as held:
        held.results = results
        raise
    return results


# Switches another case key or the runtime adapter already governs: a case's
# `settings` may not set them (a second source would make the run ambiguous).
RESERVED_SETTINGS = {'VICTORY_HEAP_SYNC_TABLES', 'VICTORY_INDIRECT_WINDOW', 'VICTORY_PROFILE',
                     'VICTORY_FRAME_LOG', 'VICTORY_CAPTURE', 'VICTORY_KEYS', 'VICTORY_CARD_METER',
                     'VICTORY_STACKS', 'VICTORY_CONFIG', 'VICTORY_LOG'}


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
        if name in RESERVED_SETTINGS or name.startswith('VICTORY_PROFILE'):
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


def case_run_folders(case, settings):
    """A case's optional `run_folders` ({"VICTORY_X": "name"}), validated, as name -> folder name.

    Some switches take a folder by its full path (`VICTORY_SHOT`), and a path
    cannot travel under `settings`. A run folder is named here, without any
    path: the runner makes it inside the run's own fresh directory and hands
    the switch its full path, so what the layer writes there stays with the
    run's private evidence.
    """
    folders = case.get('run_folders', {})
    if not isinstance(folders, dict):
        raise ValueError('run_folders must be an object of VICTORY_ switches: ' + case['name'])
    checked = {}
    for name, folder in folders.items():
        if not isinstance(name, str) or not re.fullmatch(r'VICTORY_[A-Z0-9_]+', name):
            raise ValueError(f'Invalid run folder switch {name!r} in case {case["name"]}')
        if name in RESERVED_SETTINGS or name.startswith('VICTORY_PROFILE') or name in settings:
            raise ValueError(f'Run folder switch {name} is governed elsewhere (settings, another case key or the runner): {case["name"]}')
        if not isinstance(folder, str) or not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,31}', folder):
            raise ValueError(f'A run folder is a plain name of lowercase letters, digits and hyphens, not a path: '
                             f'{folder!r} for {name} in case {case["name"]}')
        checked[name] = folder
    if len(set(checked.values())) != len(checked):
        raise ValueError('Two switches of a case cannot share a run folder: ' + case['name'])
    return dict(sorted(checked.items()))


def run_folders_mismatch(requested, launched, run):
    """Requested run folders the runner did not declare as a folder of that name inside the run."""
    launched = launched if isinstance(launched, dict) else {}
    wrong = []
    for name, folder in requested.items():
        declared = launched.get(name)
        inside = isinstance(declared, str) and Path(declared).name == folder and Path(declared).parent == Path(run)
        if not inside:
            wrong.append(f'{name} (asked {folder!r} inside the run, launch.json {declared!r})')
    return wrong


# The keys a scripted press may name, and its limits, as the layer reads
# VICTORY_KEYS (crates/victory-hle/src/modules/gameinput/script.rs).
KEY_NAMES = {'Enter', 'Esc', 'Tab', 'Back', 'Up', 'Down', 'Left', 'Right'}
KEY_SHORTEST_HOLD, KEY_LONGEST_HOLD, KEY_USUAL_GAP = 0.2, 60.0, 10.0
_NUMBER = r'\d+(?:\.\d+)?'
_KEYS_ENTRY = re.compile(rf'({_NUMBER}):([A-Za-z0-9]+)(?:~({_NUMBER}))?(?:\*(\d+)(?:/({_NUMBER}))?)?')


def case_keys(case, duration):
    """A case's optional `keys`, a VICTORY_KEYS script, validated; None without one.

    Entries are separated by commas, without spaces:
    `<seconds>:<key>[~<hold>][*<times>[/<gap>]]`. A key is a letter, a digit or
    one of KEY_NAMES; a hold lasts 0.2 to 60 seconds; a press repeats 1 to 100
    times, `gap` seconds apart (10 unless given), and the gap leaves room for the
    hold. The last press must end within the case's `duration`: a key asked for
    after the session is over was never pressed.
    """
    keys = case.get('keys')
    if keys is None:
        return None
    name = case['name']
    if not isinstance(keys, str) or not re.fullmatch(r'[A-Za-z0-9:.,~*/]{1,256}', keys):
        raise ValueError(f'keys must be a VICTORY_KEYS script of at most 256 characters, without spaces: {name}')
    for entry in keys.split(','):
        match = _KEYS_ENTRY.fullmatch(entry)
        if not match:
            raise ValueError(f'Invalid keys entry {entry!r} in case {name}')
        at, key, hold, times, gap = match.groups()
        if key not in KEY_NAMES and len(key) != 1:
            raise ValueError(f'Unknown key {key!r} in keys entry {entry!r} of case {name}')
        hold = None if hold is None else float(hold)
        if hold is not None and not KEY_SHORTEST_HOLD <= hold <= KEY_LONGEST_HOLD:
            raise ValueError(f'A hold lasts 0.2 to 60 seconds: keys entry {entry!r} of case {name}')
        times = 1 if times is None else int(times)
        gap = KEY_USUAL_GAP if gap is None else float(gap)
        if not 1 <= times <= 100:
            raise ValueError(f'A press repeats 1 to 100 times: keys entry {entry!r} of case {name}')
        held = KEY_SHORTEST_HOLD if hold is None else hold
        if times > 1 and gap < held + KEY_SHORTEST_HOLD:
            raise ValueError(f'The gap leaves no room for the hold: keys entry {entry!r} of case {name}')
        if float(at) + (times - 1) * gap + held > duration:
            raise ValueError(f'keys entry {entry!r} ends after the {duration} seconds of case {name}')
    return keys


def runtime_checks(args, directory, owner):
    manifest = json.loads(Path(args.manifest).read_text(encoding='utf-8-sig'))
    runner = Path(manifest['runner']).resolve()
    results = []
    cases = manifest['cases']
    # A name is how --case selects and how settings are kept: a repeated one
    # would let one case run with another's settings unseen.
    names = [case['name'] for case in cases]
    repeated = sorted({name for name in names if names.count(name) > 1})
    if repeated:
        raise ValueError('Repeated runtime case names: ' + ', '.join(repeated))
    if args.case:
        cases = [case for case in cases if case['name'] == args.case]
    if not cases:
        raise ValueError('No runtime cases selected')
    # Every case's settings are checked before any GPU reservation or launch.
    requested = {case['name']: case_settings(case) for case in cases}
    # And its scripted keys, against the time the case runs.
    scripted = {case['name']: case_keys(case, int(case.get('max_seconds', 600))) for case in cases}
    # And the folders it asks for inside its run.
    foldered = {case['name']: case_run_folders(case, requested[case['name']]) for case in cases}
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
        keys = scripted[case['name']]
        if keys:
            command.extend(['-Keys', keys])
        folders = foldered[case['name']]
        if folders:
            command.extend(['-RunFolders', settings_argument(folders)])
        try:
            with gpu_lease(manifest['lease_script'], owner) as lease:
                row = run_command(command, args.repo, folder, duration + 120, getattr(args, 'exit_grace', 180))
                row.update(name=case['name'], runner_sha256=sha256(runner), requested_settings=settings,
                           requested_keys=keys, requested_run_folders=folders)
                left_running(row, lease)
        except LeaseHeld as held:
            # The game's processes did not all go: nothing more runs, and the reservation stays taken.
            results.append(row)
            write_json(directory / 'progress.json', results)
            held.results = results
            raise
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
                # Nor can keys asked for and not scripted by the runner.
                elif keys and row['runtime'].get('keys') != keys:
                    row.update(status='failed', reason='Requested keys not applied by the runner: asked '
                               f'{keys!r}, launch.json {row["runtime"].get("keys")!r}')
                else:
                    outside = run_folders_mismatch(folders, row['runtime'].get('run_folders'), runs[0])
                    if outside:
                        row.update(status='failed', reason='Requested run folders not made by the runner: ' + ', '.join(outside))
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
    parser.add_argument('--exit-grace', type=int, default=180,
                        help='Seconds to wait, after a command ends, for every process it started to be gone')
    args = parser.parse_args()
    args.repo = args.repo.resolve()
    if args.repeat < 1 or args.timeout < 1 or args.exit_grace < 0:
        parser.error('repeat and timeout must be positive, and exit grace not negative')
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
    except LeaseHeld as error:
        # The GPU reservation is still taken on purpose; the message says how to release it.
        report.update(status='error', error=str(error), results=error.results, lease_left_taken=True)
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
