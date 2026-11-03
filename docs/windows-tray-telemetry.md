# Windows tray telemetry

The optional `windows-tray` feature builds a Windows-subsystem owner,
`teamy-codex-tray.exe`, and a separate console UI child,
`teamy-codex-ui.exe`. The owner starts the tray and loopback OTLP/HTTP protobuf
receiver; its child owns a separate console and inline overview. The owner
does not attach to or hide an inherited terminal. The existing
`teamy-codex telemetry` command remains the entry point for an attached
terminal and does not start a tray. The copied Windows foundation is from
`teamy-rust-cli` commit `454ab86`; tray routing and telemetry lifecycle are
Codex-specific. The child process and bounded protocol foundation are copied
from `teamy-rust-cli` commit `1d6de93`, with module paths adapted. There is no
machine-local dependency or new published crate.

The tray executable accepts `--port NUMBER`, `--events-file PATH`, and
`--log-file PATH`, each at most once. The default port is 4318; zero requests an
available port. Arguments must be Unicode. `--events-file` creates a new file
and may record commands, output and identifiers; `--log-file` uses the normal
process log behavior and can overwrite its specified file. Neither is enabled
by default. Incoming OTLP records are displayed and optionally saved, never
re-emitted into tracing or an OTLP exporter.

Show, Hide and Exit are typed tray actions. `q`, Escape at the overview, or
Hide closes the UI child while the owner and receiver continue. Show opens one
new UI child after the owner observes the previous child close. A Show request
that races with a window Close before the owner detects it can be ignored; use
Show again after closure. The owner retains up to 1,000 combined event, status and process
log records within an 8 MiB payload budget; the child replays retained records
above its inline view. The UI reports evictions and rejected oversized records;
frames are limited to 64 KiB. Acknowledging delivery does not erase history.
On tray Exit the owner stops its child and receiver; in-memory history ends with
the owner, so use a requested file sink for persistence. The intended behavior
when the child console's Close button is used is for only that child to exit;
this still requires an actual Windows host test.
The process returns a failure code on errors; `--log-file` can retain errors
after logging has initialized. Startup failures before then may not be visible
in this Windows-subsystem application.

## Later opt-in manual qualification

Do not run this while the graphics hold is active. After explicit user opt-in,
build both optional binaries without activating them:

```powershell
cargo build --offline --locked --features windows-tray --bin teamy-codex-tray --bin teamy-codex-ui
```

From the existing terminal, launch the separate application and retain its
process handle. This intentionally creates a tray icon and a new console:

```powershell
$trayExe = Join-Path (Get-Location) 'target/debug/teamy-codex-tray.exe'
$trayProcess = Start-Process -FilePath $trayExe -ArgumentList '--port','0' -WindowStyle Normal -PassThru
```

Read the assigned loopback port from the new console. Close that console with
its window Close button and confirm `$trayProcess.HasExited` is `False`; the
original terminal must remain visible. With the child closed, send a
fabricated event to the displayed port from the original PowerShell terminal:

```powershell
$testPort = 12345 # Replace with the port shown in the new console.
$name = [Text.Encoding]::UTF8.GetBytes('codex.tool_result')
$record = [byte[]](@(0x62, $name.Length) + $name)
$scope = [byte[]](@(0x12, $record.Length) + $record)
$resource = [byte[]](@(0x12, $scope.Length) + $scope)
$body = [byte[]](@(0x0a, $resource.Length) + $resource)
Invoke-WebRequest -Uri "http://127.0.0.1:${testPort}/v1/logs" -Method Post -ContentType 'application/x-protobuf' -Body $body
```

Right-click the **Teamy** notification icon, including the overflow area, and
select **Show console**. Confirm the reopened child replays the synthetic
result and its retained event count is one.
Select **Exit** and check `$trayProcess.WaitForExit(10000)` and
`$trayProcess.ExitCode` return `True` and zero; the app-owned console and tray
icon should disappear, leaving the original terminal open. This test sends
only the fabricated event and reads no Codex session data. It does not change
Codex configuration or write an event file. Process-log replay is covered
synthetically but still needs a live producer to qualify visually.

The owner and UI binaries, Close-button behavior, tray and Explorer-restart
path have not been run or visually qualified. Actual ConPTY hosts, terminal resize and recovery
after an Explorer restart remain separate tests; do not restart Explorer for
this minimal test.
