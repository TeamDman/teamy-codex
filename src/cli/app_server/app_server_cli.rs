use crate::cli::app_server::thread::AppServerThreadArgs;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct AppServerArgs {
    #[facet(args::subcommand)]
    pub command: AppServerCommand,
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum AppServerCommand {
    Thread(AppServerThreadArgs),
}

impl AppServerArgs {
    /// # Errors
    ///
    /// Returns an error if the selected app-server operation fails.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        match self.command {
            AppServerCommand::Thread(args) => args.invoke(global_args, cancellation_token).await,
        }
    }
}
