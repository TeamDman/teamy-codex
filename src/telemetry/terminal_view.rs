//! Terminal-first OTLP overview. Incoming records are not re-emitted as tracing events.
use crate::telemetry::escape_display;
use crate::telemetry::terminal_capture::TerminalCapture;
use crate::telemetry::terminal_foundation::InlineTerminal;
pub use crate::telemetry::terminal_foundation::print_above;
use crate::telemetry::terminal_navigation::{Navigation, Screen};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use eyre::{Context, ContextCompat, Result, bail};
use ratatui::widgets::Paragraph;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teamy_cancellation::CancellationToken;
use teamy_tracing::telemetry::otlp_http::{Receiver, ReceiverLimits};
use teamy_tracing::telemetry::{RecordStore, StoreStats, TelemetryRecord, TelemetryValue};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Default)]
pub struct TelemetryCursor {
    next: u64,
    pub missed: u64,
    pub decisions: u64,
    pub results: u64,
}

impl TelemetryCursor {
    /// Read only records not already processed, accounting for evicted records.
    pub fn take(&mut self, store: &RecordStore) -> Vec<TelemetryRecord> {
        let stats = store.stats();
        let first = stats.accepted.saturating_sub(stats.records as u64);
        self.missed = self.missed.saturating_add(first.saturating_sub(self.next));
        let offset = usize::try_from(self.next.saturating_sub(first)).unwrap_or(usize::MAX);
        let records: Vec<_> = store.iter().skip(offset).cloned().collect();
        self.next = stats.accepted;
        for record in &records {
            match event_name(record) {
                "codex.tool_decision" => self.decisions = self.decisions.saturating_add(1),
                "codex.tool_result" => self.results = self.results.saturating_add(1),
                _ => {}
            }
        }
        records
    }
}

fn event_name(record: &TelemetryRecord) -> &str {
    if !record.event_name.is_empty() {
        return &record.event_name;
    }
    match record.attribute("event.name") {
        Some(TelemetryValue::String(value)) => value,
        _ => "unknown event",
    }
}

/// A safe one-line summary; arguments and output remain in the bounded store.
#[must_use]
pub fn event_summary(record: &TelemetryRecord) -> String {
    let tool = match record.attribute("tool_name") {
        Some(TelemetryValue::String(value)) => value.as_str(),
        _ => "unknown tool",
    };
    let truncated = if matches!(
        record.attribute("output_truncated"),
        Some(TelemetryValue::Bool(true))
    ) {
        " · output truncated"
    } else {
        ""
    };
    single_line(&format!("{} · {}{}", event_name(record), tool, truncated))
}

/// Serialize an admitted record to a caller-owned NDJSON sink.
///
/// # Errors
/// Returns an error if serialization or writing fails.
pub fn write_event(mut writer: impl Write, record: &TelemetryRecord) -> Result<()> {
    let json = owned_facet_json::to_string(record)
        .map_err(|error| eyre::eyre!("serialize telemetry record: {error}"))?;
    writeln!(writer, "{json}").wrap_err("write telemetry event")?;
    Ok(())
}

fn single_line(input: &str) -> String {
    escape_display(input)
        .replace('\n', " ⏎ ")
        .replace('\t', " ")
}

/// Preserve the complete record while showing arguments and output as readable text.
#[must_use]
pub fn detail_text(record: &TelemetryRecord) -> String {
    let mut text = String::new();
    for (key, title) in [("arguments", "Arguments"), ("output", "Output preview")] {
        if let Some(TelemetryValue::String(value)) = record.attribute(key) {
            text.push_str(title);
            text.push_str(":\n");
            text.push_str(&escape_display(value));
            text.push_str("\n\n");
        }
    }
    text.push_str("Complete record (including unknown fields):\n");
    text.push_str(&escape_display(&format!("{record:#?}")));
    text
}

/// Wrap without dropping any characters; navigation uses logical rows rather than u16 offsets.
#[must_use]
pub fn detail_rows(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let text = text.replace('\t', "\\t");
    let mut rows = Vec::new();
    let mut line = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            rows.push(std::mem::take(&mut line));
            used = 0;
            continue;
        }
        let cells = ch.width().unwrap_or(0);
        if used > 0 && used.saturating_add(cells) > width {
            rows.push(std::mem::take(&mut line));
            used = 0;
        }
        line.push(ch);
        used = used.saturating_add(cells);
    }
    rows.push(line);
    rows
}

/// Start a loopback collector only as part of this explicit command.
///
/// # Errors
/// Returns an error for terminal, bind, file, output or shutdown failures.
pub async fn run(
    port: u16,
    events_file: Option<&str>,
    cancellation: CancellationToken,
) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("telemetry requires interactive stdin and stderr");
    }
    let mut file = events_file
        .map(|path| OpenOptions::new().write(true).create_new(true).open(path))
        .transpose()
        .wrap_err("create telemetry events file")?
        .map(BufWriter::new);
    let store = Arc::new(Mutex::new(RecordStore::default()));
    let receiver = Receiver::bind_loopback(port, ReceiverLimits::default(), Arc::clone(&store))
        .await
        .wrap_err("bind OTLP receiver")?;
    let address = receiver.address();
    let result = run_terminal(address, &store, file.as_mut(), &cancellation);
    let shutdown = tokio::time::timeout(Duration::from_secs(15), receiver.shutdown())
        .await
        .wrap_err("OTLP receiver shutdown timed out")
        .and_then(|result| result.wrap_err("stop OTLP receiver"));
    let mut cursor = result?;
    shutdown?;
    let (remaining, stats) = {
        let store = store
            .lock()
            .map_err(|_poisoned| eyre::eyre!("telemetry store poisoned"))?;
        (cursor.take(&store), store.stats())
    };
    let mut stderr = std::io::stderr().lock();
    for record in &remaining {
        if let Some(file) = file.as_mut() {
            write_event(file, record)?;
        }
        writeln!(stderr, "{}", event_summary(record)).wrap_err("display final telemetry")?;
    }
    let (_, dropped_logs) = TerminalCapture::current()
        .context("terminal log capture not installed")?
        .snapshot_limit(0)?;
    writeln!(stderr, "OTLP listener stopped. Accepted: {}  Evicted: {}  Rejected: {}  Missed before display: {}  Upstream truncated: {}  Process logs evicted: {}", stats.accepted, stats.evicted, stats.rejected_oversize, cursor.missed, stats.upstream_truncated, dropped_logs)
    .wrap_err("display final telemetry totals")?;
    if let Some(file) = file.as_mut() {
        file.flush().wrap_err("flush telemetry events file")?;
    }
    Ok(())
}

fn run_terminal(
    address: std::net::SocketAddr,
    store: &Arc<Mutex<RecordStore>>,
    mut file: Option<&mut BufWriter<File>>,
    cancellation: &CancellationToken,
) -> Result<TelemetryCursor> {
    let mut guard = InlineTerminal::open(BufWriter::new(std::io::stderr()), 12)
        .wrap_err("open inline terminal")?;
    let terminal = guard.terminal_mut().context("inline terminal is closed")?;
    let capture = TerminalCapture::current().context("terminal log capture not installed")?;
    let mut cursor = TelemetryCursor::default();
    let mut navigation = Navigation::default();
    let mut details_cache: Option<(u64, u16, Vec<String>)> = None;
    loop {
        let width = terminal.size()?.width;
        let (records, stats, selected_summary, selected_record) = {
            let store = store
                .lock()
                .map_err(|_poisoned| eyre::eyre!("telemetry store poisoned"))?;
            let records = cursor.take(&store);
            let stats = store.stats();
            navigation.initialize_selection(stats);
            let selected = navigation
                .selected_index(stats)
                .and_then(|index| store.iter().nth(index));
            let summary = selected.map(event_summary);
            let needs_detail = matches!(navigation.screen, Screen::Detail { .. })
                && selected.is_some()
                && details_cache.as_ref().is_none_or(|(id, old_width, _)| {
                    Some(*id) != navigation.selected || *old_width != width
                });
            (
                records,
                stats,
                summary,
                needs_detail.then(|| selected.cloned()).flatten(),
            )
        };
        if let (Some(id), Some(record)) = (navigation.selected, selected_record) {
            details_cache = Some((id, width, detail_rows(&detail_text(&record), width)));
        }
        for record in &records {
            if let Some(file) = file.as_mut() {
                write_event(file, record)?;
            }
            print_above(terminal, &event_summary(record)).wrap_err("display telemetry")?;
        }
        if !records.is_empty()
            && let Some(file) = file.as_mut()
        {
            file.flush().wrap_err("flush telemetry events file")?;
        }
        let (pending, dropped_logs) = capture.snapshot_limit(16)?;
        for (id, line) in &pending {
            print_above(terminal, line).wrap_err("display process log")?;
            capture.acknowledge(*id)?;
        }
        terminal.draw(|frame| {
            let text = view_text(
                &navigation,
                stats,
                details_cache.as_ref(),
                &overview(address, stats, &cursor, dropped_logs),
                selected_summary.as_deref(),
                frame.area().height,
            );
            frame.render_widget(Paragraph::new(text), frame.area());
        })?;
        if let Screen::Detail { scroll } = &mut navigation.screen
            && let Some((_, _, rows)) = &details_cache
        {
            *scroll = (*scroll).min(rows.len().saturating_sub(1));
        }
        if cancellation.bail_if_cancelled().is_err() {
            break;
        }
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && handle_key(&mut navigation, stats, key)
        {
            break;
        }
    }
    guard.close().wrap_err("restore inline terminal")?;
    Ok(cursor)
}

fn view_text(
    navigation: &Navigation,
    stats: StoreStats,
    cache: Option<&(u64, u16, Vec<String>)>,
    overview: &str,
    summary: Option<&str>,
    height: u16,
) -> String {
    match navigation.screen {
        Screen::Overview => format!(
            "{}\nSelected: {}\nUp/Down select · Enter inspect",
            overview,
            summary.unwrap_or("none or evicted")
        ),
        Screen::Detail { scroll } => match (navigation.selected_index(stats), cache) {
            (Some(_), Some((id, _, rows))) if Some(*id) == navigation.selected => {
                let start = scroll.min(rows.len().saturating_sub(1));
                let height = usize::from(height.saturating_sub(2));
                format!(
                    "Event {} · row {}/{} · arrows/PgUp/PgDn · Esc back\n{}",
                    id,
                    start + 1,
                    rows.len(),
                    rows.iter()
                        .skip(start)
                        .take(height)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
            _ => "Selected event was evicted. Press Esc to return.".into(),
        },
    }
}

fn handle_key(navigation: &mut Navigation, stats: StoreStats, key: event::KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
        KeyCode::Esc if navigation.back() => return true,
        KeyCode::Up => navigation.previous(stats),
        KeyCode::Down => navigation.next(stats),
        KeyCode::PageUp => {
            for _ in 0..10 {
                navigation.previous(stats);
            }
        }
        KeyCode::PageDown => {
            for _ in 0..10 {
                navigation.next(stats);
            }
        }
        KeyCode::Home => {
            if let Screen::Detail { scroll } = &mut navigation.screen {
                *scroll = 0;
            }
        }
        KeyCode::End => {
            if let Screen::Detail { scroll } = &mut navigation.screen {
                *scroll = usize::MAX;
            }
        }
        KeyCode::Enter => navigation.enter_detail(stats),
        _ => {}
    }
    false
}

fn overview(
    address: std::net::SocketAddr,
    stats: StoreStats,
    cursor: &TelemetryCursor,
    dropped_logs: u64,
) -> String {
    format!(
        "OTLP logs: http://{address}/v1/logs  ·  q/Esc/Ctrl+C quit\nAccepted: {}  Retained: {}  Decisions: {}  Results: {}\nEvicted: {}  Rejected: {}  Missed before display: {}\nUpstream truncated: {}  Process logs evicted: {}\nIn-memory only unless --events-file is specified",
        stats.accepted,
        stats.records,
        cursor.decisions,
        cursor.results,
        stats.evicted,
        stats.rejected_oversize,
        cursor.missed,
        stats.upstream_truncated,
        dropped_logs
    )
}
