# Thread inventory evidence

The v1 inventory boundary is read-only task metadata. It does not parse
rollout/message bodies, infer status from prose, mutate threads, control a GUI,
send or archive anything, or run as a daemon.

## Automated fixtures

`src/inventory.rs` uses in-memory SQLite fixtures to verify:

- an inclusive activity cutoff and exclusion at one millisecond before it;
- normalization of legacy Unix-second timestamps to milliseconds;
- maximum-of-recency/update/create `last_activity_ms` semantics;
- repeatable cwd-root OR filtering with path-component boundaries;
- archived exclusion by default and explicit inclusion;
- pin and agent role/path/name projection where columns exist;
- activity-descending/task-ID-ascending deterministic ties;
- explicit-only truncation counts; and
- identical semantic row SHA-256 digests for two unchanged queries.

## Private live cross-check

Machine-local captures belong under `local-evidence/`, which is ignored by
Git. The cross-check procedure is:

1. Run `thread inventory` against the live Codex state with an explicit
   lookback, cwd root, JSON output capture, and manifest path.
2. Run an independent `sqlite3 -readonly` query against `state_5.sqlite` using
   the documented timestamp maximum, inclusive cutoff, archive predicate, cwd
   component boundary, and deterministic ordering.
3. Compare returned task IDs/order and counts without copying titles, messages,
   rollout bodies, or the private captures into repository evidence.
4. Run the unchanged inventory a second time and compare only the manifest's
   canonical semantic row digest.

The repository gates are `check-all.ps1`: nightly formatting, strict Clippy,
all-feature build, and all-feature tests. `git diff --check` is also run before
handoff.

## 2026-08-23 validation

An ignored, SQLite-consistent snapshot of the live state was captured through
a read-only source connection. The tool returned 199 rows. A separately
authored `sqlite3 -readonly` query returned the same 199 task IDs and normalized
activity values in the same order, with zero differences. Two unchanged tool
runs produced the same semantic row digest.

The existing-manifest refusal returned a failure and preserved the destination
byte-for-byte. Explicit overwrite then succeeded through the Windows
same-volume atomic replacement path. An explicit `--limit 2` CSV run recorded
199 matched, 2 returned, and 197 truncated rows, and emitted two flat CSV rows.
All private database, row, title, ID, and manifest captures remain only under
the ignored `local-evidence/` directory.
