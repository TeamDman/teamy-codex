# Terminal telemetry receiver

`teamy-codex telemetry` explicitly starts an OTLP/HTTP **protobuf logs** receiver
on IPv4 loopback and an inline Ratatui view on stderr. It requires an
interactive stdin and stderr. The address is shown in the view; `--port 0`
selects an available local port, otherwise the default is 4318. Press `q`,
Escape at the overview, or Ctrl+C to stop. The command does not alter Codex configuration,
start Codex or enable telemetry elsewhere.

Received event summaries and ordinary process logs are printed above the view.
Untrusted control and bidirectional formatting characters are escaped. Lines
that do not fit the terminal width are clipped with an ellipsis. Use Up/Down to
select a retained event and Enter to inspect its escaped arguments, output and
raw fields. For a recognized `exec_command` argument object, the detail view
also renders the command, explicitly named shell and working directory; an
omitted shell is shown as unknown or implicit. The original argument text and
complete admitted record remain available. The view reports a sender-declared
truncation but cannot establish complete raw process output. Scroll with
Up/Down, Page Up/Down, Home or End, and use Escape to
return. This only shows what the sender delivered, including any upstream
truncation; it does not execute commands. An evicted selection is identified
explicitly. Counts distinguish accepted records, retained
records, evictions, rejections, records missed before display, upstream output
truncation and evicted process logs. Decisions/results are counted when the
view processes them, so high-volume loss can make those counts incomplete.
Received records are not re-emitted into `tracing` or any OTLP exporter.

The in-memory receiver uses the `teamy-tracing` default of 1,000 records and
8 MiB of logical record size, with a 1 MiB request body limit. Logical size is
not a strict process-memory limit. Records evicted before processing are not
recoverable, and the sender may lose events while the listener is down. The
view drains received records after shutdown before printing final totals.
There is no persistent event file unless `--events-file PATH` is provided; it
creates a new NDJSON file and refuses to overwrite an existing file. It writes
only records processed before eviction and can contain commands, output,
identifiers and other sensitive data. It is separate from the existing
`--log-file` process diagnostics. No automatic redaction or rotation is supplied.

The receiver and Ratatui integration are enabled by the default `telemetry`
feature and can be omitted with `--no-default-features`. No service, tray,
global hotkey or console visibility handling is installed. Do not configure
real Codex telemetry or perform a live capture without specific approval; use
synthetic data and an isolated temporary configuration for later qualification.

## Proposed isolated Codex qualification

This is a test plan, not an instruction to change the current configuration.
It requires specific approval before a real Codex process or its telemetry is
started. First qualify the tray Close/Show/Exit lifecycle with the fabricated
event in [the tray guide](windows-tray-telemetry.md). For a separate Codex
test, run one new `codex exec` session with `--ephemeral` and
`--ignore-user-config`, using only process-scoped overrides for OTLP/HTTP
binary logs at the selected IPv4 loopback port. Explicitly leave prompt,
agent-response and guardian-assessment content logging off and do not opt in
to trace or metrics export. Verify the installed version accepts each override
before relying on it; administrator requirements can still apply.

Use an invented request asking for one harmless PowerShell `Write-Output` call
and no file inspection. The model may decline or choose a different action, so
the actual tool invocation and approval remain observable test results rather
than assumptions. The installed CLI's `--ignore-user-config` still uses its
configured authentication and `--ephemeral` only promises not to persist the
session files; neither flag proves that every cache, log, network request or
environment variable is isolated. The test may contact the model provider.

Even with raw prompt logging disabled, exported records can contain command
arguments, output previews, conversation identifiers, model and runtime
metadata, and other tool fields. The default sender output limit is a preview,
not a guarantee of complete stdout. Do not select `--events-file` or
`--log-file` for this test: retained events live in bounded process memory,
and the terminal or its host may retain rendered text in scrollback. The
loopback endpoint accepts local senders that can reach its selected port; it
is not an authentication boundary. Stop the test process and receiver, then
report observed events, loss/truncation and cleanup without copying sensitive
records into a report. Do not inspect existing private Codex sessions or alter
the user's persistent telemetry configuration.

The synthetic test suite checks loopback delivery and shutdown, scrollback
insertion with Ratatui's test backend, escaped summaries, explicit loss and
truncation, process-log acknowledgement, and NDJSON writer failures. A
synthetic PTY smoke also verified a local event, counter update, detail and
page navigation, return to the overview, and `q` exit;
terminal hosts, resize behavior and failure recovery still require broader
qualification. Inline ownership and escaped scrollback are copied from Teamy
Rust CLI commit `9cbff90`, following the template-copy workflow. The process-log
capture and telemetry-specific screen/navigation remain local deviations; the
application owns the subscriber, sink and receiver lifecycle. The copied guard
rejects a second in-process owner and refuses reuse after a screen restoration
failure; it cannot coordinate external processes or unrelated terminal writers.

An optional, explicitly launched Windows tray executable is described in
[Windows tray telemetry](windows-tray-telemetry.md). It has not been activated
or visually qualified. The normal `telemetry` command does not start it or
change a shared terminal's visibility.
