//! Minimal, owned JSON-RPC transport for `codex app-server` over stdio.
//!
//! This module deliberately does not depend on the community Codex SDK. It
//! implements bounded thread/list, start/resume, and one-turn operations.

use eyre::{Context, ContextCompat, bail};
use facet::Facet;
use facet_json::RawJson;
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PROMPT_BYTES: usize = 32 * 1024;
pub const MAX_CWD_BYTES: usize = 4096;
pub const MAX_THREAD_ID_BYTES: usize = 256;
const MAX_FINAL_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_MESSAGES_PER_PHASE: u32 = 10_000;
pub const MAX_THREAD_LIST_LIMIT: u32 = 100;

#[derive(Facet)]
struct ClientInfo<'a> {
    name: &'a str,
    title: &'a str,
    version: &'a str,
}

#[derive(Facet)]
struct InitializeParams<'a> {
    #[facet(rename = "clientInfo")]
    client_info: ClientInfo<'a>,
}

#[derive(Facet)]
struct InitializeRequest<'a> {
    id: u64,
    method: &'a str,
    params: InitializeParams<'a>,
}

#[derive(Facet)]
struct ThreadListParams<'a> {
    limit: u32,
    cursor: Option<&'a str>,
    #[facet(rename = "sourceKinds")]
    source_kinds: Vec<&'a str>,
    #[facet(rename = "useStateDbOnly")]
    use_state_db_only: bool,
}

#[derive(Facet)]
struct ThreadListRequest<'a> {
    id: u64,
    method: &'a str,
    params: ThreadListParams<'a>,
}

#[derive(Facet)]
struct ThreadStartParams<'a> {
    cwd: &'a str,
    ephemeral: bool,
    sandbox: &'a str,
    #[facet(rename = "serviceName")]
    service_name: &'a str,
}

#[derive(Facet)]
struct ThreadStartRequest<'a> {
    id: u64,
    method: &'a str,
    params: ThreadStartParams<'a>,
}

#[derive(Facet)]
struct ThreadResumeParams<'a> {
    #[facet(rename = "threadId")]
    thread_id: &'a str,
}

#[derive(Facet)]
struct ThreadResumeRequest<'a> {
    id: u64,
    method: &'a str,
    params: ThreadResumeParams<'a>,
}

#[derive(Facet)]
struct TextInput<'a> {
    #[facet(rename = "type")]
    kind: &'a str,
    text: &'a str,
}

#[derive(Facet)]
struct ReadOnlySandboxPolicy<'a> {
    #[facet(rename = "type")]
    kind: &'a str,
    #[facet(rename = "networkAccess")]
    network_access: bool,
}

#[derive(Facet)]
struct TurnStartParams<'a> {
    #[facet(rename = "threadId")]
    thread_id: &'a str,
    input: Vec<TextInput<'a>>,
    #[facet(rename = "approvalPolicy")]
    approval_policy: &'a str,
    #[facet(rename = "sandboxPolicy")]
    sandbox_policy: ReadOnlySandboxPolicy<'a>,
}

#[derive(Facet)]
struct TurnStartRequest<'a> {
    id: u64,
    method: &'a str,
    params: TurnStartParams<'a>,
}

#[derive(Facet)]
struct TurnInterruptParams<'a> {
    #[facet(rename = "threadId")]
    thread_id: &'a str,
    #[facet(rename = "turnId")]
    turn_id: &'a str,
}

#[derive(Facet)]
struct TurnInterruptRequest<'a> {
    id: u64,
    method: &'a str,
    params: TurnInterruptParams<'a>,
}

#[derive(Facet)]
struct ThreadResponse {
    thread: ThreadHandle,
}

/// A persisted thread created or resumed through the app-server protocol.
#[derive(Facet, Debug)]
pub struct ThreadHandle {
    pub id: String,
    #[facet(rename = "sessionId")]
    pub session_id: Option<String>,
    pub ephemeral: bool,
}

#[derive(Facet)]
struct TurnStartResponse {
    turn: TurnHead,
}

#[derive(Facet)]
struct TurnHead {
    id: String,
    status: String,
}

/// Terminal status from the authoritative `turn/completed` event.
#[derive(Facet, Clone, Copy, Debug, Eq, PartialEq)]
#[facet(rename_all = "camelCase")]
#[repr(u8)]
pub enum TurnFinalStatus {
    Completed,
    Interrupted,
    Failed,
}

/// Bounded, typed result of one Codex turn.
#[derive(Facet, Debug)]
pub struct TurnOutcome {
    pub thread_id: String,
    pub turn_id: String,
    pub status: TurnFinalStatus,
    pub error_message: Option<String>,
    pub final_message: Option<String>,
    pub events_seen: u32,
    pub requests_declined: u32,
}

/// A single server-owned page of persisted Codex threads.
#[derive(Facet, Debug)]
pub struct ThreadListPage {
    pub data: Vec<AppServerThread>,
    #[facet(rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

/// Stable fields projected from an app-server `thread/list` row.
#[derive(Facet, Debug)]
pub struct AppServerThread {
    pub id: String,
    pub name: Option<String>,
    pub preview: Option<String>,
    pub cwd: Option<String>,
    #[facet(rename = "createdAt")]
    pub created_at: Option<i64>,
    #[facet(rename = "updatedAt")]
    pub updated_at: Option<i64>,
    #[facet(rename = "isPinned")]
    pub is_pinned: Option<bool>,
}

#[derive(Facet)]
struct InitializedNotification<'a> {
    method: &'a str,
    params: EmptyParams,
}

#[derive(Facet)]
struct EmptyParams {}

#[derive(Facet)]
struct RpcError<'a> {
    code: i32,
    message: &'a str,
}

#[derive(Facet)]
struct RpcErrorResponse<'a> {
    id: RawJson<'a>,
    error: RpcError<'a>,
}

#[derive(Facet)]
struct DecisionResult<'a> {
    decision: &'a str,
}

#[derive(Facet)]
struct RpcDecisionResponse<'a> {
    id: RawJson<'a>,
    result: DecisionResult<'a>,
}

#[derive(Facet)]
struct PermissionDeniedResult {
    permissions: EmptyParams,
}

#[derive(Facet)]
struct RpcPermissionDeniedResponse<'a> {
    id: RawJson<'a>,
    result: PermissionDeniedResult,
}

#[derive(Facet)]
struct ElicitationDeniedResult<'a> {
    action: &'a str,
    content: Option<String>,
}

#[derive(Facet)]
struct RpcElicitationDeniedResponse<'a> {
    id: RawJson<'a>,
    result: ElicitationDeniedResult<'a>,
}

#[derive(Facet)]
struct RpcEnvelope<'a> {
    id: Option<RawJson<'a>>,
    method: Option<String>,
    params: Option<RawJson<'a>>,
    result: Option<RawJson<'a>>,
    error: Option<RawJson<'a>>,
}

#[derive(Facet)]
struct RequestScope {
    #[facet(rename = "threadId")]
    thread_id: String,
    #[facet(rename = "turnId")]
    turn_id: Option<String>,
}

#[derive(Facet)]
struct TurnCompletedNotification {
    #[facet(rename = "threadId")]
    thread_id: String,
    turn: CompletedTurn,
}

#[derive(Facet)]
struct CompletedTurn {
    id: String,
    status: String,
    error: Option<TurnError>,
}

#[derive(Facet)]
struct TurnError {
    message: String,
}

#[derive(Facet)]
struct ItemCompletedNotification {
    #[facet(rename = "threadId")]
    thread_id: String,
    #[facet(rename = "turnId")]
    turn_id: String,
    item: CompletedItem,
}

#[derive(Facet)]
struct CompletedItem {
    #[facet(rename = "type")]
    kind: String,
    text: Option<String>,
    phase: Option<String>,
}

struct TurnAccumulator {
    thread_id: String,
    turn_id: Option<String>,
    completion: Option<CompletedTurn>,
    last_message: Option<(String, String, bool)>,
    events_seen: u32,
    requests_declined: u32,
}

impl TurnAccumulator {
    fn new(thread_id: &str) -> Self {
        Self {
            thread_id: thread_id.to_owned(),
            turn_id: None,
            completion: None,
            last_message: None,
            events_seen: 0,
            requests_declined: 0,
        }
    }

    fn bind_turn_id(&mut self, turn_id: &str) -> eyre::Result<()> {
        self.turn_id = Some(turn_id.to_owned());
        if self
            .completion
            .as_ref()
            .is_some_and(|completion| completion.id != turn_id)
        {
            bail!("turn/completed referred to a different turn");
        }
        Ok(())
    }

    fn observe(&mut self, method: &str, params: Option<&RawJson<'_>>) -> eyre::Result<()> {
        self.events_seen = self
            .events_seen
            .checked_add(1)
            .context("app-server event count overflow")?;
        if self.events_seen > MAX_MESSAGES_PER_PHASE {
            bail!("app-server turn emitted too many events");
        }
        match method {
            "turn/completed" => {
                let params = params.context("turn/completed omitted params")?;
                let event: TurnCompletedNotification = facet_json::from_str(params.as_str())
                    .wrap_err("invalid turn/completed event")?;
                if event.thread_id == self.thread_id {
                    if self.completion.is_some() {
                        bail!("duplicate turn/completed event");
                    }
                    if self
                        .turn_id
                        .as_ref()
                        .is_some_and(|turn_id| *turn_id != event.turn.id)
                    {
                        bail!("turn/completed referred to a different turn");
                    }
                    self.completion = Some(event.turn);
                }
            }
            "item/completed" => {
                let params = params.context("item/completed omitted params")?;
                let event: ItemCompletedNotification = facet_json::from_str(params.as_str())
                    .wrap_err("invalid item/completed event")?;
                if event.thread_id == self.thread_id
                    && event.item.kind == "agentMessage"
                    && let Some(text) = event.item.text
                {
                    if text.len() > MAX_FINAL_MESSAGE_BYTES {
                        bail!("agent final message exceeds size limit");
                    }
                    let is_final = event.item.phase.as_deref() == Some("final_answer");
                    if is_final
                        || event.item.phase.is_none()
                            && !self
                                .last_message
                                .as_ref()
                                .is_some_and(|(_, _, was_final)| *was_final)
                    {
                        self.last_message = Some((event.turn_id, text, is_final));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn into_outcome(self) -> eyre::Result<TurnOutcome> {
        let turn_id = self
            .turn_id
            .context("turn/start did not return a turn id")?;
        let completion = self.completion.context("turn/completed was not received")?;
        let status = match completion.status.as_str() {
            "completed" => TurnFinalStatus::Completed,
            "interrupted" => TurnFinalStatus::Interrupted,
            "failed" => TurnFinalStatus::Failed,
            other => bail!("invalid terminal turn status: {other}"),
        };
        let error_message = completion.error.map(|error| error.message);
        if status == TurnFinalStatus::Failed && error_message.is_none() {
            bail!("failed turn omitted error details");
        }
        let final_message = self
            .last_message
            .and_then(|(message_turn_id, text, _)| (message_turn_id == turn_id).then_some(text));
        Ok(TurnOutcome {
            thread_id: self.thread_id,
            turn_id,
            status,
            error_message,
            final_message,
            events_seen: self.events_seen,
            requests_declined: self.requests_declined,
        })
    }
}

/// Synchronous JSONL adapter. The caller owns the child process and its
/// lifetime; this type never spawns a process or grants server requests.
pub struct AppServerStdio<R, W> {
    reader: R,
    writer: W,
    next_id: u64,
}

impl<R, W> std::fmt::Debug for AppServerStdio<R, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppServerStdio")
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl<R: BufRead, W: Write> AppServerStdio<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            next_id: 1,
        }
    }

    /// Perform the app-server handshake, including the `initialized` event.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, rejected, oversized, or interrupted
    /// protocol messages, or if either stdio stream fails.
    pub fn initialize(&mut self) -> eyre::Result<()> {
        let id = self.allocate_id()?;
        self.send(&InitializeRequest {
            id,
            method: "initialize",
            params: InitializeParams {
                client_info: ClientInfo {
                    name: "teamy-codex",
                    title: "Teamy Codex",
                    version: env!("CARGO_PKG_VERSION"),
                },
            },
        })?;
        self.await_response(id, None)?;
        self.send(&InitializedNotification {
            method: "initialized",
            params: EmptyParams {},
        })
    }

    /// List one page without asking the server to scan and repair session
    /// metadata. This never creates or resumes a task.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid limit, malformed or rejected protocol
    /// messages, or if either stdio stream fails.
    pub fn list_threads(
        &mut self,
        limit: u32,
        cursor: Option<&str>,
    ) -> eyre::Result<ThreadListPage> {
        if !(1..=MAX_THREAD_LIST_LIMIT).contains(&limit) {
            bail!("thread/list limit must be between 1 and {MAX_THREAD_LIST_LIMIT}");
        }
        if cursor.is_some_and(|value| value.len() > 8192) {
            bail!("thread/list cursor exceeds size limit");
        }
        let id = self.allocate_id()?;
        self.send(&ThreadListRequest {
            id,
            method: "thread/list",
            params: ThreadListParams {
                limit,
                cursor,
                source_kinds: vec!["cli", "vscode", "appServer"],
                use_state_db_only: true,
            },
        })?;
        let result = self.await_response(id, None)?;
        let page: ThreadListPage =
            facet_json::from_str(&result).wrap_err("invalid thread/list result")?;
        if page.data.len() > limit as usize {
            bail!("thread/list result exceeded requested limit");
        }
        Ok(page)
    }

    /// Create a persisted thread with a read-only sandbox.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid working directory or protocol failure.
    pub fn start_thread(&mut self, cwd: &str) -> eyre::Result<ThreadHandle> {
        if cwd.len() > MAX_CWD_BYTES || !Path::new(cwd).is_absolute() {
            bail!("thread/start cwd must be an absolute path of at most {MAX_CWD_BYTES} bytes");
        }
        let id = self.allocate_id()?;
        self.send(&ThreadStartRequest {
            id,
            method: "thread/start",
            params: ThreadStartParams {
                cwd,
                ephemeral: false,
                sandbox: "read-only",
                service_name: "teamy-codex",
            },
        })?;
        let result = self.await_response(id, None)?;
        let response: ThreadResponse =
            facet_json::from_str(&result).wrap_err("invalid thread/start result")?;
        if response.thread.id.is_empty() || response.thread.ephemeral {
            bail!("thread/start did not return a persisted thread id");
        }
        Ok(response.thread)
    }

    /// Resume a persisted thread by id without starting a turn.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid id or protocol failure.
    pub fn resume_thread(&mut self, thread_id: &str) -> eyre::Result<ThreadHandle> {
        if thread_id.is_empty() || thread_id.len() > MAX_THREAD_ID_BYTES {
            bail!("thread/resume id must contain 1 to {MAX_THREAD_ID_BYTES} bytes");
        }
        let id = self.allocate_id()?;
        self.send(&ThreadResumeRequest {
            id,
            method: "thread/resume",
            params: ThreadResumeParams { thread_id },
        })?;
        let result = self.await_response(id, None)?;
        let response: ThreadResponse =
            facet_json::from_str(&result).wrap_err("invalid thread/resume result")?;
        if response.thread.id != thread_id || response.thread.ephemeral {
            bail!("thread/resume returned a different or ephemeral thread");
        }
        Ok(response.thread)
    }

    /// Start one text turn under read-only sandboxing and drain until the
    /// authoritative `turn/completed` notification arrives.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid input, protocol failure, or premature EOF.
    pub fn run_turn(&mut self, thread: &ThreadHandle, prompt: &str) -> eyre::Result<TurnOutcome> {
        self.run_turn_with_started(thread, prompt, &AtomicBool::new(false), |_, _, _| {})
    }

    pub(crate) fn run_turn_with_started<F: FnOnce(&str, &str, u64)>(
        &mut self,
        thread: &ThreadHandle,
        prompt: &str,
        interrupt_requested: &AtomicBool,
        on_started: F,
    ) -> eyre::Result<TurnOutcome> {
        if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            bail!("turn/start prompt must contain 1 to {MAX_PROMPT_BYTES} bytes");
        }
        let id = self.allocate_id()?;
        self.send(&TurnStartRequest {
            id,
            method: "turn/start",
            params: TurnStartParams {
                thread_id: &thread.id,
                input: vec![TextInput {
                    kind: "text",
                    text: prompt,
                }],
                approval_policy: "on-request",
                sandbox_policy: ReadOnlySandboxPolicy {
                    kind: "readOnly",
                    network_access: false,
                },
            },
        })?;
        let mut events = TurnAccumulator::new(&thread.id);
        let result = self.await_response(id, Some(&mut events))?;
        let response: TurnStartResponse =
            facet_json::from_str(&result).wrap_err("invalid turn/start result")?;
        if response.turn.id.is_empty() || response.turn.status != "inProgress" {
            bail!("turn/start did not return an in-progress turn id");
        }
        events.bind_turn_id(&response.turn.id)?;
        let interrupt_request_id = self.allocate_id()?;
        on_started(&thread.id, &response.turn.id, interrupt_request_id);
        self.drain_turn(&mut events, interrupt_requested, interrupt_request_id)?;
        events.into_outcome()
    }

    fn allocate_id(&mut self) -> eyre::Result<u64> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).context("JSON-RPC request id exhausted")?;
        Ok(id)
    }

    fn send<'a, T: Facet<'a>>(&mut self, value: &'a T) -> eyre::Result<()> {
        write_json_line(&mut self.writer, value)
    }

    fn await_response(
        &mut self,
        expected_id: u64,
        mut events: Option<&mut TurnAccumulator>,
    ) -> eyre::Result<String> {
        for _ in 0..MAX_MESSAGES_PER_PHASE {
            let line = self.read_line()?;
            let envelope: RpcEnvelope<'_> =
                facet_json::from_str_borrowed(&line).wrap_err("invalid JSON-RPC envelope")?;
            let Some(raw_id) = envelope.id.as_ref() else {
                let method = envelope
                    .method
                    .as_deref()
                    .context("JSON-RPC message omitted id and method")?;
                if let Some(state) = events.as_deref_mut() {
                    state.observe(method, envelope.params.as_ref())?;
                }
                continue;
            };
            if let Some(method) = envelope.method.as_deref() {
                self.answer_server_request(
                    raw_id,
                    method,
                    envelope.params.as_ref(),
                    events.as_deref_mut(),
                )?;
                continue;
            }
            let id: u64 = facet_json::from_str(raw_id.as_str())
                .wrap_err("response id was not an unsigned integer")?;
            if id != expected_id {
                bail!("unexpected JSON-RPC response id {id}; expected {expected_id}");
            }
            if let Some(error) = envelope.error {
                bail!("app-server rejected request {id}: {}", error.as_str());
            }
            return envelope
                .result
                .map(|result| result.as_str().to_owned())
                .context("JSON-RPC response omitted result");
        }
        bail!("app-server sent too many messages before a response");
    }

    fn drain_turn(
        &mut self,
        events: &mut TurnAccumulator,
        interrupt_requested: &AtomicBool,
        interrupt_request_id: u64,
    ) -> eyre::Result<()> {
        let mut interrupt_acknowledged = false;
        for _ in 0..MAX_MESSAGES_PER_PHASE {
            if events.completion.is_some() {
                return Ok(());
            }
            let line = self.read_line()?;
            let envelope: RpcEnvelope<'_> =
                facet_json::from_str_borrowed(&line).wrap_err("invalid JSON-RPC envelope")?;
            match (envelope.id.as_ref(), envelope.method.as_deref()) {
                (Some(id), Some(method)) => {
                    self.answer_server_request(id, method, envelope.params.as_ref(), Some(events))?;
                }
                (None, Some(method)) => events.observe(method, envelope.params.as_ref())?,
                (Some(id), None) => {
                    let response_id: u64 = facet_json::from_str(id.as_str())
                        .wrap_err("response id was not an unsigned integer")?;
                    if response_id != interrupt_request_id
                        || !interrupt_requested.load(Ordering::Acquire)
                        || interrupt_acknowledged
                    {
                        bail!("unexpected JSON-RPC response during turn");
                    }
                    if let Some(error) = envelope.error {
                        bail!("app-server rejected turn/interrupt: {}", error.as_str());
                    }
                    let result = envelope
                        .result
                        .context("turn/interrupt response omitted result")?;
                    let _: EmptyParams = facet_json::from_str(result.as_str())
                        .wrap_err("invalid turn/interrupt result")?;
                    interrupt_acknowledged = true;
                }
                (None, None) => bail!("JSON-RPC message omitted id and method"),
            }
        }
        bail!("app-server turn emitted too many messages");
    }

    fn answer_server_request(
        &mut self,
        id: &RawJson<'_>,
        method: &str,
        params: Option<&RawJson<'_>>,
        events: Option<&mut TurnAccumulator>,
    ) -> eyre::Result<()> {
        let Some(state) = events else {
            return self.reject_server_request(id);
        };

        let known_denial = matches!(
            method,
            "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/permissions/requestApproval"
                | "mcpServer/elicitation/request"
        );
        if known_denial {
            let params = params.context("approval request omitted params")?;
            let scope: RequestScope =
                facet_json::from_str(params.as_str()).wrap_err("invalid approval request scope")?;
            if scope.thread_id != state.thread_id
                || state
                    .turn_id
                    .as_ref()
                    .is_some_and(|turn_id| scope.turn_id.as_ref() != Some(turn_id))
            {
                bail!("approval request referred to a different thread or turn");
            }
            if method != "mcpServer/elicitation/request" && scope.turn_id.is_none() {
                bail!("approval request omitted turnId");
            }
        }

        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                self.send(&RpcDecisionResponse {
                    id: id.clone(),
                    result: DecisionResult {
                        decision: "decline",
                    },
                })?;
            }
            "item/permissions/requestApproval" => {
                self.send(&RpcPermissionDeniedResponse {
                    id: id.clone(),
                    result: PermissionDeniedResult {
                        permissions: EmptyParams {},
                    },
                })?;
            }
            "mcpServer/elicitation/request" => {
                self.send(&RpcElicitationDeniedResponse {
                    id: id.clone(),
                    result: ElicitationDeniedResult {
                        action: "decline",
                        content: None,
                    },
                })?;
            }
            _ => self.reject_server_request(id)?,
        }
        state.requests_declined = state
            .requests_declined
            .checked_add(1)
            .context("app-server request count overflow")?;
        Ok(())
    }

    fn reject_server_request(&mut self, id: &RawJson<'_>) -> eyre::Result<()> {
        self.send(&RpcErrorResponse {
            id: id.clone(),
            error: RpcError {
                code: -32601,
                message: "unsupported app-server request",
            },
        })
    }

    fn read_line(&mut self) -> eyre::Result<String> {
        let mut line = Vec::new();
        let mut bounded = (&mut self.reader).take((MAX_MESSAGE_BYTES + 1) as u64);
        bounded.read_until(b'\n', &mut line)?;
        if line.len() > MAX_MESSAGE_BYTES {
            bail!("incoming JSON-RPC message exceeds size limit");
        }
        if line.last() != Some(&b'\n') {
            bail!("app-server closed stdout before completing a JSON-RPC message");
        }
        String::from_utf8(line).wrap_err("app-server sent non-UTF-8 JSON-RPC")
    }
}

/// Send one cancellation request from the process owner while the protocol
/// reader is blocked on turn events. The shared writer must keep each JSONL
/// frame atomic across threads.
pub(crate) fn send_turn_interrupt<W: Write>(
    writer: &mut W,
    request_id: u64,
    thread_id: &str,
    turn_id: &str,
) -> eyre::Result<()> {
    write_json_line(
        writer,
        &TurnInterruptRequest {
            id: request_id,
            method: "turn/interrupt",
            params: TurnInterruptParams { thread_id, turn_id },
        },
    )
}

fn write_json_line<'a, T: Facet<'a>, W: Write>(writer: &mut W, value: &'a T) -> eyre::Result<()> {
    let mut line = facet_json::to_vec(value).wrap_err("could not serialize JSON-RPC message")?;
    if line.len() > MAX_MESSAGE_BYTES {
        bail!("outgoing JSON-RPC message exceeds size limit");
    }
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AppServerStdio, TurnFinalStatus};
    use std::io::{BufReader, Cursor};

    #[test]
    fn handshake_and_list_tolerate_notifications_and_deny_server_requests() {
        let input = concat!(
            "{\"method\":\"server/notice\",\"params\":{}}\n",
            "{\"id\":1,\"result\":{\"userAgent\":\"test\"}}\n",
            "{\"id\":99,\"method\":\"item/commandExecution/requestApproval\",\"params\":{}}\n",
            "{\"id\":2,\"result\":{\"data\":[]}}\n"
        );
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        client.initialize().unwrap();
        assert!(
            client
                .list_threads(3, Some("opaque-cursor"))
                .unwrap()
                .data
                .is_empty()
        );
        let output = String::from_utf8(client.writer).unwrap();
        assert!(output.contains("\"method\":\"initialize\""));
        assert!(output.contains("\"method\":\"initialized\""));
        assert!(output.contains("\"method\":\"thread/list\""));
        assert!(output.contains("\"useStateDbOnly\":true"));
        assert!(output.contains("\"appServer\""));
        assert!(output.contains("\"cursor\":\"opaque-cursor\""));
        assert!(output.contains("\"id\":99,\"error\""));
        assert!(output.contains("\"code\":-32601"));
    }

    #[test]
    fn rejects_oversized_and_truncated_messages() {
        let oversized = format!("{}\n", "x".repeat(super::MAX_MESSAGE_BYTES));
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(oversized)), Vec::new());
        assert!(client.initialize().is_err());

        let mut client = AppServerStdio::new(BufReader::new(Cursor::new("{}")), Vec::new());
        assert!(client.initialize().is_err());
    }

    #[test]
    fn rejects_an_unexpected_response_id() {
        let input = concat!(
            "{\"id\":1,\"result\":{}}\n",
            "{\"id\":19,\"result\":{\"data\":[]}}\n"
        );
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        client.initialize().unwrap();
        let error = client.list_threads(1, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unexpected JSON-RPC response id")
        );
    }

    #[test]
    fn start_turn_drains_events_and_denies_every_server_request() {
        let input = concat!(
            "{\"id\":1,\"result\":{}}\n",
            "{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_test\",\"sessionId\":\"thr_test\",\"ephemeral\":false}}}\n",
            "{\"method\":\"thread/started\",\"params\":{}}\n",
            "{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_test\",\"status\":\"inProgress\"}}}\n",
            "{\"id\":90,\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"threadId\":\"thr_test\",\"turnId\":\"turn_test\"}}\n",
            "{\"id\":91,\"method\":\"item/fileChange/requestApproval\",\"params\":{\"threadId\":\"thr_test\",\"turnId\":\"turn_test\"}}\n",
            "{\"id\":92,\"method\":\"item/permissions/requestApproval\",\"params\":{\"threadId\":\"thr_test\",\"turnId\":\"turn_test\"}}\n",
            "{\"id\":93,\"method\":\"mcpServer/elicitation/request\",\"params\":{\"threadId\":\"thr_test\",\"turnId\":\"turn_test\"}}\n",
            "{\"id\":94,\"method\":\"item/tool/call\",\"params\":{}}\n",
            "{\"method\":\"item/completed\",\"params\":{\"threadId\":\"thr_test\",\"turnId\":\"turn_test\",\"item\":{\"type\":\"agentMessage\",\"text\":\"Done\",\"phase\":\"final_answer\"}}}\n",
            "{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thr_test\",\"turn\":{\"id\":\"turn_test\",\"status\":\"completed\",\"error\":null}}}\n"
        );
        let cwd = std::env::current_dir().unwrap();
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        client.initialize().unwrap();
        let thread = client.start_thread(cwd.to_str().unwrap()).unwrap();
        let outcome = client.run_turn(&thread, "Summarize the result").unwrap();
        assert_eq!(outcome.status, TurnFinalStatus::Completed);
        assert_eq!(outcome.final_message.as_deref(), Some("Done"));
        assert_eq!(outcome.requests_declined, 5);

        let output = String::from_utf8(client.writer).unwrap();
        assert!(output.contains("\"method\":\"thread/start\""));
        assert!(output.contains("\"sandbox\":\"read-only\""));
        assert!(output.contains("\"method\":\"turn/start\""));
        assert!(output.contains("\"type\":\"readOnly\",\"networkAccess\":false"));
        assert!(output.contains("\"id\":90,\"result\":{\"decision\":\"decline\"}"));
        assert!(output.contains("\"id\":91,\"result\":{\"decision\":\"decline\"}"));
        assert!(output.contains("\"id\":92,\"result\":{\"permissions\":{}}"));
        assert!(output.contains("\"id\":93,\"result\":{\"action\":\"decline\""));
        assert!(output.contains("\"id\":94,\"error\":{\"code\":-32601"));
        assert!(!output.contains("\"decision\":\"accept\""));
    }

    #[test]
    fn resume_turn_accepts_early_completion_and_reports_failure() {
        let input = concat!(
            "{\"id\":1,\"result\":{}}\n",
            "{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_existing\",\"sessionId\":\"thr_existing\",\"ephemeral\":false}}}\n",
            "{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thr_existing\",\"turn\":{\"id\":\"turn_failed\",\"status\":\"failed\",\"error\":{\"message\":\"simulated failure\"}}}}\n",
            "{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_failed\",\"status\":\"inProgress\"}}}\n"
        );
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        client.initialize().unwrap();
        let thread = client.resume_thread("thr_existing").unwrap();
        let outcome = client.run_turn(&thread, "Continue").unwrap();
        assert_eq!(outcome.status, TurnFinalStatus::Failed);
        assert_eq!(outcome.error_message.as_deref(), Some("simulated failure"));
        assert!(
            String::from_utf8(client.writer)
                .unwrap()
                .contains("\"method\":\"thread/resume\"")
        );
    }

    #[test]
    fn rejects_completion_for_a_different_turn() {
        let input = concat!(
            "{\"id\":1,\"result\":{}}\n",
            "{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_test\",\"ephemeral\":false}}}\n",
            "{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thr_test\",\"turn\":{\"id\":\"other_turn\",\"status\":\"completed\"}}}\n",
            "{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_test\",\"status\":\"inProgress\"}}}\n"
        );
        let cwd = std::env::current_dir().unwrap();
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        client.initialize().unwrap();
        let thread = client.start_thread(cwd.to_str().unwrap()).unwrap();
        let error = client.run_turn(&thread, "Continue").unwrap_err();
        assert!(error.to_string().contains("different turn"));
    }

    #[test]
    fn turn_start_response_is_not_completion() {
        let input = concat!(
            "{\"id\":1,\"result\":{}}\n",
            "{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_test\",\"ephemeral\":false}}}\n",
            "{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_test\",\"status\":\"inProgress\"}}}\n"
        );
        let cwd = std::env::current_dir().unwrap();
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        client.initialize().unwrap();
        let thread = client.start_thread(cwd.to_str().unwrap()).unwrap();
        let error = client.run_turn(&thread, "Continue").unwrap_err();
        assert!(error.to_string().contains("closed stdout"));
    }

    #[test]
    fn interrupted_turn_has_terminal_status_without_failure_error() {
        let input = concat!(
            "{\"id\":1,\"result\":{}}\n",
            "{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_test\",\"ephemeral\":false}}}\n",
            "{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_test\",\"status\":\"inProgress\"}}}\n",
            "{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thr_test\",\"turn\":{\"id\":\"turn_test\",\"status\":\"interrupted\"}}}\n"
        );
        let cwd = std::env::current_dir().unwrap();
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        client.initialize().unwrap();
        let thread = client.start_thread(cwd.to_str().unwrap()).unwrap();
        let outcome = client.run_turn(&thread, "Continue").unwrap();
        assert_eq!(outcome.status, TurnFinalStatus::Interrupted);
        assert!(outcome.error_message.is_none());
    }

    #[test]
    #[ignore = "requires a locally installed Codex executable; does not create a thread"]
    fn installed_app_server_handshake_and_read_only_list() {
        use std::process::{Command, Stdio};
        use std::sync::{Arc, Mutex, mpsc};
        use std::time::Duration;

        let mut child = Command::new("codex")
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("launch codex app-server");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let child = Arc::new(Mutex::new(child));
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut client = AppServerStdio::new(BufReader::new(stdout), stdin);
            let result = client
                .initialize()
                .and_then(|()| client.list_threads(1, None));
            let _ = sender.send(result);
        });
        let result = receiver.recv_timeout(Duration::from_secs(15));
        let mut child = child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        drop(child);
        worker.join().unwrap();
        let output = result
            .expect("app-server did not answer within 15 seconds")
            .unwrap();
        assert!(output.data.len() <= 1, "unexpected list result: {output:?}");
    }
}
