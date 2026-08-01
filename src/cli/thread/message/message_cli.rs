use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::cli::thread::message::list::ThreadMessageListArgs;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct ThreadMessageArgs {
    #[facet(args::subcommand)]
    pub command: ThreadMessageCommand,
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum ThreadMessageCommand {
    List(ThreadMessageListArgs),
}

impl ThreadMessageArgs {
    /// # Errors
    ///
    /// Returns an error when the selected message command cannot inspect Codex state.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        match self.command {
            ThreadMessageCommand::List(args) => args.invoke(global_args, cancellation_token).await,
        }
    }
}
