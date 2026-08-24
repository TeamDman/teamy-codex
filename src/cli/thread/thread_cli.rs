use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::cli::thread::inventory::ThreadInventoryArgs;
use crate::cli::thread::list::ThreadListArgs;
use crate::cli::thread::message::ThreadMessageArgs;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct ThreadArgs {
    #[facet(args::subcommand)]
    pub command: ThreadCommand,
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum ThreadCommand {
    Inventory(ThreadInventoryArgs),
    List(ThreadListArgs),
    Message(ThreadMessageArgs),
}

impl ThreadArgs {
    /// # Errors
    ///
    /// Returns an error when the selected thread command cannot inspect Codex state.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        match self.command {
            ThreadCommand::Inventory(args) => args.invoke(global_args, cancellation_token).await,
            ThreadCommand::List(args) => args.invoke(global_args, cancellation_token).await,
            ThreadCommand::Message(args) => args.invoke(global_args, cancellation_token).await,
        }
    }
}
