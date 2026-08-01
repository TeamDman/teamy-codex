use crate::cli::thread::list::ThreadListArgs;
use crate::cli::thread::message::list::ThreadMessageListArgs;
use eyre::Context;
use eyre::ContextCompat;
use eyre::bail;
use facet::Facet;
use rusqlite::Connection;
use rusqlite::params;
use serde_json::Value;
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
        let event: Value = serde_json::from_str(&line?)?;
        if event.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let Some(payload) = event.get("payload").and_then(Value::as_object) else {
            continue;
        };
        let Some(payload_type) = payload.get("type").and_then(Value::as_str) else {
            continue;
        };
        let role = match payload_type {
            "user_message" => "user",
            "agent_message" => "assistant",
            _ => continue,
        };
        let Some(text) = payload.get("message").and_then(Value::as_str) else {
            continue;
        };
        messages.push(ThreadMessage {
            index: messages.len() + 1,
            timestamp: event
                .get("timestamp")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            role: role.to_owned(),
            phase: payload
                .get("phase")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            text: text.to_owned(),
        });
        if messages.len() >= limit {
            break;
        }
    }
    Ok(messages)
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
