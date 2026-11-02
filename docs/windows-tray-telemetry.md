# Windows tray telemetry

The optional `windows-tray` feature builds a separate Windows-subsystem binary,
`teamy-codex-tray.exe`. Launching it explicitly allocates a new console, starts
the tray icon, and runs the same loopback OTLP/HTTP protobuf log receiver and
inline overview. It refuses allocation if the process already has a console;
it never detaches, hides or adopts an inherited terminal. The existing
`teamy-codex telemetry` command remains the entry point for an attached
terminal and does not start a tray. The copied Windows foundation is from
`teamy-rust-cli` commit `454ab86`; tray routing and telemetry lifecycle are
Codex-specific. There is no machine-local dependency or new published crate.

The tray executable accepts `--port NUMBER`, `--events-file PATH`, and
`--log-file PATH`, each at most once. The default port is 4318; zero requests an
available port. Arguments must be Unicode. `--events-file` creates a new file
and may record commands, output and identifiers; `--log-file` uses the normal
process log behavior and can overwrite its specified file. Neither is enabled
by default. Incoming OTLP records are displayed and optionally saved, never
re-emitted into tracing or an OTLP exporter.

Show, Hide and Exit are typed tray actions. While the owned console is hidden,
process logs remain in the bounded capture and are replayed above the inline
view after Show. Eviction remains visible in the process-log count; event
summaries continue into the hidden console's scrollback, which has its own
host limits. Exit or `q` restores the console, stops the receiver and tray,
replays pending process logs, and releases the app-owned console. Forced
termination and closing the console window are not cooperative shutdown paths.
The process returns a failure code on errors; `--log-file` can retain errors
after logging has initialized. Startup failures before then may not be visible
in this Windows-subsystem application.

## Later opt-in manual qualification

Do not run this while the graphics hold is active. After explicit user opt-in,
build the optional binary without activating it:

```powershell
cargo build --offline --locked --features windows-tray --bin teamy-codex-tray
```

From the existing terminal, launch the separate application and retain its
process handle. This intentionally creates a tray icon and a new console:

```powershell
$trayExe = Join-Path (Get-Location) 'target/debug/teamy-codex-tray.exe'
$trayProcess = Start-Process -FilePath $trayExe -ArgumentList '--port','0' -WindowStyle Normal -PassThru
```

Read the assigned loopback port from the new console. Right-click the **Teamy**
notification icon, including in the overflow area, and select **Hide console**;
the original terminal must remain visible. With the console hidden, send a
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

Select **Show console** and confirm one synthetic result appears in the count.
Select **Exit** and check `$trayProcess.WaitForExit(10000)` and
`$trayProcess.ExitCode` return `True` and zero; the app-owned console and tray
icon should disappear, leaving the original terminal open. This test sends
only the fabricated event and reads no Codex session data. It does not change
Codex configuration or write an event file. Hidden process-log replay is
covered synthetically but still needs a live producer to qualify visually.

The binary, tray, console visibility and Explorer-restart path have not been
run or visually qualified. Actual ConPTY hosts, terminal resize and recovery
after an Explorer restart remain separate tests; do not restart Explorer for
this minimal test.
