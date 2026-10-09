//! Current native metadata only; no transcript/rollout file access.
use super::{UsageGroup, UsageQuery, observed_at};
use eyre::{Context, bail};
use facet::Facet;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub thread_id: String,
    pub cumulative_tokens: i64,
    /// Latest metadata setting, NOT attribution of `cumulative_tokens`.
    pub last_observed_model: Option<String>,
    /// Latest metadata setting, NOT historical attribution.
    pub last_observed_effort: Option<String>,
    pub archived: bool,
}

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReport {
    pub schema_version: u32,
    pub observed_at: String,
    pub source: String,
    pub scope: String,
    pub historical_attribution: String,
    pub lineage_coverage: String,
    pub query: UsageQuery,
    pub snapshots: Vec<UsageSnapshot>,
}

/// Query all matching native thread counters without reading rollout files.
/// No combined grand total: forks/children may have overlapping counters.
/// # Errors
/// Rejects historical model/time/group filters and missing/unsupported databases.
pub fn query_snapshots(codex_home: &Path, query: &UsageQuery) -> eyre::Result<SnapshotReport> {
    query.bounds()?;
    if !query.models.is_empty()
        || query.since.is_some()
        || query.until.is_some()
        || query
            .group_by
            .iter()
            .any(|group| *group != UsageGroup::Thread)
    {
        bail!(
            "native snapshots cannot attribute cumulative tokens by historical model/time/effort; use usage records with a request-usage export"
        );
    }
    let database = codex_home.join("state_5.sqlite");
    let connection = Connection::open_with_flags(
        &database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .wrap_err_with(|| {
        format!(
            "cannot open native usage database read-only: {}",
            database.display()
        )
    })?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    let mut columns_stmt = connection.prepare("PRAGMA table_info(threads)")?;
    let columns = columns_stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for required in ["id", "tokens_used"] {
        if !columns.iter().any(|column| column == required) {
            bail!("unsupported native threads schema: missing {required}");
        }
    }
    let optional = |name: &str, fallback: &str| {
        if columns.iter().any(|column| column == name) {
            name.to_owned()
        } else {
            fallback.to_owned()
        }
    };
    let sql = format!(
        "SELECT id, tokens_used, {}, {}, {} FROM threads ORDER BY id",
        optional("model", "NULL"),
        optional("reasoning_effort", "NULL"),
        optional("archived", "0")
    );
    let mut stmt = connection.prepare(&sql)?;
    let all = stmt
        .query_map([], |row| {
            Ok(UsageSnapshot {
                thread_id: row.get(0)?,
                cumulative_tokens: row.get(1)?,
                last_observed_model: row.get(2)?,
                last_observed_effort: row.get(3)?,
                archived: row.get::<_, i64>(4)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let snapshots = all
        .into_iter()
        .filter(|row| query.threads.is_empty() || query.threads.contains(&row.thread_id))
        .collect::<Vec<_>>();
    if snapshots.iter().any(|row| row.cumulative_tokens < 0) {
        bail!("native snapshot has a negative token counter");
    }
    Ok(SnapshotReport {
        schema_version: 1,
        observed_at: observed_at(),
        source: database.display().to_string(),
        scope:
            "latest persisted cumulative snapshot per thread; not interval usage or current context"
                .to_owned(),
        historical_attribution: "unavailable; latest model/effort are labels only".to_owned(),
        lineage_coverage:
            "unknown compaction/reset/fork/child coverage; no cross-thread grand total".to_owned(),
        query: query.clone(),
        snapshots,
    })
}
