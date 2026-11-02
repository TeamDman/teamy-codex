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
raw fields; scroll with Up/Down, Page Up/Down, Home or End, and use Escape to
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

The synthetic test suite checks loopback delivery and shutdown, scrollback
insertion with Ratatui's test backend, escaped summaries, explicit loss and
truncation, process-log acknowledgement, and NDJSON writer failures. A
synthetic PTY smoke also verified a local event, counter update, detail and
page navigation, return to the overview, and `q` exit;
terminal hosts, resize behavior and failure recovery still require broader
qualification. The adjacent Teamy Rust CLI checkout contains optional building
blocks; this command still owns its application-local capture and lifecycle.
