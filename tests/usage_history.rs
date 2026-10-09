use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use teamy_codex::usage::history::{HistoryReport, query_history};
use teamy_codex::usage::{UsageGroup, UsageQuery};

const THREAD: &str = "11111111-1111-4111-8111-111111111111";
const SEGMENT: &str = "22222222-2222-4222-8222-222222222222";
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "teamy-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("sessions")).unwrap();
        Self(path)
    }
    fn file(&self, id: &str, data: &str) -> PathBuf {
        let path = self
            .0
            .join("sessions")
            .join(format!("rollout-2026-10-07T00-00-00-{id}.jsonl"));
        std::fs::write(&path, data).unwrap();
        path
    }
    fn db(&self, path: &Path, counter: i64) {
        let connection = rusqlite::Connection::open(self.0.join("state_5.sqlite")).unwrap();
        connection
            .execute_batch("CREATE TABLE threads(id TEXT, rollout_path TEXT, tokens_used INTEGER)")
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads VALUES(?1, ?2, ?3)",
                rusqlite::params![THREAD, path.to_str().unwrap(), counter],
            )
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn line(kind: &str, time: &str, payload: &str) -> String {
    format!(r#"{{"type":"{kind}","timestamp":"{time}","payload":{payload}}}"#) + "\n"
}
fn header(base: Option<(u64, u64)>) -> String {
    let base = base.map_or_else(|| "null".to_owned(), |(bytes, ordinal)| format!(r#"{{"thread_id":"{THREAD}","end_byte_offset":{bytes},"end_ordinal_exclusive":{ordinal}}}"#));
    line(
        "session_meta",
        "2026-10-07T00:00:00Z",
        &format!(r#"{{"id":"{THREAD}","history_base":{base},"cli_version":"synthetic"}}"#),
    )
}
fn context(model: &str) -> String {
    line(
        "turn_context",
        "2026-10-07T00:00:00Z",
        &format!(r#"{{"turn_id":"t","model":"{model}","effort":"medium"}}"#),
    )
}
fn tokens(n: i64) -> String {
    format!(
        r#"{{"input_tokens":{n},"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":{n}}}"#
    )
}
fn response(id: &str, n: i64, time: &str) -> String {
    line(
        "token_usage_record",
        time,
        &format!(
            r#"{{"thread_id":"{THREAD}","turn_id":"t","response_id":"{id}","usage":{}}}"#,
            tokens(n)
        ),
    )
}
fn counter(total: i64, last: i64) -> String {
    line(
        "event_msg",
        "2026-10-07T00:30:00Z",
        &format!(
            r#"{{"type":"token_count","info":{{"total_token_usage":{},"last_token_usage":{}}}}}"#,
            tokens(total),
            tokens(last)
        ),
    )
}
fn query() -> UsageQuery {
    UsageQuery {
        threads: vec![THREAD.to_owned()],
        group_by: vec![UsageGroup::Model, UsageGroup::Effort],
        ..UsageQuery::default()
    }
}

#[test]
fn prefix_chain_excludes_abandoned_tail_and_uses_request_context_changes() {
    let fixture = Fixture::new();
    let prefix = header(None)
        + &context("old-model")
        + &response("r1", 100, "2026-10-07T00:01:00Z")
        + &counter(100, 100);
    fixture.file(
        THREAD,
        &(prefix.clone() + &response("abandoned", 5000, "2026-10-07T00:02:00Z")),
    );
    let data = header(Some((u64::try_from(prefix.len()).unwrap(), 4)))
        + &context("new-model")
        + &response("r2", 20, "2026-10-07T01:00:00Z")
        + &counter(120, 20)
        + &response("r2", 20, "2026-10-07T01:00:01Z");
    let path = fixture.file(SEGMENT, &data);
    fixture.db(&path, 120);
    let report = query_history(&fixture.0, &query()).unwrap();
    assert_eq!(report.sources.len(), 2);
    assert_eq!(report.modern_response_records, 2);
    assert_eq!(report.duplicate_response_records_removed, 1);
    assert_eq!(report.legacy_counter_delta_records, 0);
    assert_eq!(report.usage.totals.total_tokens, 120);
    assert_eq!(report.usage.buckets.len(), 2);
    assert_eq!(report.reconciliation[0].database_minus_recorded, 0);
    assert!(report.sources.iter().all(|s| s.error.is_none()));
    let filtered = UsageQuery {
        models: vec!["new-model".to_owned()],
        since: Some("2026-10-07T00:59:59Z".to_owned()),
        until: Some("2026-10-07T01:00:01Z".to_owned()),
        ..query()
    };
    assert_eq!(
        query_history(&fixture.0, &filtered)
            .unwrap()
            .usage
            .totals
            .total_tokens,
        20
    );
}

#[test]
fn legacy_deltas_dedupe_repeated_snapshots_and_report_resets_without_forcing_totals() {
    let fixture = Fixture::new();
    let data = header(None)
        + &context("legacy")
        + &counter(100, 100)
        + &counter(100, 100)
        + &line(
            "compacted",
            "2026-10-07T00:20:00Z",
            r#"{"message":"CONVERSATION_SECRET"}"#,
        )
        + &counter(150, 50)
        + &counter(20, 20)
        + &counter(30, 10);
    let path = fixture.file(THREAD, &data);
    fixture.db(&path, 30);
    let report = query_history(&fixture.0, &query()).unwrap();
    assert_eq!(report.legacy_counter_delta_records, 3);
    assert_eq!(report.usage.totals.total_tokens, 160);
    assert_eq!(report.sources[0].counter_resets, 1);
    assert_eq!(report.sources[0].repeated_counter_events, 1);
    assert_eq!(report.sources[0].compactions, 1);
    assert_eq!(report.reconciliation[0].database_minus_recorded, -130);
}

#[test]
fn unknown_models_missing_times_corruption_and_append_tail_are_explicit_without_content() {
    let fixture = Fixture::new();
    let missing_time = format!(
        r#"{{"type":"token_usage_record","payload":{{"thread_id":"{THREAD}","turn_id":"t","response_id":"untimed","usage":{}}}}}"#,
        tokens(7)
    ) + "\n";
    let data = header(None)
        + &response("unknown", 100, "2026-10-07T00:01:00Z")
        + &missing_time
        + r#"{"type":"token_usage_record","payload":"PRIVATE_SECRET"}"#
        + "\n"
        + r#"{"type":"token_usage_record","payload":{"unfinished":"PRIVATE_SECRET"#;
    let path = fixture.file(THREAD, &data);
    fixture.db(&path, 107);
    let report = query_history(&fixture.0, &query()).unwrap();
    assert_eq!(report.usage.unknown_model_responses, 1);
    assert_eq!(report.usage.buckets[0].key.model, None);
    assert_eq!(report.untimed_records, 1);
    assert_eq!(report.untimed_tokens.total_tokens, 7);
    assert_eq!(report.reconciliation[0].database_minus_recorded, 0);
    assert_eq!(report.sources[0].malformed_metadata_lines, 1);
    assert_eq!(report.sources[0].incomplete_trailing_lines, 1);
    let output = Command::new(env!("CARGO_BIN_EXE_teamy-codex"))
        .args([
            "--output-format",
            "json",
            "--codex-home",
            fixture.0.to_str().unwrap(),
            "usage",
            "history",
            "--thread",
            THREAD,
            "--group-by",
            "model",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE_SECRET"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_SECRET"));
    let parsed: HistoryReport = facet_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed.usage.totals, report.usage.totals);
}

#[test]
fn missing_base_and_out_of_storage_path_are_reported_without_reading_arbitrary_files() {
    let fixture = Fixture::new();
    let data =
        header(Some((10, 4))) + &context("model") + &response("r", 10, "2026-10-07T00:01:00Z");
    let path = fixture.file(SEGMENT, &data);
    fixture.db(&path, 90);
    let report = query_history(&fixture.0, &query()).unwrap();
    assert!(
        report.sources[0]
            .error
            .as_deref()
            .unwrap()
            .contains("missing history-base")
    );
    assert_eq!(report.reconciliation[0].database_minus_recorded, 80);
    let outside = fixture.0.join("outside.jsonl");
    std::fs::write(&outside, "PRIVATE_OUTSIDE_CONTENT").unwrap();
    let db = rusqlite::Connection::open(fixture.0.join("state_5.sqlite")).unwrap();
    db.execute(
        "UPDATE threads SET rollout_path=?1",
        [outside.to_str().unwrap()],
    )
    .unwrap();
    drop(db);
    let report = query_history(&fixture.0, &query()).unwrap();
    assert!(
        report.sources[0]
            .error
            .as_deref()
            .unwrap()
            .contains("outside the selected")
    );
    assert_eq!(report.modern_response_records, 0);
}

#[test]
fn inherited_response_ownership_and_uncertain_legacy_fork_prefix_are_excluded() {
    let fixture = Fixture::new();
    let header = line(
        "session_meta",
        "2026-10-07T00:00:00Z",
        &format!(r#"{{"id":"{THREAD}","forked_from_id":"parent"}}"#),
    );
    let inherited = line(
        "token_usage_record",
        "2026-10-07T00:01:00Z",
        &format!(
            r#"{{"thread_id":"parent","turn_id":"t","response_id":"parent-r","usage":{}}}"#,
            tokens(500)
        ),
    );
    let data = header
        + &context("child")
        + &inherited
        + &counter(500, 500)
        + &response("own", 10, "2026-10-07T00:02:00Z");
    let path = fixture.file(THREAD, &data);
    fixture.db(&path, 510);
    let report = query_history(&fixture.0, &query()).unwrap();
    assert_eq!(report.usage.totals.total_tokens, 10);
    assert_eq!(report.sources[0].inherited_records_excluded, 2);
    assert_eq!(report.reconciliation[0].database_minus_recorded, 500);
}

#[test]
fn truncated_prefix_and_missing_thread_coverage_are_visible() {
    let fixture = Fixture::new();
    let prefix = header(None) + &context("old") + &response("r1", 10, "2026-10-07T00:01:00Z");
    fixture.file(THREAD, &prefix);
    let path = fixture.file(
        SEGMENT,
        &header(Some((u64::try_from(prefix.len()).unwrap() + 100, 99))),
    );
    fixture.db(&path, 10);
    let report = query_history(&fixture.0, &query()).unwrap();
    assert!(report.sources.iter().any(|s| s.prefix_truncated));
    let empty = query_history(
        &fixture.0,
        &UsageQuery {
            threads: vec!["missing".to_owned()],
            ..UsageQuery::default()
        },
    )
    .unwrap();
    assert_eq!(empty.missing_threads, ["missing"]);
    assert_eq!(empty.usage.matched_responses, 0);
}

#[test]
fn conflicting_replay_attribution_becomes_unknown_and_conflicting_usage_fails() {
    let fixture = Fixture::new();
    let data = header(None)
        + &context("a")
        + &response("r", 10, "2026-10-07T00:01:00Z")
        + &context("b")
        + &response("r", 10, "2026-10-07T00:02:00Z");
    let path = fixture.file(THREAD, &data);
    fixture.db(&path, 10);
    let report = query_history(&fixture.0, &query()).unwrap();
    assert_eq!(report.attribution_conflicts, 1);
    assert_eq!(report.usage.unknown_model_responses, 1);
    assert_eq!(report.usage.totals.total_tokens, 10);
    std::fs::write(&path, data + &response("r", 11, "2026-10-07T00:03:00Z")).unwrap();
    assert!(
        query_history(&fixture.0, &query())
            .unwrap_err()
            .to_string()
            .contains("conflicting native response")
    );
}

#[test]
fn oversized_lines_are_streamed_past_and_private_header_errors_are_sanitized() {
    let fixture = Fixture::new();
    let data = header(None)
        + &"x".repeat(32 * 1024 * 1024 + 1)
        + "\n"
        + &context("known")
        + &response("r", 10, "2026-10-07T00:01:00Z");
    let path = fixture.file(THREAD, &data);
    fixture.db(&path, 10);
    let report = query_history(&fixture.0, &query()).unwrap();
    assert_eq!(report.sources[0].oversized_lines, 1);
    assert_eq!(report.usage.totals.total_tokens, 10);
    std::fs::write(
        &path,
        r#"{"type":"session_meta","payload":"PRIVATE_HEADER_SECRET"}"#,
    )
    .unwrap();
    let report = query_history(&fixture.0, &query()).unwrap();
    assert!(
        report.sources[0]
            .error
            .as_deref()
            .unwrap()
            .contains("metadata header")
    );
    assert!(
        !facet_json::to_string(&report)
            .unwrap()
            .contains("PRIVATE_HEADER_SECRET")
    );
}
