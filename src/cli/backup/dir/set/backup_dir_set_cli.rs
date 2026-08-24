use crate::backup::directory_info;
use crate::backup::set_backup_directory;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_state::resolve_codex_home;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct BackupDirSetArgs {
    /// Directory that receives the incremental Codex backup.
    #[facet(args::positional)]
    pub path: String,
}

impl BackupDirSetArgs {
    /// # Errors
    ///
    /// Returns an error when the backup directory configuration cannot be
    /// persisted.
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
        set_backup_directory(std::path::Path::new(&self.path))?;
        let source = resolve_codex_home(global_args.codex_home.as_deref())?;
        Ok(CliOutput::facet(directory_info(&source)?))
    }
}
