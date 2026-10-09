use crate::chatgpt_response_json::read_chatgpt_response;
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use std::path::Path;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct ChatgptTranscriptFromJsonArgs {
    /// A local JSON response saved from a `ChatGPT` conversation.
    #[facet(args::positional)]
    pub path: String,

    /// Optionally verify that this canonical chat URL matches the response ID.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub conversation_url: Option<String>,
}

impl ChatgptTranscriptFromJsonArgs {
    /// # Errors
    ///
    /// Returns an error for an incomplete or unsupported local response.
    #[expect(
        clippy::unused_async,
        reason = "command invoke methods share the async CLI dispatch shape"
    )]
    pub async fn invoke(
        self,
        _global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        cancellation_token.bail_if_cancelled()?;
        let transcript =
            read_chatgpt_response(Path::new(&self.path), self.conversation_url.as_deref())?;
        cancellation_token.bail_if_cancelled()?;
        // Bounded 16 MiB parser plus 10 ms per selected message for formatting.
        let threshold = Some(
            std::time::Duration::from_millis(3600).saturating_add(
                std::time::Duration::from_millis(
                    u64::try_from(transcript.messages.len())
                        .unwrap_or(u64::MAX)
                        .saturating_mul(10),
                ),
            ),
        );
        Ok(CliOutput::facet(transcript, threshold))
    }
}
