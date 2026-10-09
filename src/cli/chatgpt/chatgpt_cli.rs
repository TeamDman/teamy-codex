use crate::cli::chatgpt::transcript::ChatgptTranscriptArgs;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct ChatgptArgs {
    #[facet(args::subcommand)]
    pub command: ChatgptCommand,
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum ChatgptCommand {
    Transcript(ChatgptTranscriptArgs),
}

impl ChatgptArgs {
    /// # Errors
    ///
    /// Returns an error if the selected offline operation fails.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        match self.command {
            ChatgptCommand::Transcript(args) => args.invoke(global_args, cancellation_token).await,
        }
    }
}
