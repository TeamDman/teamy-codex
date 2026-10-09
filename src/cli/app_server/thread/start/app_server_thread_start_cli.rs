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
pub struct AppServerThreadStartArgs {
    /// Absolute working directory for the new persisted thread.
    #[facet(args::named)]
    pub cwd: String,

    /// Text prompt, at most 32 KiB. The turn runs in a read-only sandbox.
    #[facet(args::named)]
    pub prompt: String,

    /// Codex executable to launch. Defaults to `codex` on PATH.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub codex_bin: Option<String>,
}

impl AppServerThreadStartArgs {
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
                ThreadTarget::Start { cwd: self.cwd },
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
    use super::AppServerThreadStartArgs;
    use crate::codex_app_server_stdio::TurnFinalStatus;
    use std::process::Command;
    use teamy_cancellation::CancellationToken;

    #[tokio::test]
    async fn start_command_returns_typed_fake_server_outcome() {
        let cwd = std::env::current_dir().unwrap();
        let args = AppServerThreadStartArgs {
            cwd: cwd.to_str().unwrap().to_owned(),
            prompt: "Describe this fixture".to_owned(),
            codex_bin: None,
        };
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$null = [Console]::In.ReadLine(); [Console]::Out.WriteLine('{\"id\":1,\"result\":{}}'); \
             $null = [Console]::In.ReadLine(); \
             $start = [Console]::In.ReadLine() | ConvertFrom-Json; \
             if ($start.method -ne 'thread/start' -or $start.params.ephemeral -ne $false -or $start.params.sandbox -ne 'read-only') { exit 11 }; \
             [Console]::Out.WriteLine('{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr_cli\",\"ephemeral\":false}}}'); \
             $turn = [Console]::In.ReadLine() | ConvertFrom-Json; \
             if ($turn.method -ne 'turn/start' -or $turn.params.approvalPolicy -ne 'on-request' -or $turn.params.sandboxPolicy.type -ne 'readOnly' -or $turn.params.sandboxPolicy.networkAccess -ne $false) { exit 12 }; \
             [Console]::Out.WriteLine('{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn_cli\",\"status\":\"inProgress\"}}}'); \
             [Console]::Out.WriteLine('{\"id\":90,\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"threadId\":\"thr_cli\",\"turnId\":\"turn_cli\"}}'); \
             $approval = [Console]::In.ReadLine() | ConvertFrom-Json; \
             if ($approval.id -ne 90 -or $approval.result.decision -ne 'decline') { exit 10 }; \
             [Console]::Out.WriteLine('{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thr_cli\",\"turn\":{\"id\":\"turn_cli\",\"status\":\"completed\"}}}'); \
             [Console]::In.ReadToEnd() | Out-Null",
        ]);
        let outcome = args
            .invoke_with_command(command, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(outcome.thread_id, "thr_cli");
        assert_eq!(outcome.status, TurnFinalStatus::Completed);
        assert_eq!(outcome.requests_declined, 1);
        let rendered = facet_json::to_string(&outcome).unwrap();
        assert!(rendered.contains("\"status\":\"completed\""));
    }
}
