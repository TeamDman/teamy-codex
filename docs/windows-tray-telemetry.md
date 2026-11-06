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
from `teamy-rust-cli` commit `d3e304a`, with module paths adapted. There is no
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
remains pending until the current child confirms it can process the request;
if it disconnects first, the owner starts at most one replacement. A child that
does not respond within two seconds is stopped on a bounded deadline. Hide and
Exit cancel a pending Show. The owner retains up to 1,000 combined event, status and process
log records within an 8 MiB payload budget; the child replays retained records
above its inline view. The UI reports evictions and rejected oversized records;
frames are limited to 64 KiB. Acknowledging delivery does not erase history.
On tray Exit the owner stops its child and receiver; in-memory history ends with
the owner, so use a requested file sink for persistence. The intended behavior
when the child console's Close button is used is for only that child to exit;
this still requires an actual Windows host test.
If child termination is requested but exit is not yet observed, the supervisor
retains the owned process handle, reports the timeout and does not start a new
child. It continues polling for exit. On owner shutdown an unresolved child is
reported; dropping the supervisor cannot guarantee the OS has stopped it.
The process returns a failure code on errors; `--log-file` can retain errors
after logging has initialized. Startup failures before then may not be visible
in this Windows-subsystem application.

## Later opt-in manual qualification

Do not run this while the graphics hold is active. After explicit user opt-in,
build both optional binaries without activating them:

```powershell
cargo build --offline --locked --features windows-tray --bin teamy-codex-tray --bin teamy-codex-ui --example send_synthetic_otel
```

The proposed smoke test lasts at most five minutes: one unelevated owner,
one app-owned console child, and an IPv4 loopback-only HTTP listener on an
OS-selected port. It creates a temporary tray icon and window. It neither
registers startup nor installs a service, creates a credential or alters
firewall or telemetry settings. The test sends only the fabricated events
below and uses no event or process-log file. The console Close, tray Show and
tray Exit actions affect the new test instance. If normal Exit does not finish,
report the remaining process and stop; do not kill unrelated processes.

From the existing terminal, launch the separate application and retain its
process handle. This intentionally creates a tray icon and a new console:

```powershell
$trayExe = Join-Path (Get-Location) 'target/debug/teamy-codex-tray.exe'
$trayProcess = Start-Process -FilePath $trayExe -ArgumentList '--port','0' -WindowStyle Normal -PassThru
```

Read the assigned loopback port from the new console. Close that console with
its window Close button and confirm `$trayProcess.HasExited` is `False`; the
original terminal must remain visible. With the child closed, send two
fabricated events to the displayed port from the original PowerShell terminal:

```powershell
$testPort = 12345 # Replace with the port shown in the new console.
& .\target\debug\examples\send_synthetic_otel.exe $testPort
```

Right-click the **Teamy** notification icon, including the overflow area, and
select **Show console**. Confirm the reopened child replays two events. Select
either with Up/Down and press Enter: detail should show the paired decision and
result, the invented `Write-Output 'synthetic'` command and an unknown-source
correlation warning. The sample's `approved` decision is fabricated; it is not
an actual authorization or a command that was executed.
Select **Exit** and check `$trayProcess.WaitForExit(10000)` and
`$trayProcess.ExitCode` return `True` and zero; the app-owned console and tray
icon should disappear, leaving the original terminal open. This test sends
only fabricated events and reads no Codex session data. It does not change
Codex configuration or write an event file. Process-log replay is covered
synthetically but still needs a live producer to qualify visually.

The owner and UI binaries, Close-button behavior, tray and Explorer-restart
path have not been run or visually qualified. Actual ConPTY hosts, terminal resize and recovery
after an Explorer restart remain separate tests; do not restart Explorer for
this minimal test.
