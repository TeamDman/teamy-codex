//! Terminal-first OTLP overview. Incoming records are not re-emitted as tracing events.
use crate::telemetry::escape_display;
use crate::telemetry::terminal_capture::TerminalCapture;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use eyre::{Context, ContextCompat, Result, bail};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
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

fn clipped_line(input: &str, width: u16) -> String {
    let line = single_line(input);
    let mut result = String::new();
    let mut used = 0usize;
    let width = usize::from(width);
    for ch in line.chars() {
        let size = ch.width().unwrap_or(0);
        if used.saturating_add(size) >= width {
            if width > 0 {
                result.push('…');
            }
            return result;
        }
        used += size;
        result.push(ch);
    }
    result
}

/// Print a line into scrollback immediately above the inline viewport.
///
/// # Errors
/// Returns a backend error if the terminal cannot insert the line.
pub fn print_above<B: Backend>(
    terminal: &mut Terminal<B>,
    line: &str,
) -> std::result::Result<(), B::Error> {
    let width = terminal.size()?.width;
    let line = clipped_line(line, width);
    terminal.insert_before(1, |buffer| Paragraph::new(line).render(buffer.area, buffer))
}

struct InlineGuard<B: Backend> {
    terminal: Terminal<B>,
}

impl<B: Backend> Drop for InlineGuard<B> {
    fn drop(&mut self) {
        let origin = self.terminal.get_frame().area().as_position();
        let _ = self.terminal.clear();
        let _ = self.terminal.set_cursor_position(origin);
        let _ = self.terminal.show_cursor();
        let _ = self.terminal.backend_mut().flush();
        let _ = disable_raw_mode();
    }
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
    enable_raw_mode().wrap_err("enable terminal raw mode")?;
    let backend = CrosstermBackend::new(BufWriter::new(std::io::stderr()));
    let terminal = match Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(5),
        },
    ) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
    };
    let mut terminal = InlineGuard { terminal };
    let capture = TerminalCapture::current().context("terminal log capture not installed")?;
    let mut cursor = TelemetryCursor::default();
    loop {
        let (records, stats) = {
            let store = store
                .lock()
                .map_err(|_poisoned| eyre::eyre!("telemetry store poisoned"))?;
            (cursor.take(&store), store.stats())
        };
        for record in &records {
            if let Some(file) = file.as_mut() {
                write_event(file, record)?;
            }
            print_above(&mut terminal.terminal, &event_summary(record))
                .wrap_err("display telemetry")?;
        }
        if !records.is_empty()
            && let Some(file) = file.as_mut()
        {
            file.flush().wrap_err("flush telemetry events file")?;
        }
        let (pending, dropped_logs) = capture.snapshot_limit(16)?;
        for (id, line) in &pending {
            print_above(&mut terminal.terminal, line).wrap_err("display process log")?;
            capture.acknowledge(*id)?;
        }
        terminal.terminal.draw(|frame| {
            let text = overview(address, stats, &cursor, dropped_logs);
            frame.render_widget(Paragraph::new(text), frame.area());
        })?;
        if cancellation.bail_if_cancelled().is_err() {
            break;
        }
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                || (key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)))
        {
            break;
        }
    }
    Ok(cursor)
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
