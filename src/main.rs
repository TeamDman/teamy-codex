use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(name = "teamy-codex")]
struct Cli {
    #[arg(long, global = true)]
    codex_home: Option<PathBuf>,

    #[arg(short = 'o', long, global = true, default_value_t = OutputFormat::Plain)]
    output_format: OutputFormat,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Thread(ThreadArgs),
}

#[derive(Args, Debug)]
struct ThreadArgs {
    #[command(subcommand)]
    command: ThreadCommand,
}

#[derive(Subcommand, Debug)]
enum ThreadCommand {
    List(ThreadListArgs),
    Message(ThreadMessageArgs),
}

#[derive(Args, Debug)]
struct ThreadListArgs {
    #[arg(long)]
    limit: usize,

    #[arg(long)]
    all: bool,

    #[arg(long)]
    archived: bool,
}

#[derive(Args, Debug)]
struct ThreadMessageArgs {
    #[command(subcommand)]
    command: ThreadMessageCommand,
}

#[derive(Subcommand, Debug)]
enum ThreadMessageCommand {
    List(ThreadMessageListArgs),
}

#[derive(Args, Debug)]
struct ThreadMessageListArgs {
    thread_id: String,

    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(ValueEnum, Copy, Clone, Debug, Default, Eq, PartialEq)]
enum OutputFormat {
    #[default]
    Plain,
    Json,
    PrettyJson,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            OutputFormat::Plain => "plain",
            OutputFormat::Json => "json",
            OutputFormat::PrettyJson => "pretty-json",
        };
        formatter.write_str(value)
    }
}

#[derive(Serialize)]
struct ThreadSummary {
    id: String,
    title: String,
    cwd: String,
    created_at_ms: Option<i64>,
    updated_at_ms: Option<i64>,
    recency_at_ms: i64,
    archived: bool,
    tokens_used: i64,
    source: String,
    thread_source: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    rollout_path: String,
}

#[derive(Serialize)]
struct ThreadMessage {
    index: usize,
    timestamp: Option<String>,
    role: String,
    phase: Option<String>,
    text: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let codex_home = cli.codex_home.clone().unwrap_or(default_codex_home()?);

    match cli.command {
        Command::Thread(args) => match args.command {
            ThreadCommand::List(args) => {
                let threads = list_threads(&codex_home, args)?;
                render_threads(&threads, cli.output_format)
            }
            ThreadCommand::Message(args) => match args.command {
                ThreadMessageCommand::List(args) => {
                    let messages = list_thread_messages(&codex_home, args)?;
                    render_messages(&messages, cli.output_format)
                }
            },
        },
    }
}

fn default_codex_home() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("Could not determine home directory")?
        .join(".codex"))
}

fn list_threads(codex_home: &Path, args: ThreadListArgs) -> Result<Vec<ThreadSummary>> {
    if args.limit == 0 {
        bail!("--limit must be greater than zero");
    }

    let db_path = codex_home.join("state_5.sqlite");
    let connection = Connection::open(&db_path)
        .with_context(|| format!("Failed to open {}", db_path.display()))?;

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

    let mut statement = connection.prepare(&sql)?;
    let threads = statement
        .query_map(params![args.limit as i64], |row| {
            Ok(ThreadSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                cwd: normalize_extended_path(row.get::<_, String>(2)?),
                created_at_ms: row.get(3)?,
                updated_at_ms: row.get(4)?,
                recency_at_ms: row.get(5)?,
                archived: row.get::<_, i64>(6)? != 0,
                tokens_used: row.get(7)?,
                source: row.get(8)?,
                thread_source: row.get(9)?,
                model: row.get(10)?,
                reasoning_effort: row.get(11)?,
                rollout_path: normalize_extended_path(row.get::<_, String>(12)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(threads)
}

fn list_thread_messages(
    codex_home: &Path,
    args: ThreadMessageListArgs,
) -> Result<Vec<ThreadMessage>> {
    if args.limit == 0 {
        bail!("--limit must be greater than zero");
    }

    let rollout_path = find_rollout_path(codex_home, &args.thread_id)?;
    let file = File::open(&rollout_path)
        .with_context(|| format!("Failed to open {}", rollout_path.display()))?;
    let reader = BufReader::new(file);

    let mut messages = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let event: Value = serde_json::from_str(&line)?;
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

        if messages.len() >= args.limit {
            break;
        }
    }

    Ok(messages)
}

fn find_rollout_path(codex_home: &Path, thread_id: &str) -> Result<PathBuf> {
    let db_path = codex_home.join("state_5.sqlite");
    let connection = Connection::open(&db_path)
        .with_context(|| format!("Failed to open {}", db_path.display()))?;

    let rollout_path: String = connection
        .query_row(
            "select rollout_path from threads where id = ?1",
            params![thread_id],
            |row| row.get(0),
        )
        .with_context(|| format!("Thread not found: {thread_id}"))?;

    Ok(PathBuf::from(normalize_extended_path(rollout_path)))
}

fn normalize_extended_path(path: String) -> String {
    path.strip_prefix(r"\\?\")
        .map_or(path.clone(), ToOwned::to_owned)
}

fn render_threads(threads: &[ThreadSummary], output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Plain => {
            println!("id\tupdated_at_ms\ttokens_used\ttitle\tcwd");
            for thread in threads {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    thread.id,
                    thread.updated_at_ms.unwrap_or(thread.recency_at_ms),
                    thread.tokens_used,
                    thread.title.replace(['\r', '\n', '\t'], " "),
                    thread.cwd
                );
            }
        }
        OutputFormat::Json => println!("{}", serde_json::to_string(threads)?),
        OutputFormat::PrettyJson => println!("{}", serde_json::to_string_pretty(threads)?),
    }

    Ok(())
}

fn render_messages(messages: &[ThreadMessage], output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Plain => {
            println!("index\ttimestamp\trole\tphase\ttext");
            for message in messages {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    message.index,
                    message.timestamp.as_deref().unwrap_or(""),
                    message.role,
                    message.phase.as_deref().unwrap_or(""),
                    message.text.replace(['\r', '\n', '\t'], " ")
                );
            }
        }
        OutputFormat::Json => println!("{}", serde_json::to_string(messages)?),
        OutputFormat::PrettyJson => println!("{}", serde_json::to_string_pretty(messages)?),
    }

    Ok(())
}
