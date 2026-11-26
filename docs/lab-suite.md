# Automated local checks

`tools/lab_suite.py` runs checks sequentially, captures command output, enforces
timeouts, and writes JSON plus a short Markdown report. Python 3.10+ is sufficient
for the runner; code checks also need the repository's Rust toolchain.
Reports, captures, local configuration and runtime data stay outside Git.

## Code checks, without title files

```powershell
python -B tools/lab_suite.py code --out C:\Temp\victory-checks
```

This runs the asset policy, the runner's synthetic tests, and library tests for
core, PE parsing, formats and the loader. It clears inherited `VICTORY_*` settings. A zero exit with no passing
Rust tests is a failure. Ignored tests are counted separately.

Add `--gpu --lease-script C:\path\to\gpu-lease.py` for the GPU, shader and HLE
library suites. The shared helper must implement `claim`, `release`, `--owner`
and `--work`, refuse other owners, and return a nonzero exit when busy. The
suite uses a unique owner, releases in a finally block, and never steals a lease.
Use the same helper as other agents on the machine. `--repeat 3` repeats the
checks; `--timeout 900` sets a per-command limit. A failure stops the sequence.

### Process trees and the exit grace

Every command runs as a tree of its own. On Windows it is created suspended,
put in a Job Object (its descendants inherit the job) and then resumed, so no
descendant starts outside it; elsewhere it has its own process group. Every
process seen in the tree is remembered with its creation time, so a reused PID
is never taken for one of ours. A timeout or an interruption ends the whole job.

When a command has ended, every process seen in it must be gone before anything
else happens, waiting up to `--exit-grace` seconds (180 by default: a process
of many GB takes a while to leave). A process is gone when it can no longer be
opened, or when it has ended and holds less than 64 MiB (the larger of its
working set and its committed memory). A process that has ended but is still
giving back GB counts as present: the memory, and perhaps the card, are still
taken. One that has ended with little memory, held open only by someone else's
handle, counts as gone. Each command's `result.json` records `tree`
(`pids_seen`, `still_present`, `why` for each of those, such as `running` or
`exited, memory still held: 2400 MB`, `waited_seconds`, `job`) and `tree_gone`.

If processes are still present after the grace, that command is an `error`,
nothing more runs, and the **GPU reservation is left taken on purpose**: another
agent must not get the card and the memory while they are still in use. The
report is `error` with `lease_left_taken: true`, and its message names the
owner, the PIDs still present and the exact command that releases it. To
recover by hand: check that those PIDs are gone (Task Manager, or
`tasklist /FI "PID eq <pid>"`), then run that command, which is
`python <lease-script> release --owner <owner>`.

The runner has its own synthetic tests:

```powershell
python -B -m unittest discover -s tests -p test_lab_suite.py -v
```

## Private runtime matrix

```powershell
python -B tools/lab_suite.py runtime --manifest C:\Temp\runtime-cases.json --out C:\Temp\victory-runs
```

The manifest is a local file, not an artifact to publish. Its top-level keys are
`runner` (a trusted PowerShell runtime runner), `lease_script` and `cases`.
Each case contains `name`, `binary`, `commit`, `sha256`, and optionally `console`
(`series-x` or `series-s`), `heap_sync_tables`, `max_seconds`, `profile`,
`indirect_window`, `settings`, `keys` and `run_folders`. `--case NAME` selects one case. Use the same build and scene
when testing a single setting; declare cold caches and competing applications.

`settings` is an object of further switches, one measurement per manifest line:
`{"VICTORY_TEXTURE_NO_WAIT": "1"}`. Names match `^VICTORY_[A-Z0-9_]+$`; values
are strings or integers whose text matches `^[A-Za-z0-9_.,:-]{0,64}$` (no spaces,
quotes or semicolons). Switches another key or the adapter already governs are
refused: any name that begins with `VICTORY_PROFILE`, and
`VICTORY_HEAP_SYNC_TABLES`, `VICTORY_INDIRECT_WINDOW`, `VICTORY_FRAME_LOG`, `VICTORY_CAPTURE`,
`VICTORY_KEYS`, `VICTORY_CARD_METER`, `VICTORY_STACKS`, `VICTORY_CONFIG` and
`VICTORY_LOG`. Every selected case's
settings are checked before any GPU reservation or launch. They reach the
adapter as one argument, `-Settings "NAME1=V1;NAME2=V2"`, ordered by name, and
the argument is left out when a case has none. The report keeps
`requested_settings`; a requested setting that the adapter's `launch.json` does
not declare with the same value under `settings` fails the case, so a setting
asked for and not applied never passes for measured.

`keys` scripts input for a run nobody plays by hand, in the layer's own form
(`VICTORY_KEYS`, diagnostics builds): entries separated by commas, without
spaces, each `<seconds>:<key>[~<hold>][*<times>[/<gap>]]`, as in
`"150:Enter,430:W~5*3/12"`. A key is a letter, a digit, or `Enter`, `Esc`, `Tab`,
`Back`, `Up`, `Down`, `Left`, `Right`; a hold lasts 0.2 to 60 seconds; a press
repeats 1 to 100 times, `gap` seconds apart (10 unless given), and the gap must
leave room for the hold. The suite checks the whole script before anything runs,
and refuses a press that would end after the case's `max_seconds`: it would
never be pressed. The script reaches the adapter as `-Keys "<script>"`, the
argument is left out when a case has none, and the report keeps `requested_keys`;
a script the adapter's `launch.json` does not declare unchanged under `keys`
fails the case. `VICTORY_KEYS` itself stays refused under `settings`;
`VICTORY_KEYS_READS` goes there. That the title read a key is in the layer's
log; that anything moved on screen is not: control stays unverified until seen.

`run_folders` is for switches that take a folder by its full path, which cannot
travel under `settings`: `{"VICTORY_SHOT": "shot"}`. The value is a plain name
(lowercase letters, digits and hyphens, at most 32), never a path. It reaches the
adapter as `-RunFolders "NAME1=folder1;NAME2=folder2"`; the adapter makes each
folder inside the run's own fresh directory and sets the switch to its full
path, and declares them in launch.json under `run_folders` (switch to full
path). A folder the adapter does not declare there, or declares anywhere but
directly inside the run's directory, fails the case. A switch cannot be both a
setting and a run folder. What the layer writes there (a frame shot's lines and
pictures) is derived from the title: it stays with the run's private evidence
and is never published.

The runtime adapter accepts `-Binary`, `-Commit`, `-ExpectedSha256`, `-Label`,
`-HeapSyncTables`, `-ConsoleMode`, `-MaxSeconds`, `-RunRoot`, `-LeaseOwner` and
`-LeaseManagedExternally`, plus optional `-Profile`, `-IndirectWindow`,
`-Settings` (applied as environment variables and declared in launch.json) `-Keys` (set as `VICTORY_KEYS` and declared in launch.json as `keys`) and
`-RunFolders` (folders made inside the run, declared in launch.json as
`run_folders`).
It must verify the externally held lease without releasing it, retain its
memory guards, stop its own child, and create exactly one fresh directory
under RunRoot with launch.json, result.json, stderr.log and memory.jsonl.
The suite verifies the binary hash before running anything and owns the lease
across the entire subprocess tree. The outer timeout is the case limit plus
120 seconds. It never imports game paths or bytes into this repository.

## Comparing two runs

Three small scripts read the layer's own log of a run (`stderr.log`) and, where
they are beside it, the runner's `memory.jsonl` and `result.json`. They read
nothing of the title.

- `tools/world-frame-summary.py <log> <start> [--minutes N]` adds up the frame
  batches the layer logs after a moment given with its time zone, in windows of
  N minutes.
- `tools/movement-frame-summary.py <log> [--key 76 --at 540]` does the same
  from one scripted key going down.
- `tools/compare-movement.py <baseline-log> <candidate-log>` sets two runs side
  by side over the 280 seconds after the first scripted `L` (key 76 at 540 s):
  frames, logged seconds and how much of the window they cover, frames a
  second, median and 95th percentile frame, how many scripted keys the title
  read, the least free memory, the peak of the process's private memory and why
  the run ended.

Only batches that lie wholly inside the window count, and the layer rounds a
batch's length, so two runs can be compared with each other but the figures are
not exact. The same keys at the same times do not make the same scene: the
picture, the shader cache and what else the machine was doing have to be
checked apart. The comparison has its own synthetic test:

```powershell
python -B -m unittest discover -s tests -p test_compare_movement.py -v
```

## Meaning of results

Exit 0 means all **code checks** passed. Exit 1 means failure, timeout, invalid
evidence or a reservation error. Exit 2 means runtime evidence is inconclusive.
A bounded runtime that reaches its time limit is not automatically a crash.
Memory-guard termination and nonzero spontaneous exit are failures.

The report records a high-draw **scene candidate**, frame intervals after it,
dropped draws, memory and termination. A high draw count can still produce a
black picture. No picture quality, gameplay, input response or FPS improvement
is certified merely from these counters. Those fields remain unverified until
separate evidence exists. Profiling runs must not be compared as normal FPS
benchmarks. The suite needs no manual supervision to execute and collect these
results; it does not manufacture a visual pass.

Reports are private by default and are never uploaded or pushed automatically.
