//! Read-only usage queries. Snapshot counters never imply historical attribution.
pub mod history;
pub mod records;
pub mod snapshots;

use arbitrary::Arbitrary;
use chrono::{DateTime, Utc};
use eyre::{Context, bail};
use facet::Facet;

#[derive(Facet, Arbitrary, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[facet(rename_all = "kebab-case")]
#[repr(u8)]
pub enum UsageGroup {
    Thread,
    Model,
    Effort,
    Day,
}

/// Filters apply to individual completed responses before aggregation.
/// Times are RFC3339 with explicit offsets, inclusive since and exclusive until.
#[derive(Facet, Arbitrary, Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageQuery {
    pub threads: Vec<String>,
    pub models: Vec<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub group_by: Vec<UsageGroup>,
}

pub(crate) struct TimeBounds {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

impl UsageQuery {
    pub(crate) fn bounds(&self) -> eyre::Result<TimeBounds> {
        let since = self.since.as_deref().map(parse_time).transpose()?;
        let until = self.until.as_deref().map(parse_time).transpose()?;
        if let (Some(start), Some(end)) = (since, until)
            && start >= end
        {
            bail!("--since must precede --until (half-open interval)");
        }
        if self
            .threads
            .iter()
            .chain(&self.models)
            .any(|s| s.trim().is_empty())
        {
            bail!("thread/model filters cannot be empty");
        }
        let mut groups = self.group_by.clone();
        groups.sort();
        groups.dedup();
        if groups.len() != self.group_by.len() {
            bail!("duplicate group-by dimension");
        }
        Ok(TimeBounds { since, until })
    }
}

pub(crate) fn parse_time(value: &str) -> eyre::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .wrap_err_with(|| format!("invalid RFC3339 timestamp with explicit UTC offset: {value}"))
}

pub(crate) fn observed_at() -> String {
    Utc::now().to_rfc3339()
}
