use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_state::list_threads;
use crate::codex_state::resolve_codex_home;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct ThreadListArgs {
    /// Maximum number of threads to return. Defaults to 50.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub limit: Option<usize>,

    /// Include archived threads.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub all: bool,

    /// Return only archived threads.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub archived: bool,
}

impl ThreadListArgs {
    /// # Errors
    ///
    /// Returns an error when the Codex database cannot be opened or queried.
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
        let threads = list_threads(&codex_home, &self)?;
        Ok(CliOutput::facet(threads))
    }
}
