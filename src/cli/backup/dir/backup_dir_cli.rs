use crate::cli::backup::dir::set::BackupDirSetArgs;
use crate::cli::backup::dir::show::BackupDirShowArgs;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct BackupDirArgs {
    #[facet(args::subcommand)]
    pub command: BackupDirCommand,
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum BackupDirCommand {
    Set(BackupDirSetArgs),
    Show(BackupDirShowArgs),
}

impl BackupDirArgs {
    /// # Errors
    ///
    /// Returns an error when the backup directory cannot be read or saved.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        match self.command {
            BackupDirCommand::Set(args) => args.invoke(global_args, cancellation_token).await,
            BackupDirCommand::Show(args) => args.invoke(global_args, cancellation_token).await,
        }
    }
}
