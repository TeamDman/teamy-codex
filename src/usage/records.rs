//! Explicit user-supplied request usage exports, never raw Codex sessions.
use super::{UsageGroup, UsageQuery, observed_at, parse_time};
use eyre::{Context, bail};
use facet::Facet;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const MAX_EXPORT_BYTES: u64 = 32 * 1024 * 1024;

/// Token subsets are already included in input/output; only those two form total.
#[derive(Facet, Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTokens {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

impl UsageTokens {
    pub(crate) fn validate(&self) -> eyre::Result<()> {
        if [
            self.input_tokens,
            self.cached_input_tokens,
            self.output_tokens,
            self.reasoning_output_tokens,
            self.total_tokens,
        ]
        .iter()
        .any(|n| *n < 0)
            || self.cached_input_tokens > self.input_tokens
            || self.reasoning_output_tokens > self.output_tokens
            || self.input_tokens.checked_add(self.output_tokens) != Some(self.total_tokens)
        {
            bail!(
                "invalid request token breakdown: nonnegative subsets and total=input+output required"
            );
        }
        Ok(())
    }

    pub(crate) fn add(&mut self, rhs: &Self) -> eyre::Result<()> {
        for (left, right) in [
            (&mut self.input_tokens, rhs.input_tokens),
            (&mut self.cached_input_tokens, rhs.cached_input_tokens),
            (&mut self.output_tokens, rhs.output_tokens),
            (
                &mut self.reasoning_output_tokens,
                rhs.reasoning_output_tokens,
            ),
            (&mut self.total_tokens, rhs.total_tokens),
        ] {
            *left = left
                .checked_add(right)
                .ok_or_else(|| eyre::eyre!("usage total overflow"))?;
        }
        Ok(())
    }
}

/// One discrete response, attributed to its originating thread.
/// Null model/effort means unavailable, never inferred from current settings.
#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    pub response_id: String,
    pub thread_id: String,
    /// Completion timestamp, RFC3339 with an explicit offset.
    pub completed_at: String,
    pub effective_model: Option<String>,
    pub effective_effort: Option<String>,
    pub tokens: UsageTokens,
}

/// Version 1 explicitly carries response deltas, never cumulative snapshots.
/// coverage and provenance describe the supplier's claims, not verified completeness.
#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct UsageExport {
    pub schema_version: u32,
    pub accounting: String,
    pub provenance: String,
    pub coverage: String,
    pub records: Vec<UsageRecord>,
}

#[derive(Facet, Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct UsageGroupKey {
    pub thread_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub day_utc: Option<String>,
}

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct UsageBucket {
    pub key: UsageGroupKey,
    pub responses: usize,
    pub tokens: UsageTokens,
}

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct UsageReport {
    pub schema_version: u32,
    pub observed_at: String,
    pub scope: String,
    pub source_provenance: String,
    pub source_coverage_claim: String,
    pub completeness: String,
    pub query: UsageQuery,
    pub records_read: usize,
    pub duplicates_removed: usize,
    pub unique_responses: usize,
    pub matched_responses: usize,
    pub unknown_model_responses: usize,
    pub unknown_effort_responses: usize,
    pub unknown_model_excluded_by_filter: usize,
    pub totals: UsageTokens,
    pub buckets: Vec<UsageBucket>,
}

/// Read one explicit bounded usage export and apply a typed SDK query.
/// # Errors
/// Rejects raw session schemas, malformed data, duplicate conflicts, and overflow.
pub fn query_usage_file(path: &Path, query: &UsageQuery) -> eyre::Result<UsageReport> {
    let file = File::open(path)
        .wrap_err_with(|| format!("cannot open usage export {}", path.display()))?;
    let mut text = String::new();
    file.take(MAX_EXPORT_BYTES + 1).read_to_string(&mut text)?;
    if u64::try_from(text.len())? > MAX_EXPORT_BYTES {
        bail!("usage export exceeds 32 MiB");
    }
    let export: UsageExport = facet_json::from_str(&text)
        .wrap_err("expected versioned request-usage export, not raw session/rollout data")?;
    query_usage_records(&export, query)
}

/// Aggregate discrete requests; deduplicate response IDs before filtering.
/// Conflicting copies fail, including inconsistent thread attribution from forks.
/// No cumulative deltas are inferred, so source counter resets do not affect this sum.
/// # Errors
/// Rejects invalid exports, dates, filters, duplicate conflicts, and token overflow.
pub fn query_usage_records(export: &UsageExport, query: &UsageQuery) -> eyre::Result<UsageReport> {
    let bounds = query.bounds()?;
    let unique = normalized_records(export)?;
    let unique_responses = unique.len();
    let mut matched_responses = 0;
    let mut unknown_model_responses = 0;
    let mut unknown_effort_responses = 0;
    let mut unknown_model_excluded_by_filter = 0;
    let mut totals = UsageTokens::default();
    let mut buckets = BTreeMap::<UsageGroupKey, UsageBucket>::new();
    for record in unique.values() {
        let time = parse_time(&record.completed_at)?;
        if (!query.threads.is_empty() && !query.threads.contains(&record.thread_id))
            || bounds.since.is_some_and(|since| time < since)
            || bounds.until.is_some_and(|until| time >= until)
        {
            continue;
        }
        if !query.models.is_empty()
            && !record
                .effective_model
                .as_ref()
                .is_some_and(|model| query.models.contains(model))
        {
            if record.effective_model.is_none() {
                unknown_model_excluded_by_filter += 1;
            }
            continue;
        }
        matched_responses += 1;
        unknown_model_responses += usize::from(record.effective_model.is_none());
        unknown_effort_responses += usize::from(record.effective_effort.is_none());
        totals.add(&record.tokens)?;
        let key = UsageGroupKey {
            thread_id: query
                .group_by
                .contains(&UsageGroup::Thread)
                .then(|| record.thread_id.clone()),
            model: if query.group_by.contains(&UsageGroup::Model) {
                record.effective_model.clone()
            } else {
                None
            },
            effort: if query.group_by.contains(&UsageGroup::Effort) {
                record.effective_effort.clone()
            } else {
                None
            },
            day_utc: query
                .group_by
                .contains(&UsageGroup::Day)
                .then(|| time.format("%Y-%m-%d").to_string()),
        };
        let bucket = buckets.entry(key.clone()).or_insert_with(|| UsageBucket {
            key,
            responses: 0,
            tokens: UsageTokens::default(),
        });
        bucket.responses += 1;
        bucket.tokens.add(&record.tokens)?;
    }
    Ok(UsageReport {
        schema_version: 1,
        observed_at: observed_at(),
        scope: "discrete completed responses present in supplied export; UTC [since,until)".to_owned(),
        source_provenance: export.provenance.clone(),
        source_coverage_claim: export.coverage.clone(),
        completeness: "not independently verified; no native-snapshot reconciliation or automatic child/fork expansion".to_owned(),
        query: query.clone(),
        records_read: export.records.len(),
        duplicates_removed: export.records.len() - unique_responses,
        unique_responses,
        matched_responses,
        unknown_model_responses,
        unknown_effort_responses,
        unknown_model_excluded_by_filter,
        totals,
        buckets: buckets.into_values().collect(),
    })
}

fn normalized_records(export: &UsageExport) -> eyre::Result<BTreeMap<String, UsageRecord>> {
    if export.schema_version != 1 || export.accounting != "response-delta" {
        bail!(
            "only schema_version=1 accounting=response-delta is supported; cumulative snapshots/reset inference are unsupported"
        );
    }
    if export.provenance.trim().is_empty() || export.coverage.trim().is_empty() {
        bail!("usage export requires explicit provenance and coverage claims");
    }
    let mut unique = BTreeMap::<String, UsageRecord>::new();
    for record in &export.records {
        if record.response_id.trim().is_empty()
            || record.thread_id.trim().is_empty()
            || record
                .effective_model
                .as_ref()
                .is_some_and(|s| s.trim().is_empty())
            || record
                .effective_effort
                .as_ref()
                .is_some_and(|s| s.trim().is_empty())
        {
            bail!("record identifiers cannot be empty; use null for unknown model/effort");
        }
        record.tokens.validate()?;
        let mut normalized = record.clone();
        normalized.completed_at = parse_time(&record.completed_at)?.to_rfc3339();
        if let Some(previous) = unique.get(&record.response_id) {
            if previous != &normalized {
                bail!("conflicting duplicate response_id {}", record.response_id);
            }
        } else {
            unique.insert(record.response_id.clone(), normalized);
        }
    }
    Ok(unique)
}
