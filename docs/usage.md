# Usage queries

Public Rust SDK: `teamy_codex::usage`. All sources are on-demand and read-only.
No observer, credential handling, uploads or thread changes are involved.

## Native history (default local Codex storage)

```powershell
.\target\debug\teamy-codex.exe --output-format json usage history --thread <thread-id> --group-by model --group-by effort
.\target\debug\teamy-codex.exe usage history --thread <id> --model gpt-6-astra --since 2026-10-01T00:00:00Z --until 2026-10-08T00:00:00Z --group-by day
```

`--codex-home` selects the local storage root; the default is the current user's
`.codex`. Thread paths and counters are read from SQLite with READ_ONLY and
query_only. Historical prefix files are resolved by rollout IDs from filename
inventory under sessions/archived_sessions, including suffixed rollout IDs used
after revert. The reader follows explicit history_base byte and ordinal cutoffs,
with cycle/depth guards. It does not guess the latest file or scan other files'
conversation bodies. Canonical paths must remain under those storage roots.

Scope is the currently selected logical history chain, including only responses
owned by the queried thread. Reverted-away tails and descendant/fork response
usage are excluded. They are not silently treated as current-thread consumption.
An absent requested thread is listed in missing_threads.

Each file is streamed up to its length captured before reading. Lines are bounded
to 32 MiB; oversized lines are discarded with diagnostics. Concurrent append
cannot extend this query indefinitely. An incomplete trailing metadata record is
reported and skipped. Corrupt metadata, inaccessible/missing history bases,
truncated prefixes and unsupported compressed files are reported as coverage
diagnostics. No raw payload or private parser excerpt is printed in errors.

Modern token_usage_record events supply discrete token usage and response IDs.
IDs are globally deduplicated before filtering. Conflicting usage/ownership
fails; conflicting replay attribution becomes an unknown-model/effort bucket.
Wrapper replay timestamps do not create a second response. Per-response explicit
model/effort fields are preferred if present, otherwise a preceding same-turn
turn_context identifies the persisted step settings. This is not inference from
a thread's current/default model. Provider-side substitutions not recorded in the
log remain unobservable.

For older logs lacking response records in a turn, cumulative TokenCount deltas
are accepted only when all counters increase consistently and the delta equals
the recorded last usage. The first event needs total=last; an unexplained initial
baseline is not allocated. Repeated snapshots are ignored. Counter decreases
start a new epoch without assigning the reset itself. Compaction/context
estimates cannot pass request-breakdown validation. Modern response records
suppress fallback deltas in their turns to prevent double-counting. Legacy
results are separately counted and use counter-observation timestamps rather
than provider completion timestamps.

Unknown model/effort are null buckets. Missing/invalid completion time cannot be
filtered or grouped by date, so those records and token totals are reported
separately as untimed. Source diagnostics and per-thread reconciliation compare
unfiltered stored responses (including untimed records) with the database counter
sampled at query start and the latest scanned log counter. The signed difference
is preserved; it is never forced to zero. Native counters may have different
accounting semantics or histories. JSON schema version 1 records all these limits.

SDK: `history::query_history`, `history::query_history_cancellable`,
`HistoryReport`, `HistorySource`, `ThreadUsageReconciliation`.
CLI cancellation stops only this reader and never sends turn/interrupt.

## Native snapshots

```powershell
.\target\debug\teamy-codex.exe --output-format json usage snapshots --thread <id>
```

Snapshots return the latest native cumulative counter per thread, including
archived rows with no implicit limit. Missing threads yield an empty result.
Latest model/effort labels do not attribute the counter. Historical model/time
filters and non-thread grouping fail explicitly; use history instead. No
cross-thread snapshot grand total is returned because inherited counters overlap.

## Explicit request exports (optional)

```powershell
.\target\debug\teamy-codex.exe usage records --input usage-export.json --thread <thread-id> --group-by model --group-by effort
```

Records remains available for explicitly supplied versioned exports. Native
history no longer requires this intermediate file. Export fields below are
required; model/effort may be null:

```json
{
  "schema_version": 1,
  "accounting": "response-delta",
  "provenance": "identify the explicit provider/request export source",
  "coverage": "describe thread/time scope and omissions",
  "records": [{
    "response_id": "unique-upstream-response-id",
    "thread_id": "originating-thread-id",
    "completed_at": "2026-10-07T18:00:00Z",
    "effective_model": "gpt-6-astra",
    "effective_effort": "medium",
    "tokens": {
      "input_tokens": 100,
      "cached_input_tokens": 80,
      "output_tokens": 20,
      "reasoning_output_tokens": 10,
      "total_tokens": 120
    }
  }]
}
```

SDK: `UsageQuery`, `UsageGroup`, `records::UsageExport`, `UsageRecord`,
`UsageTokens`, `UsageReport`, `query_usage_records`, `query_usage_file`;
`snapshots::SnapshotReport` and `query_snapshots`.

## Shared filters and totals

Repeated thread/model filters are ORed within each dimension and ANDed across
dimensions. Model matches are exact and case-sensitive; aliases are not resolved.
Times require RFC3339 with explicit offsets. Since is inclusive and until
exclusive; day grouping uses UTC. Group by thread/model/effort/day together.
Null keys in selected dimensions mean unknown. Model filters exclude unknown
attribution and report the otherwise thread/time-matching excluded count.

Cached input is already a subset of input and reasoning output a subset of
output; neither is added again to total. Nonnegative counts, subset relationships,
total=input+output, and checked aggregation are enforced. Reports include
read/unique/duplicate/matched counts, unknown attribution, totals and buckets.
Empty matched totals do not prove zero actual usage. JSON/text preserve scope
and nested diagnostics; CSV is rejected. Export input is limited to 32 MiB.
