use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use teamy_codex::usage::records::{
    UsageExport, UsageRecord, UsageReport, UsageTokens, query_usage_records,
};
use teamy_codex::usage::snapshots::{SnapshotReport, query_snapshots};
use teamy_codex::usage::{UsageGroup, UsageQuery};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "teamy-codex-usage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn export(&self, export: &UsageExport) -> PathBuf {
        let path = self.0.join("usage.json");
        std::fs::write(&path, facet_json::to_string(export).unwrap()).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn tokens(input: i64, output: i64) -> UsageTokens {
    UsageTokens {
        input_tokens: input,
        cached_input_tokens: input / 2,
        output_tokens: output,
        reasoning_output_tokens: output / 2,
        total_tokens: input + output,
    }
}
fn record(id: &str, thread: &str, at: &str, model: Option<&str>, input: i64) -> UsageRecord {
    UsageRecord {
        response_id: id.to_owned(),
        thread_id: thread.to_owned(),
        completed_at: at.to_owned(),
        effective_model: model.map(str::to_owned),
        effective_effort: model.map(|_| "medium".to_owned()),
        tokens: tokens(input, 10),
    }
}
fn export() -> UsageExport {
    let a = record("r1", "sfm", "2026-10-07T00:00:00Z", Some("model-a"), 100);
    UsageExport {
        schema_version: 1,
        accounting: "response-delta".to_owned(),
        provenance: "synthetic requests".to_owned(),
        coverage: "fixture only".to_owned(),
        records: vec![
            a.clone(),
            a,
            record(
                "r2",
                "sfm",
                "2026-10-07T01:00:00+01:00",
                Some("model-b"),
                200,
            ),
            record("r3", "sfm", "2026-10-07T00:30:00Z", None, 300),
            record("r4", "other", "2026-10-07T00:15:00Z", Some("model-a"), 400),
            record("r5", "sfm", "2026-10-08T00:00:00Z", Some("model-a"), 500),
        ],
    }
}
fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_teamy-codex"))
        .args(args)
        .output()
        .unwrap()
}
fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn sdk_filters_before_grouping_deduplicates_and_preserves_unknowns() {
    let query = UsageQuery {
        threads: vec!["sfm".to_owned()],
        since: Some("2026-10-06T20:00:00-04:00".to_owned()),
        until: Some("2026-10-08T00:00:00Z".to_owned()),
        group_by: vec![UsageGroup::Model, UsageGroup::Effort, UsageGroup::Day],
        ..UsageQuery::default()
    };
    let report = query_usage_records(&export(), &query).unwrap();
    assert_eq!(
        (
            report.records_read,
            report.unique_responses,
            report.duplicates_removed,
            report.matched_responses
        ),
        (6, 5, 1, 3)
    );
    assert_eq!(report.totals, tokens(600, 30));
    assert_eq!(
        (
            report.unknown_model_responses,
            report.unknown_effort_responses
        ),
        (1, 1)
    );
    assert_eq!(report.buckets.len(), 3);
    assert!(
        report
            .buckets
            .iter()
            .any(|b| b.key.model.is_none() && b.tokens.total_tokens == 310)
    );
    assert!(
        report
            .buckets
            .iter()
            .all(|b| b.key.day_utc.as_deref() == Some("2026-10-07"))
    );
}
#[test]
fn subprocess_filters_exact_models_and_reports_unknown_coverage() {
    let fixture = Fixture::new();
    let input = fixture.export(&export());
    let result = cli(&[
        "--output-format",
        "json",
        "usage",
        "records",
        "--input",
        path_str(&input),
        "--thread",
        "sfm",
        "--model",
        "model-a",
        "--model",
        "model-b",
        "--since",
        "2026-10-07T00:00:00Z",
        "--until",
        "2026-10-08T00:00:00Z",
        "--group-by",
        "thread",
        "--group-by",
        "model",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: UsageReport = facet_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report.matched_responses, 2);
    assert_eq!(report.totals, tokens(300, 20));
    assert_eq!(report.unknown_model_excluded_by_filter, 1);
    assert_eq!(report.buckets.len(), 2);
    assert!(report.completeness.contains("not independently verified"));
}
#[test]
fn conflicting_copies_fail_even_when_filters_would_exclude_them() {
    let mut data = export();
    let mut conflicting = data.records[0].clone();
    conflicting.thread_id = "copied-fork".to_owned();
    data.records.push(conflicting);
    let query = UsageQuery {
        models: vec!["does-not-match".to_owned()],
        ..UsageQuery::default()
    };
    assert!(
        query_usage_records(&data, &query)
            .unwrap_err()
            .to_string()
            .contains("conflicting duplicate")
    );
}
#[test]
fn discrete_requests_survive_resets_but_cumulative_import_is_rejected() {
    let mut data = export();
    data.records = vec![
        record("before", "sfm", "2026-10-07T00:00:00Z", None, 900),
        record("after", "sfm", "2026-10-07T01:00:00Z", None, 20),
    ];
    assert_eq!(
        query_usage_records(&data, &UsageQuery::default())
            .unwrap()
            .totals
            .total_tokens,
        940
    );
    data.accounting = "cumulative-snapshot".to_owned();
    assert!(
        query_usage_records(&data, &UsageQuery::default())
            .unwrap_err()
            .to_string()
            .contains("reset inference")
    );
}
#[test]
fn timestamp_bounds_and_invalid_token_accounting_fail() {
    for query in [
        UsageQuery {
            since: Some("2026-10-07T00:00:00".to_owned()),
            ..UsageQuery::default()
        },
        UsageQuery {
            since: Some("2026-10-08T00:00:00Z".to_owned()),
            until: Some("2026-10-07T00:00:00Z".to_owned()),
            ..UsageQuery::default()
        },
    ] {
        let _error =
            query_usage_records(&export(), &query).expect_err("invalid timestamp query must fail");
    }
    let query = UsageQuery {
        since: Some("2026-10-07T00:00:00Z".to_owned()),
        until: Some("2026-10-07T00:30:00Z".to_owned()),
        threads: vec!["sfm".to_owned()],
        ..UsageQuery::default()
    };
    assert_eq!(
        query_usage_records(&export(), &query)
            .unwrap()
            .matched_responses,
        2
    );
    let mut data = export();
    data.records[0].tokens.cached_input_tokens = 101;
    let _error = query_usage_records(&data, &UsageQuery::default())
        .expect_err("invalid token accounting must fail");
    data.records = vec![record("a", "sfm", "2026-10-07T00:00:00Z", None, 1)];
    data.records[0].tokens.total_tokens = 99;
    let _error = query_usage_records(&data, &UsageQuery::default())
        .expect_err("invalid token accounting must fail");
    data.records[0].tokens = tokens(i64::MAX, 0);
    let mut second = data.records[0].clone();
    second.response_id = "b".to_owned();
    data.records.push(second);
    assert!(
        query_usage_records(&data, &UsageQuery::default())
            .unwrap_err()
            .to_string()
            .contains("overflow")
    );
}
#[test]
fn subprocess_snapshots_are_read_only_and_never_allocate_latest_model() {
    let fixture = Fixture::new();
    let db = fixture.0.join("state_5.sqlite");
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("CREATE TABLE threads(id TEXT, tokens_used INTEGER, model TEXT, reasoning_effort TEXT, archived INTEGER);
        INSERT INTO threads VALUES('sfm', 4953496943, 'latest-only', 'medium', 0);
        INSERT INTO threads VALUES('other', 7, NULL, NULL, 1);").unwrap();
    drop(connection);
    let before = std::fs::read(&db).unwrap();
    let result = cli(&[
        "--output-format",
        "json",
        "--codex-home",
        path_str(&fixture.0),
        "usage",
        "snapshots",
        "--thread",
        "sfm",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: SnapshotReport = facet_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report.snapshots.len(), 1);
    assert_eq!(report.snapshots[0].cumulative_tokens, 4_953_496_943);
    assert!(report.historical_attribution.starts_with("unavailable"));
    assert_eq!(std::fs::read(&db).unwrap(), before);
    for args in [
        vec!["--model", "latest-only"],
        vec!["--since", "2026-10-07T00:00:00Z"],
        vec!["--group-by", "model"],
    ] {
        let mut command = vec!["--codex-home", path_str(&fixture.0), "usage", "snapshots"];
        command.extend(args);
        let failure = cli(&command);
        assert!(!failure.status.success());
        assert!(String::from_utf8_lossy(&failure.stderr).contains("cannot attribute"));
    }
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute("UPDATE threads SET tokens_used=12 WHERE id='sfm'", [])
        .unwrap();
    drop(connection);
    let report = query_snapshots(
        &fixture.0,
        &UsageQuery {
            threads: vec!["sfm".to_owned()],
            ..UsageQuery::default()
        },
    )
    .unwrap();
    assert_eq!(report.snapshots[0].cumulative_tokens, 12);
}
#[test]
fn missing_database_is_not_created_and_raw_sessions_are_rejected() {
    let fixture = Fixture::new();
    let _error = query_snapshots(&fixture.0, &UsageQuery::default())
        .expect_err("missing snapshot database must fail");
    assert!(!fixture.0.join("state_5.sqlite").exists());
    let input = fixture.0.join("raw.json");
    std::fs::write(
        &input,
        r#"{"type":"event_msg","payload":{"type":"token_count"}}"#,
    )
    .unwrap();
    let failure = cli(&["usage", "records", "--input", path_str(&input)]);
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("not raw session"));
}
