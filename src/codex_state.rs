use crate::cli::thread::list::ThreadListArgs;
use crate::cli::thread::message::list::ThreadMessageListArgs;
use eyre::Context;
use eyre::ContextCompat;
use eyre::bail;
use facet::Facet;
use facet_json::RawJson;
use rusqlite::Connection;
use rusqlite::params;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::path::{Path, PathBuf};

#[derive(Facet, Debug)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
    pub recency_at_ms: i64,
    pub archived: bool,
    pub tokens_used: i64,
    pub source: String,
    pub thread_source: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub rollout_path: String,
}

#[derive(Facet, Debug)]
pub struct ThreadMessage {
    pub index: usize,
    pub timestamp: Option<String>,
    pub role: String,
    pub phase: Option<String>,
    pub text: String,
}

#[derive(Facet, Debug)]
struct RolloutEvent<'a> {
    #[facet(rename = "type")]
    kind: Option<RawJson<'a>>,
    timestamp: Option<RawJson<'a>>,
    payload: Option<RawJson<'a>>,
}

#[derive(Facet, Debug)]
struct RolloutPayload<'a> {
    #[facet(rename = "type")]
    kind: Option<RawJson<'a>>,
    message: Option<RawJson<'a>>,
    phase: Option<RawJson<'a>>,
}

pub fn resolve_codex_home(raw: Option<&str>) -> eyre::Result<PathBuf> {
    raw.map_or_else(default_codex_home, |path| Ok(PathBuf::from(path)))
}

fn default_codex_home() -> eyre::Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("could not determine home directory")?
        .join(".codex"))
}

pub fn list_threads(codex_home: &Path, args: &ThreadListArgs) -> eyre::Result<Vec<ThreadSummary>> {
    let limit = args.limit.unwrap_or(50);
    if limit == 0 {
        bail!("--limit must be greater than zero");
    }

    let connection = open_state_database(codex_home)?;
    let archived_filter = if args.all {
        ""
    } else if args.archived {
        "where archived = 1"
    } else {
        "where archived = 0"
    };
    let sql = format!(
        "select id, title, cwd, created_at_ms, updated_at_ms, recency_at_ms, archived, \
         tokens_used, source, thread_source, model, reasoning_effort, rollout_path \
         from threads {archived_filter} \
         order by coalesce(updated_at_ms, recency_at_ms, updated_at) desc \
         limit ?1"
    );

    let limit = i64::try_from(limit).wrap_err("--limit is too large")?;
    let mut statement = connection.prepare(&sql)?;
    let threads = statement
        .query_map(params![limit], |row| {
            Ok(ThreadSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                cwd: normalize_extended_path(&row.get::<_, String>(2)?),
                created_at_ms: row.get(3)?,
                updated_at_ms: row.get(4)?,
                recency_at_ms: row.get(5)?,
                archived: row.get::<_, i64>(6)? != 0,
                tokens_used: row.get(7)?,
                source: row.get(8)?,
                thread_source: row.get(9)?,
                model: row.get(10)?,
                reasoning_effort: row.get(11)?,
                rollout_path: normalize_extended_path(&row.get::<_, String>(12)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(threads)
}

pub fn list_thread_messages(
    codex_home: &Path,
    args: &ThreadMessageListArgs,
) -> eyre::Result<Vec<ThreadMessage>> {
    let limit = args.limit.unwrap_or(50);
    if limit == 0 {
        bail!("--limit must be greater than zero");
    }

    let rollout_path = find_rollout_path(codex_home, &args.thread_id)?;
    let file = File::open(&rollout_path)
        .with_context(|| format!("failed to open {}", rollout_path.display()))?;
    let reader = BufReader::new(file);
    let mut messages = Vec::new();

    for line in reader.lines() {
        if let Some(message) = parse_thread_message(&line?, messages.len() + 1)? {
            messages.push(message);
        }
        if messages.len() >= limit {
            break;
        }
    }
    Ok(messages)
}

fn parse_thread_message(line: &str, index: usize) -> eyre::Result<Option<ThreadMessage>> {
    let raw_event: RawJson<'_> = facet_json::from_str_borrowed(line)?;
    let Ok(event) = facet_json::from_str_borrowed::<RolloutEvent<'_>>(raw_event.as_str()) else {
        return Ok(None);
    };
    if raw_json_string(event.kind.as_ref()).as_deref() != Some("event_msg") {
        return Ok(None);
    }

    let Some(raw_payload) = event.payload.as_ref() else {
        return Ok(None);
    };
    let Ok(payload) = facet_json::from_str_borrowed::<RolloutPayload<'_>>(raw_payload.as_str())
    else {
        return Ok(None);
    };
    let role = match raw_json_string(payload.kind.as_ref()).as_deref() {
        Some("user_message") => "user",
        Some("agent_message") => "assistant",
        _ => return Ok(None),
    };
    let Some(text) = raw_json_string(payload.message.as_ref()) else {
        return Ok(None);
    };

    Ok(Some(ThreadMessage {
        index,
        timestamp: raw_json_string(event.timestamp.as_ref()),
        role: role.to_owned(),
        phase: raw_json_string(payload.phase.as_ref()),
        text,
    }))
}

fn raw_json_string(value: Option<&RawJson<'_>>) -> Option<String> {
    let raw = value?.as_str().trim_start();
    raw.starts_with('"')
        .then(|| facet_json::from_str(raw).ok())
        .flatten()
}

fn open_state_database(codex_home: &Path) -> eyre::Result<Connection> {
    let db_path = codex_home.join("state_5.sqlite");
    Connection::open(&db_path)
        .with_context(|| format!("failed to open Codex state database {}", db_path.display()))
}

fn find_rollout_path(codex_home: &Path, thread_id: &str) -> eyre::Result<PathBuf> {
    let connection = open_state_database(codex_home)?;
    let rollout_path: String = connection
        .query_row(
            "select rollout_path from threads where id = ?1",
            params![thread_id],
            |row| row.get(0),
        )
        .with_context(|| format!("thread not found: {thread_id}"))?;
    Ok(PathBuf::from(normalize_extended_path(&rollout_path)))
}

fn normalize_extended_path(path: &str) -> String {
    path.strip_prefix(r"\\?\")
        .map_or_else(|| path.to_owned(), ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rollout_message_from_raw_json() {
        let line = r#"{
            "timestamp": "2026-08-15T01:02:03Z",
            "type": "event_msg",
            "payload": {
                "type": "agent_message",
                "message": "first line\nsecond line",
                "phase": "commentary",
                "ignored": {"nested": true}
            }
        }"#;

        let message = parse_thread_message(line, 7)
            .expect("valid JSON should parse")
            .expect("agent message should be returned");

        assert_eq!(message.index, 7);
        assert_eq!(message.timestamp.as_deref(), Some("2026-08-15T01:02:03Z"));
        assert_eq!(message.role, "assistant");
        assert_eq!(message.phase.as_deref(), Some("commentary"));
        assert_eq!(message.text, "first line\nsecond line");
    }

    #[test]
    fn ignores_non_string_optional_rollout_fields() {
        let line = r#"{
            "timestamp": 42,
            "type": "event_msg",
            "payload": {
                "type": "user_message",
                "message": "hello",
                "phase": false
            }
        }"#;

        let message = parse_thread_message(line, 1)
            .expect("valid JSON should parse")
            .expect("user message should be returned");

        assert_eq!(message.timestamp, None);
        assert_eq!(message.role, "user");
        assert_eq!(message.phase, None);
        assert_eq!(message.text, "hello");
    }
}
