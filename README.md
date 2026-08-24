# teamy-codex

`teamy-codex` is a read-only inspection CLI for Codex thread metadata and
rollout messages. It reads the Codex home directory at runtime; it does not
vendor, copy, or generate Codex user data in this repository.

## Commands

```powershell
cargo run -- thread list
cargo run -- thread list --limit 20 --all
cargo run -- thread inventory --active-since 1week --cwd-under G:\Programming\Repos --manifest .\inventory-manifest.json
cargo run -- thread message list <thread-id>

cargo run -- backup dir show
cargo run -- backup dir set D:\OneDrive\Documents\Backups\codex\teamy-codex
cargo run -- backup status
cargo run -- backup now
cargo run -- backup now --jobs 8
```

Use `--codex-home` to inspect a deliberately selected Codex state directory.
The default is `%USERPROFILE%\\.codex`.

### Auditable active-task inventory

`thread inventory` is a bounded, read-only command-and-control view of task
metadata. `--active-since` and `--manifest` are required. `--cwd-under` may be
repeated and matches a task when its working directory is lexically contained
by any supplied root. Archived tasks are excluded unless
`--include-archived` is passed. There is no implicit row limit; `--limit`
truncates only when supplied explicitly.

Rows are ordered by `last_activity_ms` descending and task ID ascending.
`last_activity_ms` is the maximum normalized nonzero timestamp across the
schema's recency, update, and creation fields; legacy values below
`100000000000` are treated as Unix seconds and converted to milliseconds. The
cutoff is inclusive. Each flat row identifies the timestamp field that won and
includes task identity/title/cwd, archive and pin state, available agent
role/path/name, model, effort, token count, and source metadata. Rollout and
message bodies are intentionally outside the v1 inventory contract.

The manifest is versioned and written atomically at the explicit path. It
records the relative-duration input and computed cutoff with provenance,
normalized cwd filters, archive/limit sources, SQLite file/schema metadata,
matched/returned/truncated counts, tool repository/worktree/build provenance,
and the SHA-256 of canonical ordered rows. An existing manifest is refused
unless `--overwrite-manifest` is supplied.

The backup commands create a normalized, durable-data export rather than a
mirror of the Codex home directory. Each thread is stored as
`sessions/<thread-id>/metadata.json` and `sessions/<thread-id>/rollout.jsonl`;
the metadata includes the archived flag. Curated global data is stored under
`global/`: `config.toml`, history/session indexes, memories, attachments, and
generated images. Runtime markers, worktrees, build artifacts, caches,
packages, plugin installations, databases, WAL files, and credentials such as
`auth.json` are intentionally excluded.

The default destination is
`D:\\OneDrive\\Documents\\Backups\\codex\\teamy-codex`; `backup dir set` stores a
different destination in the user's application configuration. `backup now`
uses a versioned manifest at the destination to copy only new or changed
session/global files and is safe to run repeatedly. The comparison uses file
size and modification time, so unchanged file contents are not reread.
Deleted sessions and global files are retained in the backup rather than
removed. Progress is written to stderr so stdout remains suitable for the
selected text, JSON, or CSV report. Before copying, the command reports the
pending item and byte totals. Runtime updates include completed/remaining
bytes, throughput, per-item time, average time per item, and ETAs calculated
independently from item time and byte rate. Large file copies report every
256 MiB, and Ctrl+C cancels an in-flight copy and removes its temporary file.
Each completed session writes its source signature into the normalized session
metadata, so a later run resumes from that durable checkpoint even when the
final aggregate manifest was not reached. Session files are copied by a bounded
worker pool; `--jobs` controls its size and defaults to the smaller of four or
the host's available parallelism.

## Output formats

All commands support the shared Teamy CLI output contract:

```powershell
cargo run -- --output-format text thread list
cargo run -- --output-format json thread list
cargo run -- --output-format csv thread list
```

When `--output-format` is omitted, interactive stdout uses colored text and
redirected stdout uses JSON. Command results are emitted through the reusable
`CliOutput` abstraction rather than each command printing directly.

## User-data safety

The repository must never contain Codex state. `.gitignore` excludes Codex
databases, SQLite sidecars, rollout files, logs, Tracy captures, and build
artifacts. The application only opens those paths when it is run locally.

Before committing, verify the worktree and ignored-data policy:

```powershell
git status --short
git check-ignore -v .codex/state_5.sqlite state_5.sqlite rollouts/example.jsonl logs/example.ndjson tracy/example.tracy
```

## Development

The project follows the current `teamy-rust-cli` conventions for build
metadata, cancellation, logging, explicit tracing features, output rendering,
nightly formatting, Clippy, and Tracy profiling. `teamy-profiler` is an
external development tool and is not vendored here.

Inventory verification evidence and its private-live-check procedure are
documented in `docs/thread-inventory-evidence.md`.

## See also

https://github.com/0xSero/ai-data-extraction/blob/main/extract_codex.py
