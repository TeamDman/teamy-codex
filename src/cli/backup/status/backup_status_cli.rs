use crate::backup::configured_backup_directory;
use crate::backup::status;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_state::resolve_codex_home;
use arbitrary::Arbitrary;
use facet::Facet;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct BackupStatusArgs {}

impl BackupStatusArgs {
    /// # Errors
    ///
    /// Returns an error when the Codex home or backup state cannot be read.
    #[expect(
        clippy::unused_async,
        reason = "command invoke methods share the async CLI dispatch shape"
    )]
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        cancellation_token.bail_if_cancelled()?;
        let source = resolve_codex_home(global_args.codex_home.as_deref())?;
        let destination = configured_backup_directory()?;
        let report = status(&source, &destination)?;
        // Metadata inspection: setup plus 10 ms per source file/session.
        let items = report
            .source_sessions
            .saturating_add(report.source_global_files);
        let threshold = Some(std::time::Duration::from_secs(3).saturating_add(
            std::time::Duration::from_millis(
                u64::try_from(items).unwrap_or(u64::MAX).saturating_mul(10),
            ),
        ));
        Ok(CliOutput::facet(report, threshold))
    }
}
