use crate::cli::{global_args::GlobalArgs, output::CliOutput};
use crate::codex_state::resolve_codex_home;
use crate::usage::{UsageGroup, UsageQuery, snapshots::query_snapshots};
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct UsageSnapshotsArgs {
    /// Exact thread IDs (repeatable). Includes archived rows; no implicit limit.
    #[facet(args::named, default)]
    pub thread: Vec<String>,
    /// Unsupported on native snapshots; rejected rather than attributed to latest model.
    #[facet(args::named, default)]
    pub model: Vec<String>,
    /// Historical bounds are unsupported on native snapshots.
    #[facet(args::named)]
    pub since: Option<String>,
    /// Historical bounds are unsupported on native snapshots.
    #[facet(args::named)]
    pub until: Option<String>,
    /// Only thread grouping is supported for native snapshots.
    #[facet(args::named, default)]
    pub group_by: Vec<UsageGroup>,
}

impl UsageSnapshotsArgs {
    /// # Errors
    /// Rejects unsupported attribution and read-only metadata errors.
    pub fn invoke(self, global: &GlobalArgs) -> eyre::Result<CliOutput> {
        let query = UsageQuery {
            threads: self.thread,
            models: self.model,
            since: self.since,
            until: self.until,
            group_by: self.group_by,
        };
        let report = query_snapshots(&resolve_codex_home(global.codex_home.as_deref())?, &query)?;
        // Snapshot query and formatting: setup plus 10 ms per returned row.
        let threshold = Some(
            std::time::Duration::from_secs(2).saturating_add(std::time::Duration::from_millis(
                u64::try_from(report.snapshots.len())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(10),
            )),
        );
        Ok(CliOutput::facet(report, threshold))
    }
}
