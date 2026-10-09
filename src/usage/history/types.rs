use super::super::records::{UsageReport, UsageTokens};
use facet::Facet;

#[derive(Facet, Debug, Clone, Default)]
pub struct HistorySource {
    pub path: String,
    pub bytes_at_open: u64,
    pub bytes_scanned: u64,
    pub lines_scanned: usize,
    pub cli_version: Option<String>,
    pub prefix_ordinal_exclusive: Option<u64>,
    pub prefix_truncated: bool,
    pub malformed_metadata_lines: usize,
    pub oversized_lines: usize,
    pub incomplete_trailing_lines: usize,
    pub response_records: usize,
    pub token_count_events: usize,
    pub repeated_counter_events: usize,
    pub counter_resets: usize,
    pub ambiguous_counter_changes: usize,
    pub inherited_records_excluded: usize,
    pub unknown_context_records: usize,
    pub compactions: usize,
    pub error: Option<String>,
}

#[derive(Facet, Debug, Clone)]
pub struct ThreadUsageReconciliation {
    pub thread_id: String,
    pub database_counter_at_query_start: i64,
    pub latest_log_counter: Option<i64>,
    pub recorded_response_tokens: i64,
    /// Signed difference, never silently adjusted to match the database.
    pub database_minus_recorded: i64,
}

#[derive(Facet, Debug, Clone)]
pub struct HistoryReport {
    pub schema_version: u32,
    pub observed_at: String,
    pub history_scope: String,
    pub attribution_basis: String,
    pub modern_response_records: usize,
    pub legacy_counter_delta_records: usize,
    pub duplicate_response_records_removed: usize,
    pub suppressed_legacy_records: usize,
    pub attribution_conflicts: usize,
    pub untimed_records: usize,
    pub untimed_tokens: UsageTokens,
    pub missing_threads: Vec<String>,
    pub sources: Vec<HistorySource>,
    pub reconciliation: Vec<ThreadUsageReconciliation>,
    pub usage: UsageReport,
}
