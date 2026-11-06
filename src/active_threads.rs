//! Read-only observation of one explicitly selected app-server runtime.
use crate::codex_app_server_stdio::AppServerStdio;
use eyre::bail;
use facet::Facet;
use std::io::{BufRead, Write};

const PAGE_LIMIT: u32 = 100;
const MAX_PAGES: usize = 100;
const MAX_HISTORY_PAGES: usize = 4;

#[derive(Debug)]
pub struct ActiveThreadsRequest {
    pub active_only: bool,
    pub limit: Option<usize>,
    pub isolated: bool,
    pub archive_filter: ArchiveFilter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveFilter {
    NonArchived,
    Archived,
    All,
}

#[derive(Facet, Debug)]
pub struct ActiveThreadsReport {
    pub data: Vec<WorkingThread>,
    pub coverage: Coverage,
}

#[derive(Facet, Debug)]
pub struct Coverage {
    pub source: String,
    pub connection: String,
    pub scope: String,
    pub complete: bool,
    pub observed_at_ms: i64,
    pub staleness: String,
    pub unknown_threads: usize,
    pub truncated: usize,
    pub issues: Vec<String>,
}

#[derive(Facet, Debug)]
pub struct WorkingThread {
    pub id: String,
    pub title: String,
    pub status: String,
    pub last_activity_ms: Option<i64>,
    pub observed_at_ms: i64,
    pub latest_command: Option<LatestCommand>,
    pub latest_turn_status: Option<String>,
    pub latest_command_availability: String,
}

#[derive(Facet, Debug)]
pub struct LatestCommand {
    pub id: String,
    pub display: String,
    pub status: Option<String>,
    pub started_at_ms: Option<i64>,
    pub arguments_redacted: bool,
}

#[derive(Facet)]
struct ListParams<'a> {
    limit: u32,
    cursor: Option<&'a str>,
    #[facet(rename = "sourceKinds")]
    source_kinds: Vec<&'a str>,
    #[facet(rename = "useStateDbOnly")]
    use_state_db_only: bool,
    archived: bool,
}

#[derive(Facet)]
struct Page {
    data: Vec<Thread>,
    #[facet(rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Facet)]
struct LoadedPage {
    data: Vec<String>,
    #[facet(rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Facet)]
struct LoadedParams<'a> {
    limit: u32,
    cursor: Option<&'a str>,
}

#[derive(Facet)]
struct ReadParams<'a> {
    #[facet(rename = "threadId")]
    thread_id: &'a str,
    #[facet(rename = "includeTurns")]
    include_turns: bool,
}

#[derive(Facet)]
struct ReadResponse {
    thread: Thread,
}

#[derive(Facet)]
struct Thread {
    id: String,
    name: Option<String>,
    status: Option<RuntimeStatus>,
    #[facet(rename = "createdAt")]
    created_at: Option<i64>,
    #[facet(rename = "updatedAt")]
    updated_at: Option<i64>,
    #[facet(rename = "recencyAt")]
    recency_at: Option<i64>,
}

#[derive(Facet)]
struct RuntimeStatus {
    #[facet(rename = "type")]
    kind: String,
    #[facet(rename = "activeFlags")]
    active_flags: Option<Vec<String>>,
}

#[derive(Facet)]
struct ItemsParams<'a> {
    #[facet(rename = "threadId")]
    thread_id: &'a str,
    cursor: Option<&'a str>,
    limit: u32,
    #[facet(rename = "sortDirection")]
    sort_direction: &'a str,
}

#[derive(Facet)]
struct TurnsPage {
    data: Vec<Turn>,
}
#[derive(Facet)]
struct Turn {
    status: String,
}

#[derive(Facet)]
struct ItemsPage {
    data: Vec<ItemEntry>,
    #[facet(rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Facet)]
struct ItemEntry {
    item: Item,
    #[facet(rename = "startedAtMs")]
    started_at_ms: Option<i64>,
}

#[derive(Facet)]
struct Item {
    #[facet(rename = "type")]
    kind: String,
    id: String,
    command: Option<String>,
    status: Option<String>,
}

impl Coverage {
    fn new(isolated: bool) -> Self {
        Self {
            source: if isolated {
                "isolated_app_server"
            } else {
                "shared_app_server_proxy"
            }
            .into(),
            connection: "connected".into(),
            scope: "selected runtime only; excludes other desktop/remote runtimes".into(),
            complete: !isolated,
            observed_at_ms: chrono::Utc::now().timestamp_millis(),
            staleness: "point-in-time snapshot; may change immediately".into(),
            unknown_threads: 0,
            truncated: 0,
            issues: if isolated {
                vec!["isolated server cannot observe desktop-owned work".into()]
            } else {
                vec![]
            },
        }
    }
}

impl ActiveThreadsReport {
    #[must_use]
    pub fn disconnected(isolated: bool) -> Self {
        let mut coverage = Coverage::new(isolated);
        coverage.connection = "disconnected".into();
        coverage.complete = false;
        coverage.issues.push(
            "selected app-server unavailable or request failed; no activity conclusion possible"
                .into(),
        );
        Self {
            data: vec![],
            coverage,
        }
    }
}

/// Inspect metadata and bounded command history through supported RPCs only.
///
/// # Errors
/// Returns an error on invalid requests or a failed metadata query.
pub fn inspect<R: BufRead, W: Write>(
    client: &mut AppServerStdio<R, W>,
    request: &ActiveThreadsRequest,
) -> eyre::Result<ActiveThreadsReport> {
    if request.limit == Some(0) {
        bail!("--limit must be greater than zero");
    }
    let mut report = ActiveThreadsReport {
        data: vec![],
        coverage: Coverage::new(request.isolated),
    };
    collect_persisted(client, request, &mut report)?;
    // Loaded sessions may be ephemeral or absent from persisted/sidebar metadata.
    if !request.isolated
        && request.archive_filter != ArchiveFilter::Archived
        && enrich_loaded(client, &mut report).is_err()
    {
        report.coverage.complete = false;
        report
            .coverage
            .issues
            .push("loaded-session RPC unavailable; persisted metadata only".into());
    }
    let mut seen_ids = std::collections::HashSet::new();
    report
        .data
        .retain(|thread| seen_ids.insert(thread.id.clone()));
    report.coverage.unknown_threads = report
        .data
        .iter()
        .filter(|thread| thread.status == "unknown")
        .count();
    if request.active_only {
        report.data.retain(|thread| is_active(&thread.status));
    }
    report.data.sort_by(|a, b| {
        b.last_activity_ms
            .cmp(&a.last_activity_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    if let Some(limit) = request.limit {
        report.coverage.truncated = report.data.len().saturating_sub(limit);
        report.data.truncate(limit);
        if report.coverage.truncated > 0 {
            report.coverage.complete = false;
        }
    }
    enrich_history(client, &mut report);
    if report.coverage.unknown_threads > 0 {
        report.coverage.complete = false;
        report
            .coverage
            .issues
            .push("unloaded or unrecognized statuses remain unknown; excluded by --active".into());
    }
    Ok(report)
}

fn collect_persisted<R: BufRead, W: Write>(
    client: &mut AppServerStdio<R, W>,
    request: &ActiveThreadsRequest,
    report: &mut ActiveThreadsReport,
) -> eyre::Result<()> {
    let archives = match request.archive_filter {
        ArchiveFilter::NonArchived => vec![false],
        ArchiveFilter::Archived => vec![true],
        ArchiveFilter::All => vec![false, true],
    };
    for archived in archives {
        let mut cursor = None;
        let mut seen = std::collections::HashSet::new();
        for page_index in 0..MAX_PAGES {
            let page: Page = client.observe_rpc(
                "thread/list",
                ListParams {
                    limit: PAGE_LIMIT,
                    cursor: cursor.as_deref(),
                    source_kinds: vec![
                        "cli",
                        "vscode",
                        "exec",
                        "appServer",
                        "subAgent",
                        "subAgentReview",
                        "subAgentCompact",
                        "subAgentThreadSpawn",
                        "subAgentOther",
                        "unknown",
                    ],
                    use_state_db_only: true,
                    archived,
                },
            )?;
            if page.data.len() > PAGE_LIMIT as usize {
                bail!("thread/list exceeded page limit");
            }
            for thread in page.data {
                report.data.push(project_thread(thread, request.isolated));
            }
            cursor = page.next_cursor;
            let Some(next) = cursor.as_ref() else {
                break;
            };
            if !seen.insert(next.clone()) {
                bail!("thread/list repeated a cursor");
            }
            if page_index + 1 == MAX_PAGES {
                report.coverage.complete = false;
                report
                    .coverage
                    .issues
                    .push("thread pagination capped at 100 pages".into());
            }
        }
    }
    Ok(())
}

fn enrich_history<R: BufRead, W: Write>(
    client: &mut AppServerStdio<R, W>,
    report: &mut ActiveThreadsReport,
) {
    for thread in &mut report.data {
        if let Ok(page) = client.observe_rpc::<_, TurnsPage>(
            "thread/turns/list",
            ItemsParams {
                thread_id: &thread.id,
                cursor: None,
                limit: 1,
                sort_direction: "desc",
            },
        ) {
            thread.latest_turn_status = page.data.first().map(|turn| clean_text(&turn.status, 40));
        }
        match latest_command(client, &thread.id) {
            Ok((command, availability)) => {
                thread.latest_command = command;
                thread.latest_command_availability = availability.into();
            }
            Err(_) => {
                thread.latest_command_availability =
                    "unavailable: history RPC rejected or unsupported".into();
            }
        }
    }
}

fn project_thread(thread: Thread, isolated: bool) -> WorkingThread {
    WorkingThread {
        status: project_status(thread.status.as_ref(), isolated),
        id: thread.id,
        title: clean_text(thread.name.as_deref().unwrap_or("(untitled)"), 200),
        last_activity_ms: [thread.created_at, thread.updated_at, thread.recency_at]
            .into_iter()
            .flatten()
            .max()
            .and_then(|v| v.checked_mul(1000)),
        observed_at_ms: chrono::Utc::now().timestamp_millis(),
        latest_command: None,
        latest_turn_status: None,
        latest_command_availability: "unavailable".into(),
    }
}

fn enrich_loaded<R: BufRead, W: Write>(
    client: &mut AppServerStdio<R, W>,
    report: &mut ActiveThreadsReport,
) -> eyre::Result<()> {
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..MAX_PAGES {
        let page: LoadedPage = client.observe_rpc(
            "thread/loaded/list",
            LoadedParams {
                limit: PAGE_LIMIT,
                cursor: cursor.as_deref(),
            },
        )?;
        if page.data.len() > PAGE_LIMIT as usize {
            bail!("loaded list exceeded page limit");
        }
        for id in page.data {
            match client.observe_rpc::<_, ReadResponse>(
                "thread/read",
                ReadParams {
                    thread_id: &id,
                    include_turns: false,
                },
            ) {
                Ok(response) if response.thread.id == id => {
                    let row = project_thread(response.thread, false);
                    if let Some(existing) = report.data.iter_mut().find(|thread| thread.id == id) {
                        *existing = row;
                    } else {
                        report.data.push(row);
                    }
                }
                _ => {
                    report.coverage.complete = false;
                    report
                        .coverage
                        .issues
                        .push("one loaded-session metadata read failed; status unknown".into());
                    if let Some(row) = report.data.iter_mut().find(|thread| thread.id == id) {
                        row.status = "unknown".into();
                    } else {
                        report.data.push(WorkingThread {
                            id,
                            title: "(unavailable)".into(),
                            status: "unknown".into(),
                            last_activity_ms: None,
                            observed_at_ms: chrono::Utc::now().timestamp_millis(),
                            latest_command: None,
                            latest_turn_status: None,
                            latest_command_availability: "unavailable".into(),
                        });
                    }
                }
            }
        }
        cursor = page.next_cursor;
        let Some(next) = cursor.as_ref() else {
            return Ok(());
        };
        if !seen.insert(next.clone()) {
            bail!("loaded list repeated a cursor");
        }
    }
    bail!("loaded pagination capped")
}

fn latest_command<R: BufRead, W: Write>(
    client: &mut AppServerStdio<R, W>,
    id: &str,
) -> eyre::Result<(Option<LatestCommand>, &'static str)> {
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..MAX_HISTORY_PAGES {
        let page: ItemsPage = client.observe_rpc(
            "thread/items/list",
            ItemsParams {
                thread_id: id,
                cursor: cursor.as_deref(),
                limit: PAGE_LIMIT,
                sort_direction: "desc",
            },
        )?;
        if page.data.len() > PAGE_LIMIT as usize {
            bail!("history exceeded page limit");
        }
        // Descending canonical item order is authoritative even when timestamps are absent.
        for entry in page.data {
            if entry.item.kind == "commandExecution" {
                let Some(command) = entry.item.command else {
                    return Ok((None, "unavailable: command field absent"));
                };
                return Ok((
                    Some(LatestCommand {
                        id: entry.item.id,
                        display: safe_command(&command),
                        status: entry.item.status,
                        started_at_ms: entry.started_at_ms,
                        arguments_redacted: true,
                    }),
                    "available",
                ));
            }
        }
        cursor = page.next_cursor;
        let Some(next) = cursor.as_ref() else {
            return Ok((None, "none recorded"));
        };
        if !seen.insert(next.clone()) {
            bail!("history repeated a cursor");
        }
    }
    Ok((None, "unavailable: history search capped at 400 items"))
}

fn project_status(status: Option<&RuntimeStatus>, isolated: bool) -> String {
    if isolated {
        return "unknown".into();
    }
    let Some(status) = status else {
        return "unknown".into();
    };
    match status.kind.as_str() {
        "active" => {
            let flags = status.active_flags.as_deref().unwrap_or_default();
            if flags.iter().any(|v| v == "waitingOnApproval") {
                "waiting_approval"
            } else if flags.iter().any(|v| v == "waitingOnUserInput") {
                "waiting_input"
            } else if flags
                .iter()
                .all(|v| matches!(v.as_str(), "waitingOnApproval" | "waitingOnUserInput"))
            {
                "running"
            } else {
                "unknown"
            }
        }
        "idle" => "idle",
        "systemError" => "system_error",
        // NotLoaded is not evidence of idle or completion in another runtime.
        _ => "unknown",
    }
    .into()
}

#[must_use]
pub fn is_active(status: &str) -> bool {
    matches!(status, "running" | "waiting_approval" | "waiting_input")
}

/// Fail closed: arbitrary scripts and all argument values are withheld.
#[must_use]
pub fn safe_command(command: &str) -> String {
    let mut words = command.split_whitespace();
    let executable = words.next().unwrap_or_default();
    let allowed = [
        "cargo",
        "git",
        "rg",
        "rustc",
        "rustup",
        "pwsh",
        "powershell",
        "python",
        "python3",
        "uv",
        "node",
        "npm",
        "codex",
        "teamy-codex",
    ];
    if !allowed.contains(&executable) {
        return "[command withheld; arguments redacted]".into();
    }
    let verb = words.next().unwrap_or_default();
    let safe_verb = match executable {
        "cargo" => ["test", "build", "check", "clippy", "fmt"].contains(&verb),
        "git" => ["status", "diff", "log", "show"].contains(&verb),
        _ => false,
    };
    if safe_verb {
        format!("{executable} {verb} [arguments redacted]")
    } else {
        format!("{executable} [arguments redacted]")
    }
}

#[must_use]
pub fn clean_text(text: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut used = 0;
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= width
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn lifecycle_is_explicit_and_never_uses_recency() {
        for (kind, expected) in [
            ("idle", "idle"),
            ("notLoaded", "unknown"),
            ("systemError", "system_error"),
            ("completed", "unknown"),
            ("active", "running"),
        ] {
            let status = RuntimeStatus {
                kind: kind.into(),
                active_flags: None,
            };
            assert_eq!(project_status(Some(&status), false), expected);
            assert_eq!(project_status(Some(&status), true), "unknown");
        }
        let status = RuntimeStatus {
            kind: "active".into(),
            active_flags: Some(vec!["waitingOnApproval".into()]),
        };
        assert_eq!(project_status(Some(&status), false), "waiting_approval");
        assert!(is_active("waiting_approval"));
        assert!(!is_active("unknown"));
        assert!(!is_active("idle"));
    }

    #[test]
    fn commands_fail_closed_for_secrets_and_scripts() {
        for input in [
            "cargo test --token secret",
            "git diff https://user:password@host",
            "pwsh -c '$env:KEY=secret'",
            "curl -H 'Authorization: Bearer secret'",
            "SECRET=secret cargo test",
            "cargo test;secret",
            "cargo\x1b[2J secret",
        ] {
            let display = safe_command(input);
            assert!(!display.contains("secret"));
            assert!(!display.contains("password"));
            assert!(!display.contains('\x1b'));
            assert!(display.len() < 100);
        }
        assert_eq!(
            safe_command("cargo test --all-features"),
            "cargo test [arguments redacted]"
        );
    }

    #[test]
    fn task_sources_filtering_latest_order_and_partial_coverage() {
        let input = concat!(
            "{\"id\":1,\"result\":{\"data\":[{\"id\":\"task\",\"name\":\"Hidden task\",\"status\":{\"type\":\"active\",\"activeFlags\":[]},\"updatedAt\":10},{\"id\":\"unknown\",\"status\":{\"type\":\"notLoaded\"},\"updatedAt\":999},{\"id\":\"idle\",\"status\":{\"type\":\"idle\"}}]}}\n",
            "{\"id\":2,\"result\":{\"data\":[]}}\n",
            "{\"id\":3,\"result\":{\"data\": [{\"status\":\"completed\"}]}}\n",
            "{\"id\":4,\"result\":{\"data\":[{\"item\":{\"type\":\"agentMessage\",\"id\":\"message\"}},{\"item\":{\"type\":\"commandExecution\",\"id\":\"new\",\"command\":\"cargo test --token secret\",\"status\":\"inProgress\"},\"startedAtMs\":1},{\"item\":{\"type\":\"commandExecution\",\"id\":\"old\",\"command\":\"git status\"},\"startedAtMs\":999}]}}\n"
        );
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        let report = inspect(
            &mut client,
            &ActiveThreadsRequest {
                active_only: true,
                limit: None,
                isolated: false,
                archive_filter: ArchiveFilter::NonArchived,
            },
        )
        .unwrap();
        assert_eq!(report.data.len(), 1);
        assert_eq!(report.data[0].id, "task");
        assert_eq!(report.data[0].status, "running");
        assert_eq!(
            report.data[0].latest_turn_status.as_deref(),
            Some("completed")
        );
        assert_eq!(report.data[0].latest_command.as_ref().unwrap().id, "new");
        assert_eq!(report.coverage.unknown_threads, 1);
        assert!(!report.coverage.complete);
    }

    #[test]
    fn loaded_task_absent_from_sidebar_is_included() {
        let input = concat!(
            "{\"id\":1,\"result\":{\"data\":[]}}\n",
            "{\"id\":2,\"result\":{\"data\":[\"hidden\"]}}\n",
            "{\"id\":3,\"result\":{\"thread\":{\"id\":\"hidden\",\"name\":\"Task\",\"status\":{\"type\":\"active\",\"activeFlags\":[\"waitingOnUserInput\"]}}}}\n",
            "{\"id\":4,\"result\":{\"data\":[{\"status\":\"inProgress\"}]}}\n",
            "{\"id\":5,\"error\":{\"code\":-32601,\"message\":\"secret server error\"}}\n"
        );
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        let report = inspect(
            &mut client,
            &ActiveThreadsRequest {
                active_only: true,
                limit: None,
                isolated: false,
                archive_filter: ArchiveFilter::NonArchived,
            },
        )
        .unwrap();
        assert_eq!(report.data[0].id, "hidden");
        assert_eq!(report.data[0].status, "waiting_input");
        assert_eq!(
            report.data[0].latest_turn_status.as_deref(),
            Some("inProgress")
        );
        assert!(report.data[0].latest_command.is_none());
        assert!(!facet_json::to_string(&report).unwrap().contains("secret"));
    }

    #[test]
    fn disconnected_is_not_idle_and_wide_text_is_clipped() {
        let report = ActiveThreadsReport::disconnected(false);
        assert!(!report.coverage.complete);
        assert_eq!(report.coverage.connection, "disconnected");
        assert!(report.data.is_empty());
        assert_eq!(clean_text("界界界", 4), "界界");
        assert!(!clean_text("a\x1b[2J\nb", 20).contains('\x1b'));
    }

    #[test]
    fn descending_history_paginates_and_caps_without_guessing() {
        let input = concat!(
            "{\"id\":1,\"result\":{\"data\":[],\"nextCursor\":\"next\"}}\n",
            "{\"id\":2,\"result\":{\"data\":[{\"item\":{\"type\":\"commandExecution\",\"id\":\"latest\",\"command\":\"git status\"}}]}}\n"
        );
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        assert_eq!(
            latest_command(&mut client, "task").unwrap().0.unwrap().id,
            "latest"
        );
        let input = (1..=4)
            .map(|id| {
                format!("{{\"id\":{id},\"result\":{{\"data\":[],\"nextCursor\":\"page{id}\"}}}}\n")
            })
            .collect::<String>();
        let mut client = AppServerStdio::new(BufReader::new(Cursor::new(input)), Vec::new());
        let (command, availability) = latest_command(&mut client, "task").unwrap();
        assert!(command.is_none());
        assert!(availability.contains("capped"));
    }
}
