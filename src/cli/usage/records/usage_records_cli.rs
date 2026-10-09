use crate::cli::output::CliOutput;
use crate::usage::{UsageGroup, UsageQuery, records::query_usage_file};
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use std::path::Path;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct UsageRecordsArgs {
    /// Explicit versioned response-delta JSON export. Never scans Codex sessions.
    #[facet(args::named)]
    pub input: String,
    /// Exact originating thread IDs, repeatable.
    #[facet(args::named, default)]
    pub thread: Vec<String>,
    /// Exact effective model identifiers, repeatable. Unknown models are excluded and counted.
    #[facet(args::named, default)]
    pub model: Vec<String>,
    /// Inclusive RFC3339 completion time with offset, e.g. 2026-10-01T00:00:00Z.
    #[facet(args::named)]
    pub since: Option<String>,
    /// Exclusive RFC3339 completion time with offset.
    #[facet(args::named)]
    pub until: Option<String>,
    /// Repeatable dimensions: thread, model, effort, day (UTC). Null keys mean unknown.
    #[facet(args::named, default)]
    pub group_by: Vec<UsageGroup>,
}

impl UsageRecordsArgs {
    /// # Errors
    /// Rejects invalid exports, conflicting response IDs, and invalid filters.
    pub fn invoke(self) -> eyre::Result<CliOutput> {
        let query = UsageQuery {
            threads: self.thread,
            models: self.model,
            since: self.since,
            until: self.until,
            group_by: self.group_by,
        };
        let report = query_usage_file(Path::new(&self.input), &query)?;
        // Bounded 32 MiB export parsing and aggregation: setup plus 100 ms/MiB.
        let threshold = Some(std::time::Duration::from_millis(5200).saturating_add(
            std::time::Duration::from_millis(
                u64::try_from(report.records_read).unwrap_or(u64::MAX),
            ),
        ));
        Ok(CliOutput::facet(report, threshold))
    }
}

#[cfg(test)]
mod budget_tests {
    use super::UsageRecordsArgs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn records_command_returns_budget_without_changing_report_schema() -> eyre::Result<()> {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "teamy-codex-budget-fixture-{}-{suffix}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"schema_version":1,"accounting":"response-delta","provenance":"synthetic fixture","coverage":"empty fixture","records":[]}"#,
        )?;
        let result = UsageRecordsArgs {
            input: path.to_string_lossy().into_owned(),
            thread: Vec::new(),
            model: Vec::new(),
            since: None,
            until: None,
            group_by: Vec::new(),
        }
        .invoke();
        std::fs::remove_file(&path)?;
        assert_eq!(
            result?.elapsed_duration_warn_threshold,
            Some(Duration::from_millis(5200))
        );
        Ok(())
    }
}
