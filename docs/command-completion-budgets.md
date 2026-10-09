# Command completion budgets

Every output constructor requires an explicit `elapsed_duration_warn_threshold`; all parent dispatch forwards the leaf receipt. The entrypoint measures invocation and output, then warns strictly above that threshold, even on an output failure. Failed invocation has no receipt. Budget metadata never enters result serialization; `facet_rows`, custom turn statuses, and subprocess cancellation are preserved. DEBUG dispatch/completion and a command span use constant command-family labels, never user arguments.

- Backup directory show/set: 1 second for settings/path inspection.
- Backup status: 3 seconds plus 10 ms per inspected source session/global file.
- Thread list/message list: 2 seconds plus 10 ms per requested row (default 50), including formatting.
- Thread inventory: 3 seconds plus 20 ms per selected inventory row, including digest/manifest work.
- App-server thread list: 5 seconds for child startup/handshake plus 20 ms per requested page row (default 25).
- ChatGPT transcript from JSON: 3.6 seconds, comprising 2 seconds setup plus 100 ms per MiB of the existing 16 MiB input bound, plus 10 ms per selected message for formatting.
- Usage records: 5.2 seconds (2 seconds setup plus 100 ms per MiB of its existing 32 MiB input bound), plus 1 ms per input record for aggregation.
- Usage snapshots: 2 seconds plus 10 ms per returned snapshot row.
- Backup now and usage history deliberately return `None`: archival attachment/rollout scans have no aggregate byte bound. Existing count/rate/ETA tracing and cancellation remain independent.
- App-server thread start/resume deliberately return `None`: model turns have no finite completion bound.

Budgets use options, established size bounds, or workload counts, never measured elapsed time. Dynamic duration arithmetic saturates. A completion warning is not a watchdog and cannot detect a hung command before it returns.
