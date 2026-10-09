use crate::backup::directory_info;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_state::resolve_codex_home;
use arbitrary::Arbitrary;
use facet::Facet;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct BackupDirShowArgs {}

impl BackupDirShowArgs {
    /// # Errors
    ///
    /// Returns an error when the Codex home or backup configuration cannot be
    /// resolved.
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
        Ok(CliOutput::facet(
            directory_info(&source)?,
            Some(std::time::Duration::from_secs(1)),
        ))
    }
}
