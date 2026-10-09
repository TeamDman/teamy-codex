//! Own a short-lived `codex app-server` child for bounded protocol operations.

use crate::codex_app_server_stdio::{
    AppServerStdio, MAX_CWD_BYTES, MAX_PROMPT_BYTES, MAX_THREAD_ID_BYTES, ThreadListPage,
    TurnOutcome, send_turn_interrupt,
};
use eyre::{Context, ContextCompat, bail, eyre};
use std::io::{self, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use teamy_cancellation::CancellationToken;

const CANCELLATION_POLL: Duration = Duration::from_millis(50);
const INTERRUPT_GRACE: Duration = Duration::from_secs(2);

struct SharedChildStdin(Arc<Mutex<ChildStdin>>);

impl Write for SharedChildStdin {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut writer = self
            .0
            .lock()
            .map_err(|_poison| io::Error::other("app-server stdin lock poisoned"))?;
        // The adapter writes an entire JSONL frame in one call, so keeping
        // this lock through write_all prevents interleaved request bytes.
        writer.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .map_err(|_poison| io::Error::other("app-server stdin lock poisoned"))?
            .flush()
    }
}

struct ActiveTurn {
    thread_id: String,
    turn_id: String,
    interrupt_request: u64,
}

struct SupervisedResult<T> {
    response: eyre::Result<T>,
    interrupt_worker: Option<std::thread::JoinHandle<()>>,
    interrupt_attempted: bool,
}

/// Select whether a controlled turn starts a persisted thread or resumes one.
#[derive(Debug)]
pub enum ThreadTarget {
    Start { cwd: String },
    Resume { thread_id: String },
}

#[derive(Debug)]
struct OwnedChild {
    child: Child,
    reaped: bool,
}

impl OwnedChild {
    fn terminate_and_reap(&mut self) -> eyre::Result<()> {
        if self.child.try_wait()?.is_none()
            && let Err(error) = self.child.kill()
        {
            // The child may exit between `try_wait` and `kill`.
            if self.child.try_wait()?.is_none() {
                return Err(error).wrap_err("could not terminate Codex app-server child");
            }
        }
        self.child
            .wait()
            .wrap_err("could not reap Codex app-server child")?;
        self.reaped = true;
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.terminate_and_reap();
        }
    }
}

/// Spawn an owned app-server child, list one page, then terminate and reap it.
/// The caller supplies the executable and arguments, allowing controlled
/// fake-server tests without touching Codex user state.
///
/// # Errors
///
/// Returns an error if launch, protocol, cancellation, or cleanup fails.
pub fn list_threads_owned(
    command: &mut Command,
    limit: u32,
    cursor: Option<String>,
    cancellation_token: &CancellationToken,
) -> eyre::Result<ThreadListPage> {
    run_owned(command, cancellation_token, move |client, _, _| {
        client.list_threads(limit, cursor.as_deref())
    })
}

/// Create or resume one persisted thread, then run and observe one text turn.
/// The turn is forced into a read-only sandbox; approval requests are denied.
///
/// # Errors
///
/// Returns an error if launch, protocol, cancellation, or cleanup fails.
pub fn run_turn_owned(
    command: &mut Command,
    target: ThreadTarget,
    prompt: String,
    cancellation_token: &CancellationToken,
) -> eyre::Result<TurnOutcome> {
    if prompt.trim().is_empty() || prompt.len() > MAX_PROMPT_BYTES {
        bail!("prompt must contain 1 to {MAX_PROMPT_BYTES} bytes");
    }
    match &target {
        ThreadTarget::Start { cwd } => {
            if cwd.len() > MAX_CWD_BYTES || !Path::new(cwd).is_absolute() {
                bail!("cwd must be an absolute path of at most {MAX_CWD_BYTES} bytes");
            }
        }
        ThreadTarget::Resume { thread_id } => {
            if thread_id.is_empty() || thread_id.len() > MAX_THREAD_ID_BYTES {
                bail!("thread id must contain 1 to {MAX_THREAD_ID_BYTES} bytes");
            }
        }
    }
    run_owned(
        command,
        cancellation_token,
        move |client, interrupt_requested, started| {
            let thread = match target {
                ThreadTarget::Start { cwd } => client.start_thread(&cwd)?,
                ThreadTarget::Resume { thread_id } => client.resume_thread(&thread_id)?,
            };
            client.run_turn_with_started(
                &thread,
                &prompt,
                interrupt_requested,
                |thread_id, turn_id, interrupt_request_id| {
                    let _ = started.send(ActiveTurn {
                        thread_id: thread_id.to_owned(),
                        turn_id: turn_id.to_owned(),
                        interrupt_request: interrupt_request_id,
                    });
                },
            )
        },
    )
}

fn run_owned<T, F>(
    command: &mut Command,
    cancellation_token: &CancellationToken,
    operation: F,
) -> eyre::Result<T>
where
    T: Send + 'static,
    F: FnOnce(
            &mut AppServerStdio<BufReader<ChildStdout>, SharedChildStdin>,
            &AtomicBool,
            &mpsc::SyncSender<ActiveTurn>,
        ) -> eyre::Result<T>
        + Send
        + 'static,
{
    cancellation_token.bail_if_cancelled()?;
    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .wrap_err("could not launch Codex app-server")?;
    let mut child = OwnedChild {
        child,
        reaped: false,
    };
    let stdin = child
        .child
        .stdin
        .take()
        .context("app-server stdin not piped")?;
    let stdout = child
        .child
        .stdout
        .take()
        .context("app-server stdout not piped")?;
    let mut stderr = child
        .child
        .stderr
        .take()
        .context("app-server stderr not piped")?;

    // The protocol reader may block on stdout. A separate, frame-atomic
    // writer can request an interrupt without taking ownership from it.
    let shared_stdin = Arc::new(Mutex::new(stdin));
    let protocol_stdin = SharedChildStdin(Arc::clone(&shared_stdin));
    let interrupt_requested = Arc::new(AtomicBool::new(false));
    let protocol_interrupt = Arc::clone(&interrupt_requested);
    let (started_sender, started_receiver) = mpsc::sync_channel(1);
    let (sender, receiver) = mpsc::sync_channel(1);
    let protocol_worker = std::thread::Builder::new()
        .name("codex-app-server-stdio".to_owned())
        .spawn(move || {
            let mut client = AppServerStdio::new(BufReader::new(stdout), protocol_stdin);
            let result = client
                .initialize()
                .and_then(|()| operation(&mut client, &protocol_interrupt, &started_sender));
            let _ = sender.send(result);
        })
        .wrap_err("could not start app-server protocol reader")?;
    let stderr_worker = std::thread::Builder::new()
        .name("codex-app-server-stderr".to_owned())
        .spawn(move || io::copy(&mut stderr, &mut io::sink()))
        .wrap_err("could not start app-server stderr drainer")?;

    let SupervisedResult {
        response,
        interrupt_worker,
        interrupt_attempted,
    } = supervise(
        &receiver,
        &started_receiver,
        &shared_stdin,
        &interrupt_requested,
        cancellation_token,
    );

    child.terminate_and_reap()?;
    protocol_worker
        .join()
        .map_err(|_panic| eyre!("app-server protocol reader panicked"))?;
    if let Some(worker) = interrupt_worker {
        worker
            .join()
            .map_err(|_panic| eyre!("app-server interrupt writer panicked"))?;
    }
    stderr_worker
        .join()
        .map_err(|_panic| eyre!("app-server stderr drainer panicked"))?
        .wrap_err("could not drain app-server stderr")?;
    if !interrupt_attempted {
        cancellation_token.bail_if_cancelled()?;
    }
    response
}

fn supervise<T>(
    receiver: &mpsc::Receiver<eyre::Result<T>>,
    started_receiver: &mpsc::Receiver<ActiveTurn>,
    shared_stdin: &Arc<Mutex<ChildStdin>>,
    interrupt_requested: &Arc<AtomicBool>,
    cancellation_token: &CancellationToken,
) -> SupervisedResult<T> {
    let (interrupt_sender, interrupt_receiver) = mpsc::sync_channel(1);
    let mut active_turn = None;
    let mut interrupt_deadline = None;
    let mut interrupt_worker = None;
    let response = loop {
        match receiver.try_recv() {
            Ok(result) => break result,
            Err(TryRecvError::Disconnected) => {
                break Err(eyre!("app-server protocol reader stopped without a result"));
            }
            Err(TryRecvError::Empty) => {}
        }
        if active_turn.is_none()
            && let Ok(started) = started_receiver.try_recv()
        {
            active_turn = Some(started);
        }
        if interrupt_deadline.is_none() {
            if let Err(error) = cancellation_token.bail_if_cancelled() {
                let Some(ActiveTurn {
                    thread_id,
                    turn_id,
                    interrupt_request,
                }) = active_turn.take()
                else {
                    break Err(error);
                };
                interrupt_requested.store(true, Ordering::Release);
                let mut writer = SharedChildStdin(Arc::clone(shared_stdin));
                let result_sender = interrupt_sender.clone();
                match std::thread::Builder::new()
                    .name("codex-app-server-interrupt".to_owned())
                    .spawn(move || {
                        let result = send_turn_interrupt(
                            &mut writer,
                            interrupt_request,
                            &thread_id,
                            &turn_id,
                        );
                        let _ = result_sender.send(result);
                    }) {
                    Ok(worker) => interrupt_worker = Some(worker),
                    Err(error) => {
                        break Err(error).wrap_err("could not start app-server interrupt writer");
                    }
                }
                interrupt_deadline = Some(Instant::now() + INTERRUPT_GRACE);
            }
        } else {
            if let Ok(Err(error)) = interrupt_receiver.try_recv() {
                break Err(error).wrap_err("could not send turn/interrupt");
            }
            if interrupt_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                break Err(eyre!(
                    "turn/interrupt did not complete within the grace period"
                ));
            }
        }
        let wait = interrupt_deadline.map_or(CANCELLATION_POLL, |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .min(CANCELLATION_POLL)
        });
        match receiver.recv_timeout(wait) {
            Ok(result) => break result,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                break Err(eyre!("app-server protocol reader stopped without a result"));
            }
        }
    };
    SupervisedResult {
        response,
        interrupt_worker,
        interrupt_attempted: interrupt_deadline.is_some(),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::{ThreadTarget, list_threads_owned, run_turn_owned};
    use crate::codex_app_server_stdio::TurnFinalStatus;
    use std::process::Command;
    use std::time::{Duration, Instant};
    use teamy_cancellation::CancellationToken;

    fn fake_server(script: &str) -> Command {
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        command
    }

    #[test]
    fn owns_fake_server_and_projects_list_page() {
        let mut command = fake_server(
            "$null = [Console]::In.ReadLine(); [Console]::Out.WriteLine('{\"id\":1,\"result\":{}}'); \
             $null = [Console]::In.ReadLine(); $null = [Console]::In.ReadLine(); \
             [Console]::Out.WriteLine('{\"id\":2,\"result\":{\"data\":[{\"id\":\"thr_test\",\"name\":\"Test\",\"createdAt\":42}],\"nextCursor\":\"more\"}}'); \
             [Console]::In.ReadToEnd() | Out-Null",
        );
        let page = list_threads_owned(&mut command, 1, None, &CancellationToken::new()).unwrap();
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].id, "thr_test");
        assert_eq!(page.data[0].name.as_deref(), Some("Test"));
        assert_eq!(page.next_cursor.as_deref(), Some("more"));
    }

    #[test]
    fn cancellation_terminates_blocked_child() {
        let mut command = fake_server("$null = [Console]::In.ReadLine(); Start-Sleep -Seconds 30");
        let cancellation_token = CancellationToken::new();
        let cancel_from_thread = cancellation_token.clone();
        let cancel_worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            cancel_from_thread.request_cancel("test cancellation");
        });
        let started = Instant::now();
        let result = list_threads_owned(&mut command, 1, None, &cancellation_token);
        cancel_worker.join().unwrap();
        let _error = result.expect_err("cancelled fake server must stop");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn owns_fake_server_through_turn_completion() {
        let mut command = fake_server(
            "$null = [Console]::In.ReadLine(); [Console]::Out.WriteLine('{\"id\":1,\"result\":{}}'); \
             $null = [Console]::In.ReadLine(); $null = [Console]::In.ReadLine(); \
             [Console]::Out.WriteLine('{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_fake\",\"ephemeral\":false}}}'); \
             $null = [Console]::In.ReadLine(); \
             [Console]::Out.WriteLine('{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_fake\",\"status\":\"inProgress\"}}}'); \
             [Console]::Out.WriteLine('{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thr_fake\",\"turn\":{\"id\":\"turn_fake\",\"status\":\"completed\"}}}'); \
             [Console]::In.ReadToEnd() | Out-Null",
        );
        let cwd = std::env::current_dir().unwrap();
        let outcome = run_turn_owned(
            &mut command,
            ThreadTarget::Start {
                cwd: cwd.to_str().unwrap().to_owned(),
            },
            "Describe the fixture".to_owned(),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(outcome.thread_id, "thr_fake");
        assert_eq!(outcome.status, TurnFinalStatus::Completed);
    }

    #[test]
    fn cancellation_interrupts_known_turn_and_returns_terminal_status() {
        let mut command = fake_server(
            "$null = [Console]::In.ReadLine(); [Console]::Out.WriteLine('{\"id\":1,\"result\":{}}'); \
             $null = [Console]::In.ReadLine(); $null = [Console]::In.ReadLine(); \
             [Console]::Out.WriteLine('{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_fake\",\"ephemeral\":false}}}'); \
             $null = [Console]::In.ReadLine(); \
             [Console]::Out.WriteLine('{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_fake\",\"status\":\"inProgress\"}}}'); \
             $interrupt = [Console]::In.ReadLine() | ConvertFrom-Json; \
             if ($interrupt.id -ne 4 -or $interrupt.method -ne 'turn/interrupt' -or $interrupt.params.threadId -ne 'thr_fake' -or $interrupt.params.turnId -ne 'turn_fake') { exit 11 }; \
             [Console]::Out.WriteLine('{\"id\":4,\"result\":{}}'); \
             [Console]::Out.WriteLine('{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thr_fake\",\"turn\":{\"id\":\"turn_fake\",\"status\":\"interrupted\"}}}'); \
             [Console]::In.ReadToEnd() | Out-Null",
        );
        let cancellation_token = CancellationToken::new();
        let cancel_from_thread = cancellation_token.clone();
        let cancel_worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(1));
            cancel_from_thread.request_cancel("test graceful interruption");
        });
        let started = Instant::now();
        let outcome = run_turn_owned(
            &mut command,
            ThreadTarget::Resume {
                thread_id: "thr_fake".to_owned(),
            },
            "Continue".to_owned(),
            &cancellation_token,
        )
        .unwrap();
        cancel_worker.join().unwrap();
        assert_eq!(outcome.turn_id, "turn_fake");
        assert_eq!(outcome.status, TurnFinalStatus::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn cancellation_times_out_interrupt_then_terminates_child() {
        let mut command = fake_server(
            "$null = [Console]::In.ReadLine(); [Console]::Out.WriteLine('{\"id\":1,\"result\":{}}'); \
             $null = [Console]::In.ReadLine(); $null = [Console]::In.ReadLine(); \
             [Console]::Out.WriteLine('{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_fake\",\"ephemeral\":false}}}'); \
             $null = [Console]::In.ReadLine(); \
             [Console]::Out.WriteLine('{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_fake\",\"status\":\"inProgress\"}}}'); \
             Start-Sleep -Seconds 30",
        );
        let cancellation_token = CancellationToken::new();
        let cancel_from_thread = cancellation_token.clone();
        let cancel_worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(1));
            cancel_from_thread.request_cancel("test turn cancellation");
        });
        let started = Instant::now();
        let result = run_turn_owned(
            &mut command,
            ThreadTarget::Resume {
                thread_id: "thr_fake".to_owned(),
            },
            "Continue".to_owned(),
            &cancellation_token,
        );
        cancel_worker.join().unwrap();
        let error = result.unwrap_err();
        assert!(
            format!("{error:?}")
                .contains("turn/interrupt did not complete within the grace period")
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
