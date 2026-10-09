use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_app_server_process::list_threads_owned;
use crate::codex_app_server_stdio::MAX_THREAD_LIST_LIMIT;
use arbitrary::Arbitrary;
use eyre::{Context, bail};
use facet::Facet;
use figue as args;
use std::process::Command;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct AppServerThreadListArgs {
    /// Maximum rows in this page, from 1 to 100. Defaults to 25.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub limit: Option<u32>,

    /// Opaque cursor returned as `nextCursor` by a prior page.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub cursor: Option<String>,

    /// Codex executable to launch. Defaults to `codex` on PATH.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub codex_bin: Option<String>,
}

impl AppServerThreadListArgs {
    /// # Errors
    ///
    /// Returns an error if the child process, protocol, or cancellation fails.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        cancellation_token.bail_if_cancelled()?;
        let limit = self.limit.unwrap_or(25);
        if !(1..=MAX_THREAD_LIST_LIMIT).contains(&limit) {
            bail!("--limit must be between 1 and {MAX_THREAD_LIST_LIMIT}");
        }
        let mut command = Command::new(self.codex_bin.as_deref().unwrap_or("codex"));
        command.args(["app-server", "--stdio"]);
        if let Some(codex_home) = global_args.codex_home.as_deref() {
            command.env("CODEX_HOME", codex_home);
        }
        let worker_token = cancellation_token.clone();
        let page = tokio::task::spawn_blocking(move || {
            list_threads_owned(&mut command, limit, self.cursor, &worker_token)
        })
        .await
        .wrap_err("app-server process owner panicked")??;
        cancellation_token.bail_if_cancelled()?;
        // Child startup/protocol setup plus 20 ms per requested page row.
        let threshold = Some(std::time::Duration::from_secs(5).saturating_add(
            std::time::Duration::from_millis(u64::from(limit).saturating_mul(20)),
        ));
        Ok(CliOutput::facet(page, threshold))
    }
}
