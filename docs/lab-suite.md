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
`indirect_window` and `settings`. `--case NAME` selects one case. Use the same build and scene
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

The runtime adapter accepts `-Binary`, `-Commit`, `-ExpectedSha256`, `-Label`,
`-HeapSyncTables`, `-ConsoleMode`, `-MaxSeconds`, `-RunRoot`, `-LeaseOwner` and
`-LeaseManagedExternally`, plus optional `-Profile`, `-IndirectWindow` and
`-Settings` (applied as environment variables and declared in launch.json).
It must verify the externally held lease without releasing it, retain its
memory guards, stop its own child, and create exactly one fresh directory
under RunRoot with launch.json, result.json, stderr.log and memory.jsonl.
The suite verifies the binary hash before running anything and owns the lease
across the entire subprocess tree. The outer timeout is the case limit plus
120 seconds. It never imports game paths or bytes into this repository.

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
