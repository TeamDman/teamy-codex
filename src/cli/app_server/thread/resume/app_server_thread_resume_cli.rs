use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_app_server_process::{ThreadTarget, run_turn_owned};
use crate::codex_app_server_stdio::TurnOutcome;
use arbitrary::Arbitrary;
use eyre::Context;
use facet::Facet;
use figue as args;
use std::process::Command;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct AppServerThreadResumeArgs {
    /// Identifier of an existing persisted Codex thread.
    #[facet(args::positional)]
    pub thread_id: String,

    /// Text prompt, at most 32 KiB. The turn runs in a read-only sandbox.
    #[facet(args::named)]
    pub prompt: String,

    /// Codex executable to launch. Defaults to `codex` on PATH.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub codex_bin: Option<String>,
}

impl AppServerThreadResumeArgs {
    /// # Errors
    ///
    /// Returns an error if the owned child, protocol, or cancellation fails.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        let mut command = Command::new(self.codex_bin.as_deref().unwrap_or("codex"));
        command.args(["app-server", "--stdio"]);
        if let Some(codex_home) = global_args.codex_home.as_deref() {
            command.env("CODEX_HOME", codex_home);
        }
        let outcome = self
            .invoke_with_command(command, cancellation_token)
            .await?;
        // A model turn is open-ended and can involve user-driven work or cancellation.
        Ok(CliOutput::facet(outcome, None))
    }

    async fn invoke_with_command(
        self,
        mut command: Command,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<TurnOutcome> {
        cancellation_token.bail_if_cancelled()?;
        let worker_token = cancellation_token.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            run_turn_owned(
                &mut command,
                ThreadTarget::Resume {
                    thread_id: self.thread_id,
                },
                self.prompt,
                &worker_token,
            )
        })
        .await
        .wrap_err("app-server process owner panicked")??;
        Ok(outcome)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::AppServerThreadResumeArgs;
    use crate::codex_app_server_stdio::TurnFinalStatus;
    use std::process::Command;
    use std::time::Duration;
    use teamy_cancellation::CancellationToken;

    #[tokio::test]
    async fn resume_command_returns_failed_fake_server_outcome() {
        let args = AppServerThreadResumeArgs {
            thread_id: "thr_existing".to_owned(),
            prompt: "Continue".to_owned(),
            codex_bin: None,
        };
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$null = [Console]::In.ReadLine(); [Console]::Out.WriteLine('{\"id\":1,\"result\":{}}'); \
             $null = [Console]::In.ReadLine(); \
             $resume = [Console]::In.ReadLine() | ConvertFrom-Json; \
             if ($resume.method -ne 'thread/resume' -or $resume.params.threadId -ne 'thr_existing') { exit 11 }; \
             [Console]::Out.WriteLine('{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_existing\",\"ephemeral\":false}}}'); \
             $turn = [Console]::In.ReadLine() | ConvertFrom-Json; \
             if ($turn.method -ne 'turn/start' -or $turn.params.sandboxPolicy.type -ne 'readOnly' -or $turn.params.sandboxPolicy.networkAccess -ne $false) { exit 12 }; \
             [Console]::Out.WriteLine('{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_failed\",\"status\":\"inProgress\"}}}'); \
             [Console]::Out.WriteLine('{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thr_existing\",\"turn\":{\"id\":\"turn_failed\",\"status\":\"failed\",\"error\":{\"message\":\"fake failure\"}}}}'); \
             [Console]::In.ReadToEnd() | Out-Null",
        ]);
        let outcome = args
            .invoke_with_command(command, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(outcome.status, TurnFinalStatus::Failed);
        assert_eq!(outcome.error_message.as_deref(), Some("fake failure"));
    }

    #[tokio::test]
    async fn resume_command_preserves_interrupted_status_after_cancellation() {
        let args = AppServerThreadResumeArgs {
            thread_id: "thr_existing".to_owned(),
            prompt: "Continue".to_owned(),
            codex_bin: None,
        };
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$null = [Console]::In.ReadLine(); [Console]::Out.WriteLine('{\"id\":1,\"result\":{}}'); \
             $null = [Console]::In.ReadLine(); $null = [Console]::In.ReadLine(); \
             [Console]::Out.WriteLine('{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_existing\",\"ephemeral\":false}}}'); \
             $null = [Console]::In.ReadLine(); \
             [Console]::Out.WriteLine('{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_active\",\"status\":\"inProgress\"}}}'); \
             $interrupt = [Console]::In.ReadLine() | ConvertFrom-Json; \
             if ($interrupt.id -ne 4 -or $interrupt.method -ne 'turn/interrupt' -or $interrupt.params.threadId -ne 'thr_existing' -or $interrupt.params.turnId -ne 'turn_active') { exit 11 }; \
             [Console]::Out.WriteLine('{\"id\":4,\"result\":{}}'); \
             [Console]::Out.WriteLine('{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thr_existing\",\"turn\":{\"id\":\"turn_active\",\"status\":\"interrupted\"}}}'); \
             [Console]::In.ReadToEnd() | Out-Null",
        ]);
        let cancellation_token = CancellationToken::new();
        let cancel_from_thread = cancellation_token.clone();
        let cancel_worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(1));
            cancel_from_thread.request_cancel("test CLI interruption");
        });
        let outcome = args
            .invoke_with_command(command, cancellation_token)
            .await
            .unwrap();
        cancel_worker.join().unwrap();
        assert_eq!(outcome.status, TurnFinalStatus::Interrupted);
    }
}
