//! Terminal-first OTLP overview. Incoming records are not re-emitted as tracing events.
use crate::telemetry::terminal_capture::TerminalCapture;
use crate::telemetry::terminal_foundation::InlineTerminal;
pub use crate::telemetry::terminal_foundation::print_above;
use crate::telemetry::terminal_navigation::{Navigation, Screen};
use crate::telemetry::{
    Correlation, ShellKind, command_detail, escape_display, from_telemetry_record,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use eyre::{Context, ContextCompat, Result, bail};
use ratatui::widgets::Paragraph;
use std::fmt::Write as _;
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

/// The optional host controls visibility and may request a clean exit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalState {
    Visible,
    Hidden,
    Exit,
}

/// Preserve bounded process logs while the owned console is hidden.
#[must_use]
pub fn process_log_batch_limit(state: TerminalState) -> usize {
    if state == TerminalState::Visible {
        16
    } else {
        0
    }
}

pub trait TerminalControl {
    /// # Errors
    /// Returns an error if the host cannot deliver or apply its control events.
    fn poll(&mut self) -> Result<TerminalState>;
}

struct AlwaysVisible;
impl TerminalControl for AlwaysVisible {
    fn poll(&mut self) -> Result<TerminalState> {
        Ok(TerminalState::Visible)
    }
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
    match record.attribute("event.name") {
        Some(TelemetryValue::String(value)) if !value.is_empty() => value,
        _ if !record.event_name.is_empty() => &record.event_name,
        _ => "unknown event",
    }
}

/// A safe one-line summary; arguments and output remain in the bounded store.
#[must_use]
pub fn event_summary(record: &TelemetryRecord) -> String {
    let name = event_name(record);
    let mut summary = name.to_owned();
    match record.attribute("tool_name") {
        Some(TelemetryValue::String(value)) if !value.is_empty() => {
            summary.push_str(" · ");
            summary.push_str(value);
        }
        _ if matches!(name, "codex.tool_decision" | "codex.tool_result") => {
            summary.push_str(" · unknown tool");
        }
        _ => {}
    }
    if event_name(record) == "codex.tool_result" {
        match record.attribute("success") {
            Some(TelemetryValue::Bool(true)) => summary.push_str(" · success"),
            Some(TelemetryValue::Bool(false)) => summary.push_str(" · failed"),
            Some(TelemetryValue::String(value)) if value == "true" => {
                summary.push_str(" · success");
            }
            Some(TelemetryValue::String(value)) if value == "false" => {
                summary.push_str(" · failed");
            }
            _ => {}
        }
        let duration = match record.attribute("duration_ms") {
            Some(TelemetryValue::Integer(value)) => u64::try_from(*value).ok(),
            Some(TelemetryValue::String(value)) => value.parse::<u64>().ok(),
            _ => None,
        };
        if let Some(duration) = duration {
            let _ = write!(summary, " · {duration} ms");
        }
    } else if event_name(record) == "codex.tool_decision"
        && let Some(TelemetryValue::String(decision)) = record.attribute("decision")
    {
        summary.push_str(" · ");
        summary.push_str(decision);
    }
    let truncated = if matches!(
        record.attribute("output_truncated"),
        Some(TelemetryValue::Bool(true))
    ) || matches!(record.attribute("output_truncated"), Some(TelemetryValue::String(value)) if value == "true")
    {
        " · output truncated"
    } else {
        ""
    };
    summary.push_str(truncated);
    single_line(&summary)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolLink {
    pub correlation: Correlation,
    /// Stable sequence in the current store; absent for ambiguous/unmatched events.
    pub related: Option<u64>,
}

fn attribute_text<'a>(record: &'a TelemetryRecord, key: &str) -> Option<&'a str> {
    match record.attribute(key) {
        Some(TelemetryValue::String(value)) if !value.is_empty() => Some(value),
        _ => None,
    }
}

fn source(record: &TelemetryRecord) -> Option<&str> {
    let attribute = record
        .resource_attributes
        .iter()
        .find(|attribute| attribute.key == "service.instance.id")?;
    match &attribute.value {
        TelemetryValue::String(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// Relate only uniquely matching retained decisions/results; never infer missing IDs.
#[must_use]
pub fn tool_link(store: &RecordStore, selected: usize) -> Option<ToolLink> {
    let selected_record = store.iter().nth(selected)?;
    let selected_name = event_name(selected_record);
    if !matches!(selected_name, "codex.tool_decision" | "codex.tool_result") {
        return None;
    }
    let (Some(session), Some(call)) = (
        attribute_text(selected_record, "conversation.id"),
        attribute_text(selected_record, "call_id"),
    ) else {
        return Some(ToolLink {
            correlation: Correlation::MissingIdentity,
            related: None,
        });
    };
    let selected_source = source(selected_record);
    let mut decisions = 0usize;
    let mut results = 0usize;
    let mut related = None;
    let first = store
        .stats()
        .accepted
        .saturating_sub(store.stats().records as u64)
        .saturating_add(1);
    for (index, candidate) in store.iter().enumerate() {
        if source(candidate) != selected_source
            || attribute_text(candidate, "conversation.id") != Some(session)
            || attribute_text(candidate, "call_id") != Some(call)
        {
            continue;
        }
        match event_name(candidate) {
            "codex.tool_decision" => decisions += 1,
            "codex.tool_result" => results += 1,
            _ => continue,
        }
        if index != selected && event_name(candidate) != selected_name {
            related = Some(first.saturating_add(index as u64));
        }
    }
    let correlation = if decisions > 1 || results > 1 {
        Correlation::Ambiguous
    } else if decisions == 1 && results == 1 {
        if selected_source.is_some() {
            Correlation::Exact
        } else {
            Correlation::SourceUnknown
        }
    } else {
        Correlation::Unmatched
    };
    Some(ToolLink {
        correlation,
        related: matches!(correlation, Correlation::Exact | Correlation::SourceUnknown)
            .then_some(related)
            .flatten(),
    })
}

/// Cached rows and identity fingerprint; unrelated incoming events do not rebuild text.
#[derive(Debug)]
pub struct DetailCache {
    pub id: u64,
    pub width: u16,
    pub version: u64,
    pub records: usize,
    pub link: Option<ToolLink>,
    pub rows: Vec<String>,
}

/// Compute changed detail input while the caller has access to the store.
#[must_use]
pub fn detail_update(
    store: &RecordStore,
    id: u64,
    width: u16,
    cache: Option<&mut DetailCache>,
) -> Option<(TelemetryRecord, Option<TelemetryRecord>, Option<ToolLink>)> {
    let stats = store.stats();
    let first = stats
        .accepted
        .saturating_sub(stats.records as u64)
        .saturating_add(1);
    let index = usize::try_from(id.checked_sub(first)?).ok()?;
    let selected = store.iter().nth(index)?;
    if let Some(cache) = cache {
        if cache.id == id
            && cache.width == width
            && cache.version == stats.accepted
            && cache.records == stats.records
        {
            return None;
        }
        let link = tool_link(store, index);
        if cache.id == id && cache.width == width && cache.link == link {
            cache.version = stats.accepted;
            cache.records = stats.records;
            return None;
        }
        let related = link
            .and_then(|link| link.related)
            .and_then(|seq| usize::try_from(seq.checked_sub(first)?).ok())
            .and_then(|i| store.iter().nth(i))
            .cloned();
        return Some((selected.clone(), related, link));
    }
    let link = tool_link(store, index);
    let related = link
        .and_then(|link| link.related)
        .and_then(|seq| usize::try_from(seq.checked_sub(first)?).ok())
        .and_then(|i| store.iter().nth(i))
        .cloned();
    Some((selected.clone(), related, link))
}

#[must_use]
pub fn detail_cache(
    id: u64,
    width: u16,
    stats: StoreStats,
    selected: &TelemetryRecord,
    related: Option<&TelemetryRecord>,
    link: Option<ToolLink>,
) -> DetailCache {
    let mut text = String::new();
    if let Some(link) = link {
        let label = match link.correlation {
            Correlation::Exact => "matched by source, session and call ID",
            Correlation::SourceUnknown => "matched by session and call ID; producer source unknown",
            Correlation::MissingIdentity => "unmatched: missing session or call ID",
            Correlation::Ambiguous => "ambiguous: duplicate decisions or results",
            Correlation::Unmatched => "unmatched in retained history",
            Correlation::OtherEvent => "not a tool event",
        };
        let _ = writeln!(text, "Correlation: {label}\n");
    }
    text.push_str("Selected event:\n");
    text.push_str(&detail_text(selected));
    if let Some(record) = related {
        let _ = write!(text, "\n\nRelated {}:\n", single_line(event_name(record)));
        text.push_str(&detail_text(record));
    }
    DetailCache {
        id,
        width,
        version: stats.accepted,
        records: stats.records,
        link,
        rows: detail_rows(&text, width),
    }
}

/// Preserve the complete record while showing arguments and output as readable text.
#[must_use]
pub fn detail_text(record: &TelemetryRecord) -> String {
    let mut text = String::new();
    let detail = command_detail(&from_telemetry_record(record));
    if let Some(command) = &detail.command {
        text.push_str("Command:\n");
        text.push_str(&escape_display(command));
        text.push_str("\n\nShell: ");
        match &detail.shell {
            ShellKind::PowerShell(shell) => {
                text.push_str("PowerShell (");
                text.push_str(&escape_display(shell));
                text.push(')');
            }
            ShellKind::Other(shell) => text.push_str(&escape_display(shell)),
            ShellKind::Unknown => text.push_str("unknown or implicit"),
        }
        if let Some(workdir) = &detail.workdir {
            text.push_str("\nWorking directory: ");
            text.push_str(&escape_display(workdir));
        }
        text.push_str("\n\n");
    }
    if detail.output_truncated == Some(true) {
        text.push_str("Sender reports output was truncated.\n\n");
    }
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
    run_controlled(port, events_file, cancellation, &mut AlwaysVisible).await
}

/// Run the receiver while an optional host controls its owned terminal.
/// # Errors
/// Returns an error for terminal, host, bind, file, output or shutdown failures.
pub async fn run_controlled(
    port: u16,
    events_file: Option<&str>,
    cancellation: CancellationToken,
    control: &mut dyn TerminalControl,
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
    let result = run_terminal(address, &store, file.as_mut(), &cancellation, control);
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
    control: &mut dyn TerminalControl,
) -> Result<TelemetryCursor> {
    let mut guard = InlineTerminal::open(BufWriter::new(std::io::stderr()), 12)
        .wrap_err("open inline terminal")?;
    let terminal = guard.terminal_mut().context("inline terminal is closed")?;
    let capture = TerminalCapture::current().context("terminal log capture not installed")?;
    let mut cursor = TelemetryCursor::default();
    let mut navigation = Navigation::default();
    let mut details_cache: Option<DetailCache> = None;
    loop {
        let visibility = control.poll()?;
        if visibility == TerminalState::Exit {
            break;
        }
        let width = terminal.size()?.width;
        let (records, stats, selected_summary, detail_input) = {
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
            let detail_input = if matches!(navigation.screen, Screen::Detail { .. }) {
                navigation
                    .selected
                    .and_then(|id| detail_update(&store, id, width, details_cache.as_mut()))
            } else {
                None
            };
            (records, stats, summary, detail_input)
        };
        if let (Some(id), Some((selected, related, link))) = (navigation.selected, detail_input) {
            details_cache = Some(detail_cache(
                id,
                width,
                stats,
                &selected,
                related.as_ref(),
                link,
            ));
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
        let (pending, dropped_logs) =
            capture.snapshot_limit(process_log_batch_limit(visibility))?;
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
            && let Some(cache) = &details_cache
        {
            *scroll = (*scroll).min(cache.rows.len().saturating_sub(1));
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
    cache: Option<&DetailCache>,
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
            (Some(_), Some(cache)) if Some(cache.id) == navigation.selected => {
                let start = scroll.min(cache.rows.len().saturating_sub(1));
                let height = usize::from(height.saturating_sub(2));
                format!(
                    "Event {} · row {}/{} · arrows/PgUp/PgDn · Esc back\n{}",
                    cache.id,
                    start + 1,
                    cache.rows.len(),
                    cache
                        .rows
                        .iter()
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
