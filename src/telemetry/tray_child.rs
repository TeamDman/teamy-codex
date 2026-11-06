//! Console-only child. Standard streams are not used for its terminal.
use super::terminal_foundation::{InlineTerminal, print_above};
use super::terminal_navigation::{Navigation, Screen};
use super::terminal_view::{DetailCache, detail_cache, detail_update, event_summary};
use super::tray_visibility::SHOW_PROBE_KIND;
use super::ui_process::{Message, connect_child, read_next, write_message};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use eyre::{Context, ContextCompat, Result, bail};
use ratatui::widgets::Paragraph;
use ratatui::{Terminal, backend::Backend};
use std::fs::OpenOptions;
use std::io::{self, BufWriter, IsTerminal, Read};
use std::sync::mpsc;
use std::time::Duration;
use teamy_tracing::telemetry::{RecordStore, StoreStats, TelemetryRecord};

fn start_reader(
    mut input: Box<dyn Read + Send>,
) -> io::Result<mpsc::Receiver<io::Result<Option<Message>>>> {
    let (sender, receiver) = mpsc::sync_channel(8);
    let _reader = std::thread::Builder::new()
        .name("ui-owner-reader".into())
        .spawn(move || {
            loop {
                let message = read_next(&mut input);
                let finished = !matches!(message, Ok(Some(_)));
                if sender.send(message).is_err() || finished {
                    break;
                }
            }
        })?;
    Ok(receiver)
}

fn show_confirmation(generation: u64, payload: Vec<u8>) -> Message {
    Message::Intent {
        generation,
        kind: SHOW_PROBE_KIND,
        payload,
    }
}

fn open_console() -> Result<(std::fs::File, std::fs::File)> {
    let input = OpenOptions::new().read(true).write(true).open("CONIN$")?;
    let output = OpenOptions::new().read(true).write(true).open("CONOUT$")?;
    if !input.is_terminal() || !output.is_terminal() {
        bail!("UI child requires its own console");
    }
    Ok((input, output))
}

fn receiver_address(args: &[std::ffi::OsString]) -> Result<String> {
    Ok(args
        .first()
        .and_then(|arg| arg.to_str())
        .context("missing receiver address")?
        .to_owned())
}

fn present<B: Backend>(
    terminal: &mut Terminal<B>,
    records: &mut RecordStore,
    payload: &[u8],
) -> Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let (kind, data) = payload.split_first().context("empty UI record")?;
    let text = std::str::from_utf8(data).wrap_err("UI record is not UTF-8")?;
    match *kind {
        b'E' => {
            let record: TelemetryRecord = owned_facet_json::from_str(text)
                .map_err(|error| eyre::eyre!("decode UI event: {error}"))?;
            print_above(terminal, &event_summary(&record))?;
            if !records.push(record) {
                bail!("UI event exceeded local bounds");
            }
        }
        b'L' | b'S' => print_above(terminal, text)?,
        _ => bail!("unknown UI record kind"),
    }
    Backend::flush(terminal.backend_mut())?;
    Ok(())
}

fn view_text(
    address: &str,
    nav: &Navigation,
    cache: Option<&DetailCache>,
    records: &RecordStore,
    gap: (u64, u64),
    height: u16,
) -> String {
    let stats = records.stats();
    match nav.screen {
        Screen::Overview => {
            let selected = nav
                .selected_index(stats)
                .and_then(|index| records.iter().nth(index));
            format!(
                "OTLP logs: {address} · q closes this UI\nRetained UI events: {} · history evicted: {} · rejected: {}\nSelected: {}\nUp/Down select · Enter inspect · tray Exit stops the owner",
                stats.records,
                gap.0,
                gap.1,
                selected.map_or_else(|| "none or evicted".to_owned(), event_summary)
            )
        }
        Screen::Detail { scroll } => match (cache, nav.selected_index(stats)) {
            (Some(cache), Some(_)) if Some(cache.id) == nav.selected => {
                let start = scroll.min(cache.rows.len().saturating_sub(1));
                format!(
                    "Event {id} · row {}/{} · arrows/PgUp/PgDn · Esc back\n{}",
                    start + 1,
                    cache.rows.len(),
                    cache
                        .rows
                        .iter()
                        .skip(start)
                        .take(usize::from(height.saturating_sub(2)))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n"),
                    id = cache.id
                )
            }
            _ => "Selected event was evicted. Press Esc to return.".into(),
        },
    }
}

fn handle_key(key: event::KeyEvent, nav: &mut Navigation, stats: StoreStats) -> bool {
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
        KeyCode::Esc if nav.back() => return true,
        KeyCode::Up => nav.previous(stats),
        KeyCode::Down => nav.next(stats),
        KeyCode::PageUp => {
            for _ in 0..10 {
                nav.previous(stats);
            }
        }
        KeyCode::PageDown => {
            for _ in 0..10 {
                nav.next(stats);
            }
        }
        KeyCode::Home => {
            if let Screen::Detail { scroll } = &mut nav.screen {
                *scroll = 0;
            }
        }
        KeyCode::End => {
            if let Screen::Detail { scroll } = &mut nav.screen {
                *scroll = usize::MAX;
            }
        }
        KeyCode::Enter => nav.enter_detail(stats),
        _ => {}
    }
    false
}

/// Run the UI child using its dedicated owner pipes and own console.
/// # Errors
/// Returns an error for bootstrap, protocol, terminal or output failures.
pub fn main() -> Result<()> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let address = receiver_address(&args)?;
    let (mut input, mut output) = connect_child(&args)?;
    let Some(Message::Hello { generation }) = read_next(&mut input)? else {
        bail!("missing UI hello");
    };
    let (_console_input, console_output) = open_console()?;
    let mut guard = InlineTerminal::open(BufWriter::new(console_output), 12)?;
    let terminal = guard.terminal_mut().context("terminal closed")?;
    write_message(&mut output, &Message::Ready { generation })?;
    let receiver = start_reader(input)?;
    let mut records = RecordStore::default();
    let mut nav = Navigation::default();
    let mut gap = (0u64, 0u64);
    let mut last_sequence = 0u64;
    let mut cache: Option<DetailCache> = None;
    'ui: loop {
        for _ in 0..8 {
            match receiver.try_recv() {
                Ok(Ok(Some(Message::Gap {
                    generation: id,
                    evicted,
                    rejected,
                }))) if id == generation => gap = (evicted, rejected),
                Ok(Ok(Some(Message::Record {
                    generation: id,
                    sequence,
                    payload,
                }))) if id == generation && sequence > last_sequence => {
                    present(terminal, &mut records, &payload)?;
                    write_message(
                        &mut output,
                        &Message::Ack {
                            generation,
                            sequence,
                        },
                    )?;
                    last_sequence = sequence;
                }
                Ok(Ok(Some(Message::AppEvent {
                    generation: id,
                    kind: SHOW_PROBE_KIND,
                    payload,
                }))) if id == generation && payload.len() == 8 => {
                    write_message(&mut output, &show_confirmation(generation, payload))?;
                }
                Ok(Ok(Some(Message::Close { generation: id }))) if id == generation => break 'ui,
                Ok(Ok(None)) | Err(mpsc::TryRecvError::Disconnected) => break 'ui,
                Ok(Err(error)) => return Err(error.into()),
                Ok(Ok(Some(_))) => bail!("invalid or stale UI message"),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        let stats = records.stats();
        nav.initialize_selection(stats);
        let width = terminal.size()?.width;
        if matches!(nav.screen, Screen::Detail { .. })
            && let Some(id) = nav.selected
            && let Some((selected, related, link)) =
                detail_update(&records, id, width, cache.as_mut())
        {
            cache = Some(detail_cache(
                id,
                width,
                stats,
                &selected,
                related.as_ref(),
                link,
            ));
        }
        terminal.draw(|frame| {
            let text = view_text(
                &address,
                &nav,
                cache.as_ref(),
                &records,
                gap,
                frame.area().height,
            );
            frame.render_widget(Paragraph::new(text), frame.area());
        })?;
        if let Screen::Detail { scroll } = &mut nav.screen
            && let Some(cache) = &cache
        {
            *scroll = (*scroll).min(cache.rows.len().saturating_sub(1));
        }
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && handle_key(key, &mut nav, stats)
        {
            write_message(&mut output, &Message::Close { generation })?;
            break;
        }
    }
    guard.close()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::tray_owner::retain_event;
    use crate::telemetry::ui_process::History;
    use crossterm::event::KeyEvent;
    use ratatui::{TerminalOptions, Viewport, backend::TestBackend};
    use teamy_tracing::telemetry::{Attribute, TelemetryValue};

    #[test]
    fn synthetic_child_presents_escaped_log_and_inspectable_event() {
        let mut terminal = Terminal::with_options(
            TestBackend::new(64, 12),
            TerminalOptions {
                viewport: Viewport::Inline(6),
            },
        )
        .unwrap();
        let mut records = RecordStore::default();
        present(&mut terminal, &mut records, b"Lsynthetic \x1b[31m").unwrap();
        let record = TelemetryRecord {
            event_name: "codex.tool_result".into(),
            attributes: vec![Attribute {
                key: "arguments".into(),
                value: TelemetryValue::String("Get-Date\u{1b}".into()),
            }],
            ..TelemetryRecord::default()
        };
        let mut history = History::new(4, 4096).unwrap();
        assert!(retain_event(&mut history, &record).unwrap());
        present(
            &mut terminal,
            &mut records,
            &history.iter().next().unwrap().payload,
        )
        .unwrap();
        assert_eq!(records.stats().records, 1);
        let mut nav = Navigation::default();
        nav.initialize_selection(records.stats());
        assert!(!handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut nav,
            records.stats()
        ));
        let id = nav.selected.unwrap();
        let record = records.iter().next().unwrap();
        let cache = detail_cache(id, 64, records.stats(), record, None, None);
        let detail = view_text("synthetic", &nav, Some(&cache), &records, (2, 1), 12);
        assert!(detail.contains("Get-Date\\u{1b}"));
        assert!(!detail.contains('\u{1b}'));
        assert!(!handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut nav,
            records.stats()
        ));
        assert!(handle_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &mut nav,
            records.stats()
        ));
        let buffer = terminal.backend().buffer();
        let visible = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(visible.contains("synthetic \\u{1b}[31m"));
    }
}
