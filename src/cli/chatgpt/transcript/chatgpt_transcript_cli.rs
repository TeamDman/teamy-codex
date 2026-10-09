use crate::cli::chatgpt::transcript::from_json::ChatgptTranscriptFromJsonArgs;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct ChatgptTranscriptArgs {
    #[facet(args::subcommand)]
    pub command: ChatgptTranscriptCommand,
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum ChatgptTranscriptCommand {
    FromJson(ChatgptTranscriptFromJsonArgs),
}

impl ChatgptTranscriptArgs {
    /// # Errors
    ///
    /// Returns an error if the selected offline operation fails.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        match self.command {
            ChatgptTranscriptCommand::FromJson(args) => {
                args.invoke(global_args, cancellation_token).await
            }
        }
    }
}
