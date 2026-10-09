use crate::cli::app_server::thread::list::AppServerThreadListArgs;
use crate::cli::app_server::thread::resume::AppServerThreadResumeArgs;
use crate::cli::app_server::thread::start::AppServerThreadStartArgs;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct AppServerThreadArgs {
    #[facet(args::subcommand)]
    pub command: AppServerThreadCommand,
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum AppServerThreadCommand {
    List(AppServerThreadListArgs),
    Start(AppServerThreadStartArgs),
    Resume(AppServerThreadResumeArgs),
}

impl AppServerThreadArgs {
    /// # Errors
    ///
    /// Returns an error if the selected app-server thread operation fails.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        match self.command {
            AppServerThreadCommand::List(args) => {
                args.invoke(global_args, cancellation_token).await
            }
            AppServerThreadCommand::Start(args) => {
                args.invoke(global_args, cancellation_token).await
            }
            AppServerThreadCommand::Resume(args) => {
                args.invoke(global_args, cancellation_token).await
            }
        }
    }
}
