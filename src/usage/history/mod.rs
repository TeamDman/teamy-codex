//! On-demand, read-only native usage history. No conversation content leaves this module.
mod scan;
mod storage;
mod types;
mod wire;
pub use types::{HistoryReport, HistorySource, ThreadUsageReconciliation};

use super::{
    UsageQuery, observed_at,
    records::{UsageExport, UsageRecord, UsageTokens, query_usage_records},
};
use scan::{Candidate, ScanState};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use storage::{Storage, read_header};
use teamy_cancellation::CancellationToken;
use wire::HistoryBase;

struct HistoryReader<'a> {
    storage: &'a Storage,
    owner: &'a str,
    cancellation: &'a CancellationToken,
    state: ScanState,
    sources: Vec<HistorySource>,
    visiting: BTreeSet<PathBuf>,
}

impl HistoryReader<'_> {
    fn read(&mut self, path: &Path, prefix: Option<&HistoryBase>) -> eyre::Result<()> {
        self.cancellation.bail_if_cancelled()?;
        let path = self.storage.allowed(path)?;
        if self.visiting.len() >= 128 || !self.visiting.insert(path.clone()) {
            eyre::bail!("cyclic or excessively deep history_base chain");
        }
        let result = self.read_segment(&path, prefix);
        self.visiting.remove(&path);
        result
    }

    fn read_segment(&mut self, path: &Path, prefix: Option<&HistoryBase>) -> eyre::Result<()> {
        let header = read_header(path)?;
        if prefix.is_none() && header.id != self.owner {
            eyre::bail!("native metadata/thread owner mismatch");
        }
        let length = fs::metadata(path)?.len();
        let mut source = HistorySource {
            path: path.display().to_string(),
            bytes_at_open: length,
            cli_version: header.cli_version.clone(),
            prefix_ordinal_exclusive: prefix.map(|p| p.end_ordinal_exclusive),
            prefix_truncated: prefix.is_some_and(|p| p.end_byte_offset > length),
            ..HistorySource::default()
        };
        if let Some(base) = &header.history_base {
            let result = self
                .storage
                .base_path(base)
                .and_then(|base_path| self.read(&base_path, Some(base)));
            if let Err(error) = result {
                source.error = Some(error.to_string());
            }
        }
        self.cancellation.bail_if_cancelled()?;
        let limit = prefix.map_or(length, |p| p.end_byte_offset.min(length));
        if let Err(error) = self.state.scan(
            path,
            &header,
            limit,
            prefix.map(|p| p.end_ordinal_exclusive),
            self.owner,
            &mut source,
            self.cancellation,
        ) {
            source.error = Some(error.to_string());
        }
        self.cancellation.bail_if_cancelled()?;
        self.sources.push(source);
        Ok(())
    }
}

#[derive(Default)]
struct NativeRecords {
    unique: BTreeMap<String, UsageRecord>,
    modern: usize,
    legacy: usize,
    duplicates: usize,
    suppressed: usize,
    attribution_conflicts: usize,
}

fn deduplicate(candidates: Vec<Candidate>) -> eyre::Result<NativeRecords> {
    let modern_turns = candidates
        .iter()
        .filter(|c| c.modern)
        .map(|c| (c.record.thread_id.clone(), c.turn.clone()))
        .collect::<BTreeSet<_>>();
    let mut result = NativeRecords::default();
    for Candidate {
        record,
        turn,
        modern,
    } in candidates
    {
        if !modern && modern_turns.contains(&(record.thread_id.clone(), turn)) {
            result.suppressed += 1;
            continue;
        }
        if let Some(old) = result.unique.get_mut(&record.response_id) {
            if old.tokens != record.tokens || old.thread_id != record.thread_id {
                eyre::bail!("conflicting native response identity; refusing to double-count");
            }
            if old.effective_model != record.effective_model
                || old.effective_effort != record.effective_effort
            {
                old.effective_model = None;
                old.effective_effort = None;
                result.attribution_conflicts += 1;
            }
            if old.completed_at.is_empty() && !record.completed_at.is_empty() {
                record.completed_at.clone_into(&mut old.completed_at);
            }
            result.duplicates += 1;
            continue;
        }
        if modern {
            result.modern += 1;
        } else {
            result.legacy += 1;
        }
        result.unique.insert(record.response_id.clone(), record);
    }
    Ok(result)
}

/// Query native history selected by read-only `SQLite` metadata and explicit prefix links.
/// Files are captured at their initial lengths; only usage/settings metadata is retained.
/// # Errors
/// Returns storage, invalid filter, conflicting response identity or arithmetic errors.
/// Missing/corrupt history segments become content-free coverage diagnostics.
pub fn query_history(home: &Path, query: &UsageQuery) -> eyre::Result<HistoryReport> {
    query_history_cancellable(home, query, &CancellationToken::new())
}

/// Query native history with cancellation local to this reader, never to a Codex thread.
/// # Errors
/// Returns the same failures as `query_history`, plus reader cancellation.
pub fn query_history_cancellable(
    home: &Path,
    query: &UsageQuery,
    cancellation: &CancellationToken,
) -> eyre::Result<HistoryReport> {
    query.bounds()?;
    cancellation.bail_if_cancelled()?;
    let storage = Storage::open(home, &query.threads)?;
    let missing_threads = query
        .threads
        .iter()
        .filter(|id| !storage.roots.iter().any(|r| &r.id == *id))
        .cloned()
        .collect();
    let mut candidates = Vec::new();
    let mut sources = Vec::new();
    let mut last_counters = BTreeMap::new();
    for root in &storage.roots {
        cancellation.bail_if_cancelled()?;
        let mut reader = HistoryReader {
            storage: &storage,
            owner: &root.id,
            cancellation,
            state: ScanState::default(),
            sources: Vec::new(),
            visiting: BTreeSet::new(),
        };
        if let Err(error) = reader.read(&root.path, None) {
            reader.sources.push(HistorySource {
                path: root.path.display().to_string(),
                error: Some(error.to_string()),
                ..HistorySource::default()
            });
        }
        cancellation.bail_if_cancelled()?;
        candidates.extend(reader.state.records);
        last_counters.extend(reader.state.last_counters);
        sources.extend(reader.sources);
    }
    let collected = deduplicate(candidates)?;
    let mut untimed_records = 0;
    let mut untimed_tokens = UsageTokens::default();
    let mut owned_totals = BTreeMap::<String, i64>::new();
    let mut records = Vec::new();
    for record in collected.unique.into_values() {
        let total = owned_totals.entry(record.thread_id.clone()).or_default();
        *total = total
            .checked_add(record.tokens.total_tokens)
            .ok_or_else(|| eyre::eyre!("history total overflow"))?;
        if record.completed_at.is_empty() {
            untimed_records += 1;
            untimed_tokens.add(&record.tokens)?;
        } else {
            records.push(record);
        }
    }
    let export = UsageExport { schema_version: 1, accounting: "response-delta".to_owned(),
        provenance: "read-only native JSONL usage metadata; response IDs and preceding same-turn step contexts".to_owned(),
        coverage: "selected logical history prefixes; archived roots included; abandoned tails and descendant/fork usage excluded".to_owned(), records };
    let mut usage = query_usage_records(&export, query)?;
    "stored response usage plus validated legacy deltas; UTC [since,until); see history diagnostics".clone_into(&mut usage.scope);
    "see sources and signed reconciliation; no forced equality with cumulative snapshots"
        .clone_into(&mut usage.completeness);
    let reconciliation = storage
        .roots
        .iter()
        .map(|root| {
            let recorded = owned_totals.get(&root.id).copied().unwrap_or_default();
            Ok(ThreadUsageReconciliation {
                thread_id: root.id.clone(),
                database_counter_at_query_start: root.counter,
                latest_log_counter: last_counters.get(&root.id).copied(),
                recorded_response_tokens: recorded,
                database_minus_recorded: root
                    .counter
                    .checked_sub(recorded)
                    .ok_or_else(|| eyre::eyre!("reconciliation overflow"))?,
            })
        })
        .collect::<eyre::Result<Vec<_>>>()?;
    Ok(HistoryReport { schema_version: 1, observed_at: observed_at(),
        history_scope: "current logical history_base prefix chain; response ownership from stored thread IDs; no automatic child/fork expansion".to_owned(),
        attribution_basis: "explicit response model/effort when stored, otherwise preceding persisted same-turn step context; provider substitutions are not observable; legacy times are counter event times".to_owned(),
        modern_response_records: collected.modern, legacy_counter_delta_records: collected.legacy,
        duplicate_response_records_removed: collected.duplicates, suppressed_legacy_records: collected.suppressed,
        attribution_conflicts: collected.attribution_conflicts,
        untimed_records, untimed_tokens, missing_threads, sources, reconciliation, usage })
}
