pub mod app_server;
pub mod backup;
pub mod chatgpt;
pub mod global_args;
pub mod output;
pub mod thread;
pub mod usage;

use crate::cli::app_server::AppServerArgs;
use crate::cli::chatgpt::ChatgptArgs;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::cli::thread::ThreadArgs;
use arbitrary::Arbitrary;
use eyre::Context;
use facet::Facet;
use figue as args;
use figue::FigueBuiltins;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug)]
pub struct Cli {
    #[facet(flatten)]
    pub global_args: GlobalArgs,

    #[facet(flatten)]
    #[arbitrary(default)]
    pub builtins: FigueBuiltins,

    #[facet(args::subcommand)]
    pub command: Command,
}

impl Cli {
    /// # Errors
    ///
    /// Returns an error when the runtime cannot be built or command execution fails.
    pub fn invoke(self, cancellation_token: CancellationToken) -> eyre::Result<CliOutput> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .wrap_err("failed to build tokio runtime")?;
        runtime.block_on(async move {
            self.command
                .invoke(&self.global_args, cancellation_token)
                .await
        })
    }
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum Command {
    AppServer(AppServerArgs),
    Backup(backup::BackupArgs),
    Chatgpt(ChatgptArgs),
    Thread(ThreadArgs),
    Usage(usage::UsageArgs),
}

impl Command {
    /// # Errors
    ///
    /// Returns an error when the selected command cannot inspect Codex state or is cancelled.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        cancellation_token.bail_if_cancelled()?;
        match self {
            Self::AppServer(args) => args.invoke(global_args, cancellation_token).await,
            Self::Backup(args) => args.invoke(global_args, cancellation_token).await,
            Self::Chatgpt(args) => args.invoke(global_args, cancellation_token).await,
            Self::Thread(args) => args.invoke(global_args, cancellation_token).await,
            Self::Usage(args) => args.invoke(global_args, cancellation_token).await,
        }
    }
}
