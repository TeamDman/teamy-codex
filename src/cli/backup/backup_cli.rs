use crate::cli::backup::dir::BackupDirArgs;
use crate::cli::backup::now::BackupNowArgs;
use crate::cli::backup::status::BackupStatusArgs;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct BackupArgs {
    #[facet(args::subcommand)]
    pub command: BackupCommand,
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum BackupCommand {
    Now(BackupNowArgs),
    Dir(BackupDirArgs),
    Status(BackupStatusArgs),
}

impl BackupArgs {
    /// # Errors
    ///
    /// Returns an error when the selected backup command cannot read or write
    /// the configured Codex backup.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        match self.command {
            BackupCommand::Now(args) => args.invoke(global_args, cancellation_token).await,
            BackupCommand::Dir(args) => args.invoke(global_args, cancellation_token).await,
            BackupCommand::Status(args) => args.invoke(global_args, cancellation_token).await,
        }
    }
}
