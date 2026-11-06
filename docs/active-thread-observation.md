# Working-thread observation

`thread list --active` and `dashboard` share `ActiveThreadsRequest` and
`ActiveThreadsReport`. Normal `thread list` retains its existing persisted
metadata output. `thread list --observe` returns the observer projection without
filtering idle/unknown rows.

The JSON report follows the existing app-server page convention: `data` is the
array, accompanied by `coverage`. Keeping coverage outside the rows makes an
empty/disconnected result distinguishable from a successful empty observation.

```powershell
$report = teamy-codex --output-format json thread list --active | ConvertFrom-Json
$report.coverage
$report.data | Select-Object id, title, status, last_activity_ms, latest_command

while ($true) {
    $report = teamy-codex --output-format json thread list --active | ConvertFrom-Json
    $report.data | Select-Object title, status, latest_command
    $report.coverage | Select-Object connection, complete, unknown_threads, issues
    Start-Sleep -Seconds 3
}

teamy-codex dashboard
teamy-codex dashboard --refresh-seconds 5
teamy-codex dashboard --once

# Metadata-only diagnostic view when no shared runtime is reachable:
teamy-codex --output-format json thread list --observe --isolated --limit 20
```

## Sources and limits

The default connection launches `codex app-server proxy`, which attaches through
the supported shared app-server control socket. `--socket PATH` explicitly
selects a socket using the proxy's `--sock` option. `--codex-bin PATH` selects
the executable. It never starts/restarts a shared daemon or resumes a thread.
All documented source kinds, including exec, appServer, subagents, and unknown,
are requested without sidebar section/project filters. Persisted metadata uses
`useStateDbOnly: true`, avoiding rollout scan-and-repair. Loaded-session IDs are
also paged and metadata-only `thread/read` results merged, allowing ephemeral or
task-created sessions absent from the persisted/sidebar list to appear.

Coverage is limited to the selected runtime. Other desktop, remote, or hosted
executors are outside it. A standalone server (`--isolated`) may expose stored
metadata but cannot observe those executors. Every lifecycle in isolated mode
is therefore unknown. An unreachable proxy yields no rows with incomplete,
disconnected coverage; an empty array does **not** prove that all work is idle.
The implementation does not inspect runtime session files or process command
lines, and does not bypass unavailable/denied sources.

Runtime status determines inclusion: `active` is running, waiting on approval,
or waiting on user input; `idle` and `systemError` remain distinct. `notLoaded`,
missing status, and unfamiliar status/flags become unknown. Recent timestamps
are used for presentation order only. `latest_turn_status` separately exposes
completed/interrupted/failed/inProgress history when the supported turns API
provides it; a previously completed turn never proves the thread is idle now.

`last_activity_ms` is the maximum of server-created, updated, and recency Unix
timestamps, normalized from seconds. `observed_at_ms` and the coverage staleness
description identify these as point-in-time observations, not a global atomic
snapshot. Unknown counts refer to the observed metadata before the active
filter. Explicit row limits report truncation and make completeness false.
Metadata and loaded IDs are bounded to 100 pages of 100 rows each, per archive
selection. An owned query has a 15-second total deadline and cancellation poll;
the owned child/proxy is terminated and reaped on exit, not the shared daemon.

## Commands and terminal behavior

Latest commands use `thread/items/list` in descending canonical item order
across turns. The first command execution wins, even when producer timestamps
are missing or inconsistent. Search stops after 400 items. Missing, unsupported,
or rejected history is explicit in `latest_command_availability`. The output
contains only command ID/status/time and a safe display string; no command
output or raw command is serialized. Display is deliberately conservative:
known executables and selected cargo/git verbs can be shown, all arguments are
withheld, and arbitrary scripts/unknown executable paths are withheld entirely.
Server error payloads are not emitted.

Dashboard refreshes one snapshot at a time, defaults to a three-second interval,
waits between events, and keeps keyboard handling responsive while a query runs
on a blocking worker. `q`, Escape, and Ctrl-C exit. Guards restore raw mode,
cursor visibility, and the alternate screen after normal exit, errors, or
cancellation. Display clips Unicode text to available terminal columns, with
commands on their own lines. `--once` emits a plain frame for automation/smoke
checks. No inference/model requests are made.

## Validation

Fixture tests cover lifecycle versus recency, task sources, loaded tasks absent
from metadata, command ordering, fail-closed redaction, missing history,
unknown/partial coverage, and narrow/wide terminal text. The repository gates
remain nightly formatting, strict all-feature Clippy, all-feature build/test,
and `git diff --check`.

Live coverage must be verified separately against the intended owning runtime.
An unavailable control socket or metadata-only source cannot establish whether
app-managed tasks are active. Installing the CLI or passing synthetic tests
does not establish that coverage. See [runtime connection diagnosis](runtime-connectivity-diagnosis.md)
for supported read-only checks and integration limits.