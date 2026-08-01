use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_state::list_thread_messages;
use crate::codex_state::resolve_codex_home;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct ThreadMessageListArgs {
    /// Codex thread identifier.
    #[facet(args::positional)]
    pub thread_id: String,

    /// Maximum number of messages to return. Defaults to 50.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub limit: Option<usize>,
}

impl ThreadMessageListArgs {
    /// # Errors
    ///
    /// Returns an error when the thread database or rollout file cannot be read.
    #[expect(
        clippy::unused_async,
        reason = "command invoke methods share the async CLI dispatch shape"
    )]
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        cancellation_token.bail_if_cancelled()?;
        let codex_home = resolve_codex_home(global_args.codex_home.as_deref())?;
        let messages = list_thread_messages(&codex_home, &self)?;
        Ok(CliOutput::facet(messages))
    }
}
