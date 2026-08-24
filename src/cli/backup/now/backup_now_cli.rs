use crate::backup::configured_backup_directory;
use crate::backup::run_backup;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_state::resolve_codex_home;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct BackupNowArgs {
    /// Concurrent session copies; defaults to min(4, available parallelism).
    #[facet(args::named)]
    #[arbitrary(default)]
    pub jobs: Option<usize>,
}

impl BackupNowArgs {
    /// # Errors
    ///
    /// Returns an error when the Codex home cannot be read or the backup
    /// destination cannot be updated.
    #[expect(
        clippy::unused_async,
        reason = "command invoke methods share the async CLI dispatch shape"
    )]
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        let source = resolve_codex_home(global_args.codex_home.as_deref())?;
        let destination = configured_backup_directory()?;
        let report = run_backup(
            &source,
            &destination,
            &cancellation_token,
            self.jobs.unwrap_or_else(crate::backup::default_backup_jobs),
        )?;
        Ok(CliOutput::facet(report))
    }
}
