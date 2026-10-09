use crate::cli::{global_args::GlobalArgs, output::CliOutput};
use crate::codex_state::resolve_codex_home;
use crate::usage::{UsageGroup, UsageQuery, history::query_history_cancellable};
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct UsageHistoryArgs {
    /// Exact thread IDs, repeatable. Default: all native metadata rows, including archived.
    #[facet(args::named, default)]
    pub thread: Vec<String>,
    /// Exact persisted request model identifier, repeatable. Unknown models are reported.
    #[facet(args::named, default)]
    pub model: Vec<String>,
    /// Inclusive RFC3339 usage completion/observation timestamp with explicit offset.
    #[facet(args::named)]
    pub since: Option<String>,
    /// Exclusive RFC3339 usage completion/observation timestamp with explicit offset.
    #[facet(args::named)]
    pub until: Option<String>,
    /// Repeatable grouping: thread, model, effort, day (UTC).
    #[facet(args::named, default)]
    pub group_by: Vec<UsageGroup>,
}
impl UsageHistoryArgs {
    /// # Errors
    /// Returns read-only native storage, filter, or usage validation failures.
    pub fn invoke(
        self,
        global: &GlobalArgs,
        cancellation: &teamy_cancellation::CancellationToken,
    ) -> eyre::Result<CliOutput> {
        let query = UsageQuery {
            threads: self.thread,
            models: self.model,
            since: self.since,
            until: self.until,
            group_by: self.group_by,
        };
        let report = query_history_cancellable(
            &resolve_codex_home(global.codex_home.as_deref())?,
            &query,
            cancellation,
        )?;
        // Historical rollouts have no total scan-byte bound; retain cancellation
        // and operation-specific scanning diagnostics for this archival query.
        let threshold = None;
        Ok(CliOutput::facet(report, threshold))
    }
}
