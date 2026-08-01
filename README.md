# teamy-codex

`teamy-codex` is a read-only inspection CLI for Codex thread metadata and
rollout messages. It reads the Codex home directory at runtime; it does not
vendor, copy, or generate Codex user data in this repository.

## Commands

```powershell
cargo run -- thread list
cargo run -- thread list --limit 20 --all
cargo run -- thread message list <thread-id>
```

Use `--codex-home` to inspect a deliberately selected Codex state directory.
The default is `%USERPROFILE%\\.codex`.

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
