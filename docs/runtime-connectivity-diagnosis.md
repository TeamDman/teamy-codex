# Runtime connection diagnosis

The dashboard can observe only the runtime reached by its selected provider.
Installing the CLI or passing fixture tests does not establish coverage of
threads owned by a different desktop, remote, or app-managed runtime.

## Read-only checks

1. Confirm which Codex executable and `CODEX_HOME` the command uses. Different
   executable versions or home directories can expose different capabilities
   and metadata. A newer executable alone does not guarantee wider coverage.
2. Run `codex app-server daemon version` with the intended executable. Check the
   documented control socket, normally
   `%USERPROFILE%\.codex\app-server-control\app-server-control.sock` on Windows.
   An absent socket or connection error does not authorize changing firewall,
   security, or registration settings.
3. Verify the owning runtime's supported transport. App-server stdio is
   single-client and does not publish the Unix control socket used by the daemon
   proxy. A proxy cannot attach to an existing stdio connection.
4. Use metadata-only reads against an authorized test thread. JSON-RPC
   `-32600: thread not loaded: <UUID>` indicates that the selected server cannot
   supply that thread. It does not prove that the thread is absent elsewhere.
   Resuming a thread is not a read-only observation workaround.
5. Compare results with the owning application's supported thread API when
   available. Conventional cloud-task APIs and app-managed thread providers
   may have different coverage; a missing result or HTTP 404 in one does not
   establish absence in another.

Do not publish live thread IDs, titles, metadata inventories, account details,
or captured command output in diagnostic examples. Use synthetic fixtures and
keep private validation captures outside version control.

## Available mechanisms

| Mechanism | Coverage and prerequisites |
| --- | --- |
| App-server stdio | Its own loaded sessions and accessible persisted metadata |
| App-server daemon proxy | Requires an existing supported control socket |
| `codex --remote` / `codex agents --remote` | Requires a known authorized ws/wss/unix endpoint and any required authentication |
| Supported application thread APIs | Coverage depends on the application's provider; availability does not imply a standalone CLI transport |
| `codex cloud list` / `status` | Conventional cloud-task service, not necessarily all app-managed threads |
| Exec-server protocol | Execution/environment operations, not a global thread lifecycle inventory |
| Remote-control start/pair | Changes service/enrollment state and is not a read-only diagnostic |

Do not extract credentials, read denied session paths, guess private IPC/backend
routes, or start an unrelated daemon as a substitute for a supported provider.

## Integration requirements

To observe app-managed threads from a standalone CLI, use a supported,
authorized read-only transport or SDK belonging to the same owning provider.
Implement it behind the existing typed request/projection and verify coverage
with authorized test fixtures before claiming support.

Checking `app-server daemon version` and the documented socket can establish a
connection problem; it cannot independently establish global task coverage.
The JSON contract remains `{ data, coverage }`: PowerShell consumers use `.data`
and should retain `.coverage`.

Relevant locations in the official Codex source include:
`app-server/src/lib.rs` for transport dispatch,
`app-server/src/request_processors/thread_processor.rs` for metadata reads,
`app-server-transport/src/transport/mod.rs` for control-socket paths,
`exec-server-protocol/src/protocol.rs` for execution methods, and
`cloud-tasks-client/src/http.rs` for the conventional cloud-task API.